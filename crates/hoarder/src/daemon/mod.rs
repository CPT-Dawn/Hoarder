//! `hoarder daemon`: one single-threaded `calloop` loop that watches the
//! Selections, reads every Offer by the Reader rules, takes a Selection over
//! when its source app goes away (ADR-0005), and answers Front ends on the
//! socket (ADR-0004). It sleeps in the kernel until something happens: no
//! timers run while idle.

mod history;
mod reader;
pub mod service;
mod socket;

use std::collections::VecDeque;
use std::fs::File;
use std::io::{ErrorKind, Read as _, Write as _};
use std::os::fd::OwnedFd;
use std::sync::Arc;
use std::time::Instant;

use calloop::generic::Generic;
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, Interest, LoopHandle, Mode, PostAction, RegistrationToken};
use hoarder_config::Config;
use hoarder_ipc::{Owner, SelectionStatus, Status};
use hoarder_wayland::clipboard::{ClipboardError, Selection};
use hoarder_wayland::watch::{Offer, WatchEvent, Watcher};

use crate::Exit;
use crate::clipboard::PASSWORD_HINT;

type Fail = (Exit, String);

const SELECTIONS: [Selection; 2] = [Selection::Clipboard, Selection::Primary];

/// Run the Daemon until the compositor goes away.
pub fn run(overrides: &[(String, String)]) -> Result<(), Fail> {
    let config = crate::config::effective(overrides)?;
    let watcher = Watcher::connect().map_err(wayland_error)?;
    let listener = socket::bind()?;

    let mut event_loop: EventLoop<'static, Daemon> = EventLoop::try_new()
        .map_err(|e| (Exit::Io, format!("cannot start the event loop: {e}")))?;
    let handle = event_loop.handle();
    let loop_error = |e: calloop::Error| (Exit::Io, format!("event loop: {e}"));

    let wayland_fd = watcher.poll_fd().map_err(|e| (Exit::Io, e.to_string()))?;
    handle
        .insert_source(
            Generic::new(wayland_fd, Interest::READ, Mode::Level),
            |_, _, daemon: &mut Daemon| daemon.on_wayland(),
        )
        .map_err(|e| loop_error(e.error))?;
    socket::listen(&handle, listener)?;

    let config_path = hoarder_config::path();
    let config_watch = match hoarder_config::Watcher::new(&config_path) {
        Ok(w) => {
            let fd = std::os::fd::AsFd::as_fd(&w)
                .try_clone_to_owned()
                .map_err(|e| (Exit::Io, e.to_string()))?;
            handle
                .insert_source(
                    Generic::new(fd, Interest::READ, Mode::Level),
                    |_, _, daemon: &mut Daemon| {
                        daemon.on_config_changed();
                        Ok(PostAction::Continue)
                    },
                )
                .map_err(|e| loop_error(e.error))?;
            Some(w)
        }
        Err(e) => {
            eprintln!(
                "hoarder: cannot watch {}: {e}; config changes need a restart",
                config_path.display()
            );
            None
        }
    };

    let history = history::History::start(&handle);
    let mut daemon = Daemon {
        handle,
        history,
        watcher,
        config,
        overrides: overrides.to_vec(),
        config_watch,
        slots: Default::default(),
        clients: Default::default(),
        syncs: Vec::new(),
        next_token: 0,
        fatal: None,
    };
    // The Selections as they were at startup.
    daemon.on_wayland().map_err(|e| (Exit::Io, e.to_string()))?;
    daemon.flush();
    while daemon.fatal.is_none() {
        event_loop.dispatch(None, &mut daemon).map_err(loop_error)?;
        daemon.flush();
    }
    Err(daemon.fatal.take().expect("loop ended on a fatal error"))
}

/// `hoarder clear`: through the Daemon when it runs, so it forgets the
/// clipboard too; otherwise straight over data control.
pub fn clear() -> Result<(), Fail> {
    if let Ok(mut daemon) = hoarder_ipc::Connection::connect() {
        return match daemon.call(hoarder_ipc::Op::Clear) {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(failure)) => Err((Exit::Io, failure.message)),
            Err(e) => Err((Exit::Io, format!("lost the Daemon: {e}"))),
        };
    }
    hoarder_wayland::clipboard::Clipboard::connect(None)
        .and_then(|mut c| c.clear(Selection::Clipboard))
        .map_err(wayland_error)
}

fn wayland_error(e: ClipboardError) -> Fail {
    let code = match e {
        ClipboardError::NoWayland(_) | ClipboardError::Unsupported(_) => Exit::Unsupported,
        ClipboardError::Failed(_) | ClipboardError::Io(_) => Exit::Io,
    };
    (code, e.to_string())
}

/// One representation the Daemon holds: a MIME type and its bytes.
type Representation = (String, Arc<[u8]>);

