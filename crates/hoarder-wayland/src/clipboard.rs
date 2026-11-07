//! The clipboard and primary Selections over `ext-data-control-v1`: read what
//! is offered, serve our own Offer, clear, and follow changes (PLAN.md §5).
//!
//! Data control needs no focus and no surface, so this works from a terminal,
//! a script or a background process alike.

use std::collections::{HashMap, HashSet};
use std::io::{Read as _, Write as _};
use std::os::fd::{AsFd, OwnedFd};
use std::sync::Arc;

use wayland_client::backend::ObjectId;
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_registry::WlRegistry,
    wl_seat::{self, WlSeat},
};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop, event_created_child,
};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::{self, ExtDataControlDeviceV1},
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_offer_v1::{self, ExtDataControlOfferV1},
    ext_data_control_source_v1::{self, ExtDataControlSourceV1},
};

use crate::capture::HIDE_GLOBALS_ENV;

const EXT_DATA_CONTROL: &str = "ext_data_control_manager_v1";

/// Which Selection to work with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    /// The regular clipboard (Ctrl+C / Ctrl+V).
    Clipboard,
    /// The primary selection (select / middle-click).
    Primary,
}

/// Everything that can go wrong talking to the clipboard.
#[derive(Debug)]
pub enum ClipboardError {
    /// No Wayland compositor to talk to.
    NoWayland(String),
    /// The compositor lacks a protocol Hoarder needs (exit code 3).
    Unsupported(String),
    /// The compositor broke the conversation, or the seat went away.
    Failed(String),
    Io(std::io::Error),
}

impl std::fmt::Display for ClipboardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClipboardError::NoWayland(m) => {
                write!(f, "cannot connect to a Wayland compositor: {m}")
            }
            ClipboardError::Unsupported(m) | ClipboardError::Failed(m) => f.write_str(m),
            ClipboardError::Io(e) => write!(f, "clipboard transfer failed: {e}"),
        }
    }
}

impl std::error::Error for ClipboardError {}

impl From<std::io::Error> for ClipboardError {
    fn from(e: std::io::Error) -> Self {
        ClipboardError::Io(e)
    }
}

type Result<T> = std::result::Result<T, ClipboardError>;

/// One representation of an Offer we serve: a MIME type and its bytes.
/// Several types may share the same bytes.
#[derive(Debug, Clone)]
pub struct Representation {
    pub mime_type: String,
    pub data: Arc<[u8]>,
}

/// How long [`Clipboard::serve`] keeps going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Serve {
    /// Until another client takes the Selection.
    UntilReplaced,
    /// Exactly one paste, then give the Selection up.
    Once,
}

/// A connection to the compositor's data-control, bound to one seat.
#[derive(Debug)]
pub struct Clipboard {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
    manager: ExtDataControlManagerV1,
    device: ExtDataControlDeviceV1,
}

impl Clipboard {
    /// Connect to `$WAYLAND_DISPLAY` and attach to `seat` (the first seat if `None`).
    pub fn connect(seat: Option<&str>) -> Result<Self> {
        let conn =
            Connection::connect_to_env().map_err(|e| ClipboardError::NoWayland(e.to_string()))?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn)
            .map_err(|e| ClipboardError::NoWayland(e.to_string()))?;
        let qh = queue.handle();
        let mut state = State::default();

        if hidden_globals().contains(EXT_DATA_CONTROL) || !advertises(&globals, EXT_DATA_CONTROL) {
            return Err(ClipboardError::Unsupported(
                "this compositor does not offer ext-data-control-v1, \
                 which Hoarder needs to use the clipboard"
                    .into(),
            ));
        }
        let manager: ExtDataControlManagerV1 = globals
            .bind(&qh, 1..=1, ())
            .map_err(|e| ClipboardError::Unsupported(format!("cannot bind data control: {e}")))?;

