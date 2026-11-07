//! `hoarder daemon`: one single-threaded `calloop` loop that watches the
//! Selections, reads every Offer by the Reader rules, takes a Selection over
//! when its source app goes away (ADR-0005), and answers Front ends on the
//! socket (ADR-0004). It sleeps in the kernel until something happens: no
//! timers run while idle.

mod classify;
mod concealed;
mod history;
mod keyring;
mod pause;
mod reader;
pub mod service;
mod socket;

use std::collections::VecDeque;
use std::fs::File;
use std::io::{ErrorKind, Read as _, Write as _};
use std::os::fd::OwnedFd;
use std::sync::Arc;
use std::time::{Duration, Instant};

use calloop::generic::Generic;
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, Interest, LoopHandle, Mode, PostAction, RegistrationToken};
use hoarder_config::Config;
use hoarder_ipc::{Owner, SelectionStatus, Sensitivity, Status};
use hoarder_wayland::clipboard::{ClipboardError, Selection};
use hoarder_wayland::watch::{Offer, WatchEvent, Watcher};
use zeroize::Zeroize as _;

use crate::Exit;
use crate::clipboard::PASSWORD_HINT;

type Fail = (Exit, String);

/// How often retention runs besides on every insert.
const RETENTION_EVERY: Duration = Duration::from_secs(60 * 60);

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

    let history = history::History::start(&handle, config.history.encrypt);
    // Retention runs on every insert, at startup and once an hour.
    let _ = history.send(history::Job::Retain {
        limits: history::limits(&config.history),
    });
    handle
        .insert_source(
            Timer::from_duration(RETENTION_EVERY),
            |_, _, daemon: &mut Daemon| {
                daemon.retain();
                TimeoutAction::ToDuration(RETENTION_EVERY)
            },
        )
        .map_err(|e| loop_error(e.error))?;
    let classifier = classifier(&config);
    let mut daemon = Daemon {
        handle,
        history,
        classifier,
        vault: Default::default(),
        watcher,
        config,
        overrides: overrides.to_vec(),
        config_watch,
        slots: Default::default(),
        current: Default::default(),
        clients: Default::default(),
        syncs: Vec::new(),
        next_token: 0,
        pause: None,
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
pub fn clear(which: Selection) -> Result<(), Fail> {
    if let Ok(mut daemon) = hoarder_ipc::Connection::connect() {
        let op = hoarder_ipc::Op::Clear {
            selection: to_ipc(which),
        };
        return match daemon.call(op) {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(failure)) => Err((Exit::Io, failure.message)),
            Err(e) => Err((Exit::Io, format!("lost the Daemon: {e}"))),
        };
    }
    hoarder_wayland::clipboard::Clipboard::connect(None)
        .and_then(|mut c| c.clear(which))
        .map_err(wayland_error)
}

pub(crate) fn to_ipc(which: Selection) -> hoarder_ipc::Selection {
    match which {
        Selection::Clipboard => hoarder_ipc::Selection::Clipboard,
        Selection::Primary => hoarder_ipc::Selection::Primary,
    }
}

fn from_ipc(which: hoarder_ipc::Selection) -> Selection {
    match which {
        hoarder_ipc::Selection::Clipboard => Selection::Clipboard,
        hoarder_ipc::Selection::Primary => Selection::Primary,
    }
}