/// What the Daemon knows and holds for one Selection.
#[derive(Debug, Default)]
struct Slot {
    owner: Option<Owner>,
    /// Types offered now, in the source's order.
    offered: Vec<String>,
    /// What was read from the latest app Offer, for Takeover.
    kept: Vec<Representation>,
    /// Whether `kept` may be served once its source goes away.
    may_take_over: bool,
    /// Whether `kept` goes to History once read.
    record: bool,
    read: Option<ReadState>,
    /// The Selection went empty mid-read: take over once the read ends.
    take_over_after_read: bool,
    /// Bumped on every change, so stale read callbacks know to stop.
    generation: u64,
}

/// An Offer being read, one type at a time.
#[derive(Debug)]
struct ReadState {
    offer: Offer,
    queue: VecDeque<String>,
    deadline: Instant,
    current: Option<InFlight>,
    bytes: usize,
}

/// The type being read right now.
#[derive(Debug)]
struct InFlight {
    mime_type: String,
    data: Vec<u8>,
    pipe: RegistrationToken,
    timer: RegistrationToken,
}

/// How a type's read ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    Complete,
    TimedOut,
    TooBig,
}

pub(crate) struct Daemon {
    handle: LoopHandle<'static, Daemon>,
    watcher: Watcher,
    config: Config,
    overrides: Vec<(String, String)>,
    config_watch: Option<hoarder_config::Watcher>,
    slots: [Slot; 2],
    history: history::History,
    clients: socket::Clients,
    /// `(sync token, client, request id, answer)` of requests answered once
    /// the compositor has caught up.
    syncs: Vec<(u64, u64, u64, hoarder_ipc::Value)>,
    next_token: u64,
    fatal: Option<Fail>,
}

impl Daemon {
    fn flush(&mut self) {
        if let Err(e) = self.watcher.flush() {
            self.fatal.get_or_insert(wayland_error(e));
        }
    }

    fn on_wayland(&mut self) -> std::io::Result<PostAction> {
        let events = match self.watcher.dispatch() {
            Ok(events) => events,
            Err(e) => {
                self.fatal = Some(wayland_error(e));
                return Ok(PostAction::Remove);
            }
        };
        for event in events {
            match event {
                WatchEvent::Changed { which, offer } => self.selection_changed(which, offer),
                WatchEvent::Send {
                    which,
                    mime_type,
                    fd,
                } => self.send(which, &mime_type, fd),
                WatchEvent::Cancelled { .. } => {}
                WatchEvent::Synced(token) => self.synced(token),
            }
        }
        Ok(PostAction::Continue)
    }

    /// Whether the Daemon handles `which` at all: the primary selection is
    /// ignored unless the config asks for it.
    fn watched(&self, which: Selection) -> bool {
        match which {
            Selection::Clipboard => true,
            Selection::Primary => {
                let p = &self.config.clipboard.primary;
                p.persist || p.history
            }
        }
    }

    fn persists(&self, which: Selection) -> bool {
        match which {
            Selection::Clipboard => self.config.clipboard.persist,
            Selection::Primary => self.config.clipboard.primary.persist,
        }
    }

    /// Whether Normal changes of `which` go to History.
    fn records(&self, which: Selection) -> bool {
        self.config.history.enabled
            && match which {
                Selection::Clipboard => true,
                Selection::Primary => self.config.clipboard.primary.history,
            }
    }

    fn selection_changed(&mut self, which: Selection, offer: Option<Offer>) {
        if !self.watched(which) {
            return;
        }
        let Some(offer) = offer else {
            return self.selection_emptied(which);
        };
        if offer.own {
            let slot = &mut self.slots[which as usize];
            slot.owner = Some(Owner::Hoarder);
            slot.offered = offer.mime_types;
            return;
        }
        self.stop_reading(which);
        let secret = offer.mime_types.iter().any(|m| m == PASSWORD_HINT);
        let excluded = offer
            .focused_app
            .as_ref()
            .is_some_and(|app| self.config.clipboard.no_takeover_apps.contains(app));
        let may_take_over = self.persists(which) && !secret && !excluded;
        let record = !secret && self.records(which);
        let plan = if secret {
            // Secret: never touched (ADR-0006).
            Vec::new()
        } else {
            reader::plan(&offer.mime_types, &self.config.clipboard.deny_types)
        };
        let slot = &mut self.slots[which as usize];
        slot.owner = Some(Owner::App);
        slot.offered = offer.mime_types.clone();
        slot.kept.clear();
        slot.may_take_over = may_take_over;
        slot.record = record;
        slot.take_over_after_read = false;
        slot.read = Some(ReadState {
            offer,
            queue: plan.into(),
            deadline: Instant::now() + reader::PER_OFFER,
            current: None,
            bytes: 0,
        });
        self.read_next(which);
    }