        let seats: Vec<(u32, u32)> = globals.contents().with_list(|list| {
            list.iter()
                .filter(|g| g.interface == "wl_seat")
                .map(|g| (g.name, g.version))
                .collect()
        });
        for (i, (name, version)) in seats.iter().enumerate() {
            let wl: WlSeat = globals.registry().bind(*name, (*version).min(2), &qh, i);
            state.seats.push((wl, None));
        }
        queue.roundtrip(&mut state).map_err(failed)?;
        let wl_seat = match seat {
            None => state.seats.first().map(|(s, _)| s.clone()),
            Some(want) => state
                .seats
                .iter()
                .find(|(_, name)| name.as_deref() == Some(want))
                .map(|(s, _)| s.clone()),
        }
        .ok_or_else(|| {
            ClipboardError::Failed(match seat {
                Some(want) => format!("cannot find seat {want}"),
                None => "the compositor has no seat".into(),
            })
        })?;

        let device = manager.get_data_device(&wl_seat, &qh, ());
        // The device announces the current Selections right away.
        queue.roundtrip(&mut state).map_err(failed)?;
        Ok(Clipboard {
            conn,
            queue,
            state,
            manager,
            device,
        })
    }

    /// The MIME types currently offered on `which`, in the source's order,
    /// or `None` if nothing is copied.
    pub fn offered(&self, which: Selection) -> Option<&[String]> {
        let offer = self.state.current(which)?;
        Some(
            self.state
                .mime_types
                .get(&offer.id())
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        )
    }

    /// Block until `which` changes, then return its new MIME types
    /// (`None` once it is cleared).
    pub fn next_change(&mut self, which: Selection) -> Result<Option<Vec<String>>> {
        let seen = self.state.generation(which);
        while self.state.generation(which) == seen {
            self.queue
                .blocking_dispatch(&mut self.state)
                .map_err(failed)?;
            self.state.check_finished()?;
        }
        Ok(self.offered(which).map(<[String]>::to_vec))
    }

    /// Read the current Offer on `which` as `mime_type`, to the end.
    pub fn read(&mut self, which: Selection, mime_type: &str) -> Result<Vec<u8>> {
        let offer = self
            .state
            .current(which)
            .ok_or_else(|| ClipboardError::Failed("nothing is copied".into()))?
            .clone();
        let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)
            .map_err(std::io::Error::from)?;
        offer.receive(mime_type.to_owned(), write.as_fd());
        drop(write);
        self.conn
            .flush()
            .map_err(|e| ClipboardError::Failed(e.to_string()))?;
        let mut data = Vec::new();
        std::fs::File::from(read).read_to_end(&mut data)?;
        Ok(data)
    }

    /// Give up `which`, leaving nothing copied.
    pub fn clear(&mut self, which: Selection) -> Result<()> {
        match which {
            Selection::Clipboard => self.device.set_selection(None),
            Selection::Primary => self.device.set_primary_selection(None),
        }
        self.queue.roundtrip(&mut self.state).map_err(failed)?;
        Ok(())
    }

    /// Own `which` with `representations` and answer pastes until `serve`
    /// says stop. `ready` runs once the compositor has accepted the Offer,
    /// so anything started after it sees the new Selection.
    pub fn serve(
        mut self,
        which: Selection,
        representations: Vec<Representation>,
        serve: Serve,
        ready: impl FnOnce(),
    ) -> Result<()> {
        let qh = self.queue.handle();
        let source = self.manager.create_data_source(&qh, ());
        for r in &representations {
            source.offer(r.mime_type.clone());
        }
        match which {
            Selection::Clipboard => self.device.set_selection(Some(&source)),
            Selection::Primary => self.device.set_primary_selection(Some(&source)),
        }
        self.queue.roundtrip(&mut self.state).map_err(failed)?;
        ready();

        let mut writers = Vec::new();
        loop {
            for (mime_type, fd) in std::mem::take(&mut self.state.sends) {
                let data = representations
                    .iter()
                    .find(|r| r.mime_type == mime_type)
                    .map(|r| Arc::clone(&r.data));
                // Write off-thread so a slow reader never blocks other pastes.
                writers.push(std::thread::spawn(move || {
                    if let Some(data) = data {
                        // A reader that hangs up early is its own business.
                        let _ = std::fs::File::from(fd).write_all(&data);
                    }
                }));
                if serve == Serve::Once {
                    for w in writers {
                        let _ = w.join();
                    }
                    return Ok(());
                }
            }
            if self.state.cancelled.contains(&source.id()) {
                break;
            }
            self.queue
                .blocking_dispatch(&mut self.state)
                .map_err(failed)?;
            self.state.check_finished()?;
        }
        source.destroy();
        for w in writers {
            let _ = w.join();
        }
        Ok(())
    }
}