/// The Classifier for `config`; a bad extra pattern is skipped with a
/// warning, never fatal.
fn classifier(config: &Config) -> classify::Classifier {
    let (classifier, warnings) = classify::Classifier::new(&config.privacy);
    for warning in warnings {
        eprintln!("hoarder: {warning}");
    }
    classifier
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

/// Whether `mime` is plain text, as a plain-text Restore keeps it.
fn is_plain_text(mime: &str) -> bool {
    let essence = mime.split(';').next().unwrap_or(mime).trim();
    essence == "text/plain" || matches!(essence, "UTF8_STRING" | "STRING" | "TEXT")
}

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
    /// Our Offer serves one paste, then gives the Selection up (`copy -o`).
    once: bool,
    /// How private the current Selection is (ADR-0006).
    sensitivity: Sensitivity,
    /// The Concealed Clip in the vault the Selection holds, in place of
    /// `kept`. Cleared on any change, so at expiry it says whether the
    /// Selection still holds that Clip.
    concealed: Option<u64>,
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

/// An `offer` request, its payload read.
#[derive(Debug)]
struct OfferRequest {
    which: Selection,
    types: Vec<String>,
    sensitive: bool,
    once: bool,
    data: Vec<u8>,
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
    /// The Offer on each Selection right now, watched or not, for `paste`.
    current: [Option<Offer>; 2],
    history: history::History,
    classifier: classify::Classifier,
    /// Concealed Clips, never written anywhere.
    vault: concealed::Vault,
    clients: socket::Clients,
    /// `(sync token, client, request id, answer)` of requests answered once
    /// the compositor has caught up.
    syncs: Vec<(u64, u64, u64, hoarder_ipc::Value)>,
    next_token: u64,
    /// Set while Paused: nothing is recorded.
    pause: Option<pause::Pause>,
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
                WatchEvent::Changed { which, offer } => {
                    self.current[which as usize] = offer.clone();
                    self.broadcast(&hoarder_ipc::Event::Selection {
                        selection: to_ipc(which),
                        types: offer.as_ref().map(|o| o.mime_types.clone()),
                    });
                    self.selection_changed(which, offer);
                }
                WatchEvent::Send {
                    which,
                    mime_type,
                    fd,
                } => self.send(which, &mime_type, fd),
                WatchEvent::Cancelled { which } => {
                    // Nothing else tracks an unwatched Selection: let go of
                    // what we served on it.
                    if !self.watched(which) {
                        self.slots[which as usize] = Slot::default();
                    }
                }
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
        let early = self
            .classifier
            .offer(&offer.mime_types, offer.focused_app.as_deref());
        let secret = early == Some(Sensitivity::Secret);
        let excluded = offer
            .focused_app
            .as_ref()
            .is_some_and(|app| self.config.clipboard.no_takeover_apps.contains(app));
        let may_take_over = self.persists(which) && !secret && !excluded;
        let record = !secret && self.records(which) && self.pause.is_none();
        let plan = if secret {
            // Secret: never touched (ADR-0006).
            Vec::new()
        } else {
            reader::plan(&offer.mime_types, &self.config.clipboard.deny_types)
        };
        let slot = &mut self.slots[which as usize];
        slot.owner = Some(Owner::App);
        slot.offered = offer.mime_types.clone();
        concealed::scrub(&mut slot.kept);
        slot.may_take_over = may_take_over;
        slot.record = record;
        slot.once = false;
        // Provisional until the read shows whether the text looks private.
        slot.sensitivity = early.unwrap_or_default();
        slot.concealed = None;
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
        let types = self.held_types(which);
        if !self.slots[which as usize].may_take_over || types.is_empty() || !self.persists(which) {
            return;
        }
        self.watcher.serve(which, &types);
    }

    /// The types the Daemon holds for `which`: what was kept, or the
    /// Concealed Clip in its place.
    fn held_types(&self, which: Selection) -> Vec<String> {
        let slot = &self.slots[which as usize];
        match slot.concealed {
            Some(id) => self.vault.types(id),
            None => slot.kept.iter().map(|(m, _)| m.clone()).collect(),
        }
    }

    /// Answer a paste from our own Offer, off-thread so a slow reader never
    /// stalls the loop.
    fn send(&mut self, which: Selection, mime_type: &str, fd: OwnedFd) {
        let slot = &mut self.slots[which as usize];
        if let Some(id) = slot.concealed {
            if let Some(data) = self.vault.get(id, mime_type) {
                std::thread::spawn(move || {
                    let _ = File::from(fd).write_all(data.bytes());
                });
            }
        } else if let Some(data) = slot
            .kept
            .iter()
            .find(|(m, _)| m == mime_type)
            .map(|(_, d)| Arc::clone(d))
        {
            std::thread::spawn(move || {
                // A reader that hangs up early is its own business.
                let _ = File::from(fd).write_all(&data);
            });
        }
        if std::mem::take(&mut slot.once) {
            // Pasted once: give it up for good, as `wl-copy -o` exits.
            concealed::scrub(&mut slot.kept);
            slot.may_take_over = false;
            if let Some(id) = slot.concealed.take() {
                self.vault.forget(id);
            }
            self.watcher.clear(which);
        }
    }

    /// `offer`: serve `data` as `types` on `which`, as `copy` or a Capture
    /// asks. `sensitive` makes it Secret: served, never recorded nor
    /// restored. Otherwise the Classifier judges it like any copy: Concealed
    /// text lives in the vault and expires, Normal content is recorded.
    fn offer(&mut self, client: u64, id: u64, offer: OfferRequest) {
        let OfferRequest {
            which,
            types,
            sensitive,
            once,
            mut data,
        } = offer;
        self.stop_reading(which);
        let sensitivity = if sensitive {
            Sensitivity::Secret
        } else {
            let reprs = types.iter().map(|m| (m.as_str(), &data[..]));
            self.classifier.content(reprs)
        };
        let mut kept: Vec<Representation> = if sensitivity == Sensitivity::Concealed {
            // A copy per type, so the vault can wipe every one of them.
            types
                .iter()
                .map(|m| (m.clone(), Arc::from(&data[..])))
                .collect()
        } else {
            let shared: Arc<[u8]> = Arc::from(&data[..]);
            types
                .iter()
                .map(|m| (m.clone(), Arc::clone(&shared)))
                .collect()
        };
        data.zeroize();
        if sensitive {
            kept.push((PASSWORD_HINT.into(), Arc::from(&b"secret"[..])));
        }
        if sensitivity == Sensitivity::Normal && self.records(which) && self.pause.is_none() {
            let job = history::Job::Record {
                reprs: kept.clone(),
                app: None,
                limits: history::limits(&self.config.history),
            };
            // Without History there is nothing to record into.
            let _ = self.history.send(job);
        }
        let slot = &mut self.slots[which as usize];
        concealed::scrub(&mut slot.kept);
        if let Some(old) = slot.concealed.take() {
            self.vault.forget(old);
        }
        let slot = &mut self.slots[which as usize];
        slot.kept = kept;
        // A Secret is never restored (ADR-0006); a one-paste Offer is gone
        // once pasted.
        slot.may_take_over = sensitivity != Sensitivity::Secret && !once;
        slot.record = false;
        slot.once = once;
        slot.sensitivity = sensitivity;
        slot.take_over_after_read = false;
        if sensitivity == Sensitivity::Concealed {
            self.conceal(which, None);
        }
        let served = self.held_types(which);
        self.watcher.serve(which, &served);
        self.reply_when_synced(client, id, hoarder_ipc::Value::Null);
    }

    /// `paste`: have the source of `which` write `mime` into `pipe`.
    fn paste(&mut self, which: Selection, mime: &str, pipe: OwnedFd) -> Result<(), String> {
        let Some(offer) = &self.current[which as usize] else {
            return Err("Nothing is copied".into());
        };
        if !offer.mime_types.iter().any(|m| m == mime) {
            return Err(format!("the Selection is not offered as {mime}"));
        }
        let offer = offer.clone();
        self.watcher
            .receive_into(&offer, mime, pipe)
            .map_err(|e| e.to_string())
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
        // Any of it may turn out Concealed: leave no copies behind.
        let mut chunk = zeroize::Zeroizing::new([0u8; 64 * 1024]);
        let ended = loop {
            match (&*file).read(&mut chunk[..]) {
                Ok(0) => break Ended::Complete,
                Ok(n) => {
                    concealed::extend_scrubbed(&mut current.data, &chunk[..n]);
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
        let Some(mut current) = read.current.take() else {
            return;
        };
        if from_pipe {
            self.handle.remove(current.timer);
        } else {
            self.handle.remove(current.pipe);
        }
        if ended == Ended::Complete {
            read.bytes += current.data.len();
            slot.kept
                .push((current.mime_type, Arc::from(current.data.as_slice())));
        }
        current.data.zeroize();
        self.read_next(which);
    }

    fn finish_read(&mut self, which: Selection) {
        let slot = &mut self.slots[which as usize];
        let read = slot.read.take();
        if read.is_some() && slot.sensitivity == Sensitivity::Normal {
            let reprs = slot.kept.iter().map(|(m, d)| (m.as_str(), &d[..]));
            slot.sensitivity = self.classifier.content(reprs);
        }
        if slot.sensitivity == Sensitivity::Concealed {
            // Memory only: never recorded, and moved into the vault.
            slot.record = false;
            if let Some(read) = &read {
                self.conceal(which, read.offer.focused_app.clone());
            }
        }
        let slot = &mut self.slots[which as usize];
        if let Some(read) = read
            && std::mem::take(&mut slot.record)
            && !slot.kept.is_empty()
        {
            let job = history::Job::Record {
                reprs: slot.kept.clone(),
                app: read.offer.focused_app,
                limits: history::limits(&self.config.history),
            };
            // Without History there is nothing to record into.
            let _ = self.history.send(job);
        }
        let slot = &mut self.slots[which as usize];
        if std::mem::take(&mut slot.take_over_after_read) {
            self.take_over(which);
        }
    }

    /// Move what `which` keeps into the vault, to expire after the TTL.
    /// `app` is the app it was copied in, if known.
    fn conceal(&mut self, which: Selection, app: Option<String>) {
        let slot = &mut self.slots[which as usize];
        if slot.kept.is_empty() {
            return;
        }
        let ttl = Duration::from_secs(self.config.privacy.concealed_ttl_secs.get().into());
        let now = jiff::Timestamp::now().as_millisecond();
        let id = self
            .vault
            .keep(&mut slot.kept, app, now, ttl.as_millis() as i64);
        slot.concealed = Some(id);
        let timer = self.handle.insert_source(
            Timer::from_duration(ttl),
            move |_, _, daemon: &mut Daemon| {
                daemon.concealed_expired(id);
                TimeoutAction::Drop
            },
        );
        if timer.is_err() {
            // Without a timer it would never expire: don't hold it.
            self.vault.forget(id);
            self.slots[which as usize].concealed = None;
        }
    }

    /// Abandon any read in progress on `which`.
    fn stop_reading(&mut self, which: Selection) {
        let slot = &mut self.slots[which as usize];
        slot.generation += 1;
        if let Some(read) = slot.read.take()
            && let Some(mut current) = read.current
        {
            self.handle.remove(current.pipe);
            self.handle.remove(current.timer);
            current.data.zeroize();
        }
    }

    /// A Concealed Clip's TTL is up: forget it, and clear any Selection
    /// that still holds it, whether its app or the Daemon serves it.
    fn concealed_expired(&mut self, id: u64) {
        self.vault.forget(id);
        for which in SELECTIONS {
            let slot = &mut self.slots[which as usize];
            if slot.concealed != Some(id) {
                continue;
            }
            slot.concealed = None;
            slot.take_over_after_read = false;
            if slot.owner != Some(Owner::None) {
                self.watcher.clear(which);
            }
        }
    }

    /// `hoarder clear`: forget the clipboard and empty it, so nothing is
    /// restored. The reply waits until the compositor has applied it.
    fn clear(&mut self, client: u64, id: u64, which: Selection) {
        self.stop_reading(which);
        let slot = &mut self.slots[which as usize];
        concealed::scrub(&mut slot.kept);
        slot.once = false;
        slot.take_over_after_read = false;
        slot.sensitivity = Sensitivity::Normal;
        if let Some(id) = slot.concealed.take() {
            self.vault.forget(id);
        }
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
            history::Done::Changed(events) => {
                for event in &events {
                    self.broadcast(event);
                }
            }
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
        slot.once = false;
        slot.sensitivity = Sensitivity::Normal;
        slot.concealed = None;
        slot.take_over_after_read = false;
        let types: Vec<String> = slot.kept.iter().map(|(m, _)| m.clone()).collect();
        self.watcher.serve(which, &types);
        let answer = history::to_value(&clip);
        self.broadcast(&hoarder_ipc::Event::ClipAdded { clip });
        self.reply_when_synced(client, id, answer);
    }

    fn status(&self) -> Status {
        let selection = |which: Selection| {
            let slot = &self.slots[which as usize];
            SelectionStatus {
                owner: slot.owner.unwrap_or(Owner::None),
                types: slot.offered.clone(),
                kept: self.held_types(which),
                reading: slot.read.is_some(),
                sensitivity: slot.sensitivity,
            }
        };
        Status {
            daemon: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
            data_control: self.watcher.data_control().name().into(),
            persist: self.config.clipboard.persist,
            recording: self.recording(),
            clipboard: selection(Selection::Clipboard),
            primary: self
                .watched(Selection::Primary)
                .then(|| selection(Selection::Primary)),
        }
    }

    /// Apply the retention limits now.
    fn retain(&mut self) {
        let limits = history::limits(&self.config.history);
        // Without History there is nothing to retain.
        let _ = self.history.send(history::Job::Retain { limits });
    }

    /// Re-read `config.toml`. A broken file keeps the config in force.
    fn on_config_changed(&mut self) {
        let Some(watch) = &mut self.config_watch else {
            return;
        };
        if !matches!(watch.changed(), Ok(true)) {
            return;
        }
        if let Err(e) = self.reload() {
            eprintln!("hoarder: {e}");
        }
    }

    /// Re-read `config.toml` (the `reload` op, or a change noticed). A
    /// broken file keeps the config in force and says why.
    fn reload(&mut self) -> Result<(), String> {
        let path = hoarder_config::path();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
        };
        if let Err(d) = hoarder_config::parse(&text) {
            return Err(format!(
                "{}:{d}; keeping the previous config",
                path.display()
            ));
        }
        let config = hoarder_config::with_overrides(&text, &self.overrides)
            .map_err(|e| format!("--set {e}"))?;
        if config.history.encrypt != self.config.history.encrypt {
            let _ = self
                .history
                .send(history::Job::Encrypt(config.history.encrypt));
        }
        self.classifier = classifier(&config);
        self.config = config;
        self.retain();
        for which in SELECTIONS {
            if !self.watched(which) {
                self.stop_reading(which);
                self.slots[which as usize] = Slot::default();
            }
        }
        Ok(())
    }
}