    fn selection_emptied(&mut self, which: Selection) {
        let slot = &mut self.slots[which as usize];
        slot.owner = Some(Owner::None);
        slot.offered.clear();
        match &mut slot.read {
            // The source is gone: finish the type in flight, skip the rest.
            Some(read) => {
                read.queue.clear();
                slot.take_over_after_read = true;
            }
            None => self.take_over(which),
        }
    }

    /// Serve what was kept, now that its source has gone (Takeover).
    fn take_over(&mut self, which: Selection) {
        let slot = &self.slots[which as usize];
        if !slot.may_take_over || slot.kept.is_empty() || !self.persists(which) {
            return;
        }
        let types: Vec<String> = slot.kept.iter().map(|(m, _)| m.clone()).collect();
        self.watcher.serve(which, &types);
    }

    /// Answer a paste from our own Offer, off-thread so a slow reader never
    /// stalls the loop.
    fn send(&mut self, which: Selection, mime_type: &str, fd: OwnedFd) {
        let data = self.slots[which as usize]
            .kept
            .iter()
            .find(|(m, _)| m == mime_type)
            .map(|(_, d)| Arc::clone(d));
        if let Some(data) = data {
            std::thread::spawn(move || {
                // A reader that hangs up early is its own business.
                let _ = File::from(fd).write_all(&data);
            });
        }
    }

    /// Start reading the next planned type, or finish the read.
    fn read_next(&mut self, which: Selection) {
        let generation = {
            let slot = &mut self.slots[which as usize];
            slot.generation += 1;
            slot.generation
        };
        loop {
            let slot = &mut self.slots[which as usize];
            let Some(read) = &mut slot.read else { return };
            let now = Instant::now();
            let next = if now < read.deadline {
                read.queue.pop_front()
            } else {
                None
            };
            let Some(mime_type) = next else {
                return self.finish_read(which);
            };
            let fd = match self.watcher.receive(&read.offer, &mime_type) {
                Ok(fd) => fd,
                Err(_) => continue,
            };
            let deadline = read.deadline.min(now + reader::PER_TYPE);
            let pipe = self.handle.insert_source(
                Generic::new(File::from(fd), Interest::READ, Mode::Level),
                move |_, file, daemon: &mut Daemon| Ok(daemon.on_pipe(which, generation, file)),
            );
            let timer = self.handle.insert_source(
                Timer::from_deadline(deadline),
                move |_, _, daemon: &mut Daemon| {
                    daemon.on_read_timeout(which, generation);
                    TimeoutAction::Drop
                },
            );
            match (pipe, timer) {
                (Ok(pipe), Ok(timer)) => {
                    read.current = Some(InFlight {
                        mime_type,
                        data: Vec::new(),
                        pipe,
                        timer,
                    });
                    return;
                }
                (pipe, timer) => {
                    for token in [pipe.ok(), timer.ok()].into_iter().flatten() {
                        self.handle.remove(token);
                    }
                }
            }
        }
    }