fn failed(e: impl std::fmt::Display) -> ClipboardError {
    ClipboardError::Failed(format!("lost the compositor: {e}"))
}

fn advertises(globals: &GlobalList, interface: &str) -> bool {
    globals
        .contents()
        .with_list(|list| list.iter().any(|g| g.interface == interface))
}

fn hidden_globals() -> HashSet<String> {
    std::env::var(HIDE_GLOBALS_ENV)
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

#[derive(Debug, Default)]
struct State {
    seats: Vec<(WlSeat, Option<String>)>,
    mime_types: HashMap<ObjectId, Vec<String>>,
    clipboard: Option<ExtDataControlOfferV1>,
    primary: Option<ExtDataControlOfferV1>,
    /// Bumped on every Selection event, so a watcher sees each change.
    generations: [u64; 2],
    sends: Vec<(String, OwnedFd)>,
    cancelled: HashSet<ObjectId>,
    finished: bool,
}

impl State {
    fn current(&self, which: Selection) -> Option<&ExtDataControlOfferV1> {
        match which {
            Selection::Clipboard => self.clipboard.as_ref(),
            Selection::Primary => self.primary.as_ref(),
        }
    }

    fn generation(&self, which: Selection) -> u64 {
        self.generations[which as usize]
    }

    fn set(&mut self, which: Selection, offer: Option<ExtDataControlOfferV1>) {
        let slot = match which {
            Selection::Clipboard => &mut self.clipboard,
            Selection::Primary => &mut self.primary,
        };
        let old = std::mem::replace(slot, offer);
        if let Some(old) = old {
            let still_used =
                self.clipboard.as_ref() == Some(&old) || self.primary.as_ref() == Some(&old);
            if !still_used {
                self.mime_types.remove(&old.id());
                old.destroy();
            }
        }
        self.generations[which as usize] += 1;
    }

    fn check_finished(&self) -> Result<()> {
        if self.finished {
            Err(ClipboardError::Failed(
                "the seat went away while using the clipboard".into(),
            ))
        } else {
            Ok(())
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, usize> for State {
    fn event(
        state: &mut Self,
        _: &WlSeat,
        event: wl_seat::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Name { name } = event {
            state.seats[*index].1 = Some(name);
        }
    }
}

delegate_noop!(State: ignore ExtDataControlManagerV1);

impl Dispatch<ExtDataControlDeviceV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtDataControlDeviceV1,
        event: ext_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_data_control_device_v1::Event;
        match event {
            Event::DataOffer { id } => {
                state.mime_types.insert(id.id(), Vec::new());
            }
            Event::Selection { id } => state.set(Selection::Clipboard, id),
            Event::PrimarySelection { id } => state.set(Selection::Primary, id),
            Event::Finished => state.finished = true,
            _ => {}
        }
    }

    event_created_child!(State, ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ExtDataControlOfferV1, ()> for State {
    fn event(
        state: &mut Self,
        offer: &ExtDataControlOfferV1,
        event: ext_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_data_control_offer_v1::Event::Offer { mime_type } = event {
            state
                .mime_types
                .entry(offer.id())
                .or_default()
                .push(mime_type);
        }
    }
}

impl Dispatch<ExtDataControlSourceV1, ()> for State {
    fn event(
        state: &mut Self,
        source: &ExtDataControlSourceV1,
        event: ext_data_control_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_source_v1::Event::Send { mime_type, fd } => {
                state.sends.push((mime_type, fd));
            }
            ext_data_control_source_v1::Event::Cancelled => {
                state.cancelled.insert(source.id());
            }
            _ => {}
        }
    }
}