    fn on_pipe(&mut self, which: Selection, generation: u64, file: &File) -> PostAction {
        let max = self.config.clipboard.max_offer_mb.get() as usize * 1024 * 1024;
        let slot = &mut self.slots[which as usize];
        if slot.generation != generation {
            return PostAction::Remove;
        }
        let Some(read) = &mut slot.read else {
            return PostAction::Remove;
        };
        let Some(current) = &mut read.current else {
            return PostAction::Remove;
        };
        let mut chunk = [0u8; 64 * 1024];
        let ended = loop {
            match (&*file).read(&mut chunk) {
                Ok(0) => break Ended::Complete,
                Ok(n) => {
                    current.data.extend_from_slice(&chunk[..n]);
                    if read.bytes + current.data.len() > max {
                        break Ended::TooBig;
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => return PostAction::Continue,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                // A source that breaks the pipe has not given us this type.
                Err(_) => break Ended::TimedOut,
            }
        };
        self.end_type(which, ended, true);
        PostAction::Remove
    }

    fn on_read_timeout(&mut self, which: Selection, generation: u64) {
        if self.slots[which as usize].generation == generation {
            self.end_type(which, Ended::TimedOut, false);
        }
    }

    /// The type in flight is done; keep it if it completed, then go on.
    /// `from_pipe` says the pipe source removes itself.
    fn end_type(&mut self, which: Selection, ended: Ended, from_pipe: bool) {
        let slot = &mut self.slots[which as usize];
        let Some(read) = &mut slot.read else { return };
        let Some(current) = read.current.take() else {
            return;
        };
        if from_pipe {
            self.handle.remove(current.timer);
        } else {
            self.handle.remove(current.pipe);
        }
        if ended == Ended::Complete {
            read.bytes += current.data.len();
            slot.kept.push((
                current.mime_type,
                Arc::from(current.data.into_boxed_slice()),
            ));
        }
        self.read_next(which);
    }

    fn finish_read(&mut self, which: Selection) {
        let slot = &mut self.slots[which as usize];
        let read = slot.read.take();
        if let Some(read) = read
            && std::mem::take(&mut slot.record)
            && !slot.kept.is_empty()
        {
            let job = history::Job::Record {
                reprs: slot.kept.clone(),
                app: read.offer.focused_app,
            };
            // Without History there is nothing to record into.
            let _ = self.history.send(job);
        }
        let slot = &mut self.slots[which as usize];
        if std::mem::take(&mut slot.take_over_after_read) {
            self.take_over(which);
        }
    }

    /// Abandon any read in progress on `which`.
    fn stop_reading(&mut self, which: Selection) {
        let slot = &mut self.slots[which as usize];
        slot.generation += 1;
        if let Some(read) = slot.read.take()
            && let Some(current) = read.current
        {
            self.handle.remove(current.pipe);
            self.handle.remove(current.timer);
        }
    }

    /// `hoarder clear`: forget the clipboard and empty it, so nothing is
    /// restored. The reply waits until the compositor has applied it.
    fn clear(&mut self, client: u64, id: u64) {
        let which = Selection::Clipboard;
        self.stop_reading(which);
        let slot = &mut self.slots[which as usize];
        slot.kept.clear();
        slot.take_over_after_read = false;
        self.watcher.clear(which);
        self.reply_when_synced(client, id, hoarder_ipc::Value::Null);
    }

    /// Answer `id` with `answer` once the compositor has applied what was
    /// just asked of it.
    fn reply_when_synced(&mut self, client: u64, id: u64, answer: hoarder_ipc::Value) {
        let token = self.next_token;
        self.next_token += 1;
        self.watcher.sync(token);
        self.syncs.push((token, client, id, answer));
    }

    fn synced(&mut self, token: u64) {
        if let Some(i) = self.syncs.iter().position(|(t, ..)| *t == token) {
            let (_, client, id, answer) = self.syncs.remove(i);
            self.reply(client, id, Ok(answer));
        }
    }

    /// The History thread has finished a job.
    fn history_done(&mut self, done: history::Done) {
        match done {
            history::Done::Reply { client, id, result } => self.reply(client, id, result),
            history::Done::Restore {
                client,
                id,
                clip,
                reprs,
            } => self.restore(client, id, clip, reprs),
        }
    }

    /// Restore: own the clipboard with a Clip's representations.
    fn restore(
        &mut self,
        client: u64,
        id: u64,
        clip: hoarder_ipc::ClipSummary,
        reprs: Vec<Representation>,
    ) {
        let which = Selection::Clipboard;
        self.stop_reading(which);
        let slot = &mut self.slots[which as usize];
        slot.kept = reprs;
        slot.may_take_over = true;
        slot.record = false;
        slot.take_over_after_read = false;
        let types: Vec<String> = slot.kept.iter().map(|(m, _)| m.clone()).collect();
        self.watcher.serve(which, &types);
        self.reply_when_synced(client, id, history::to_value(&clip));
    }

    fn status(&self) -> Status {
        let selection = |which: Selection| {
            let slot = &self.slots[which as usize];
            SelectionStatus {
                owner: slot.owner.unwrap_or(Owner::None),
                types: slot.offered.clone(),
                kept: slot.kept.iter().map(|(m, _)| m.clone()).collect(),
                reading: slot.read.is_some(),
            }
        };
        Status {
            daemon: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
            data_control: self.watcher.data_control().name().into(),
            persist: self.config.clipboard.persist,
            clipboard: selection(Selection::Clipboard),
            primary: self
                .watched(Selection::Primary)
                .then(|| selection(Selection::Primary)),
        }
    }

    /// Re-read `config.toml`. A broken file keeps the config in force.
    fn on_config_changed(&mut self) {
        let Some(watch) = &mut self.config_watch else {
            return;
        };
        if !matches!(watch.changed(), Ok(true)) {
            return;
        }
        let path = hoarder_config::path();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => String::new(),
            Err(e) => {
                eprintln!("hoarder: cannot read {}: {e}", path.display());
                return;
            }
        };
        if let Err(d) = hoarder_config::parse(&text) {
            eprintln!(
                "hoarder: {}:{d}; keeping the previous config",
                path.display()
            );
            return;
        }
        match hoarder_config::with_overrides(&text, &self.overrides) {
            Ok(config) => {
                self.config = config;
                for which in SELECTIONS {
                    if !self.watched(which) {
                        self.stop_reading(which);
                        self.slots[which as usize] = Slot::default();
                    }
                }
            }
            Err(e) => eprintln!("hoarder: --set {e}"),
        }
    }
}
