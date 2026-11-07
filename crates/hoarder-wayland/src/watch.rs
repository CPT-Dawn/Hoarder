//! The Daemon's view of the Selections: a non-blocking, event-driven
//! connection that reports every Selection change, reads Offers into pipes
//! and serves the Daemon's own Offer after a Takeover (PLAN.md §5.1).
//!
//! It speaks `ext-data-control-v1`, falling back to `wlr-data-control-unstable-v1`
//! (same shape) on compositors without the ext protocol. It also follows
//! keyboard focus through `wlr-foreign-toplevel-management`, so every change
//! carries the app that was focused when it happened.
//!
//! Nothing here blocks: poll [`Watcher::poll_fd`] in an event loop, call
//! [`Watcher::dispatch`] when it is readable and [`Watcher::flush`] before
//! sleeping again.

use std::collections::HashMap;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use wayland_client::backend::{ObjectId, WaylandError};
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_callback::{self, WlCallback},
    wl_registry::WlRegistry,
    wl_seat::WlSeat,
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
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1::{self, ZwlrDataControlDeviceV1},
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_offer_v1::{self, ZwlrDataControlOfferV1},
    zwlr_data_control_source_v1::{self, ZwlrDataControlSourceV1},
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

use crate::clipboard::{ClipboardError, Selection, hidden_globals};

const EXT_DATA_CONTROL: &str = "ext_data_control_manager_v1";
const WLR_DATA_CONTROL: &str = "zwlr_data_control_manager_v1";
const WLR_FOREIGN_TOPLEVEL: &str = "zwlr_foreign_toplevel_manager_v1";

type Result<T> = std::result::Result<T, ClipboardError>;

/// Which data-control protocol a [`Watcher`] ended up with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataControl {
    /// `ext-data-control-v1`.
    Ext,
    /// `wlr-data-control-unstable-v1`, the fallback.
    Wlr,
}

impl DataControl {
    pub fn name(self) -> &'static str {
        match self {
            DataControl::Ext => "ext",
            DataControl::Wlr => "wlr",
        }
    }
}

/// Something that happened on the Selections.
#[derive(Debug)]
pub enum WatchEvent {
    /// A Selection changed hands. `offer` is `None` once it is empty.
    Changed {
        which: Selection,
        offer: Option<Offer>,
    },
    /// A paste of `mime_type` from the Offer set by [`Watcher::serve`]:
    /// write the bytes to `fd` and close it.
    Send {
        which: Selection,
        mime_type: String,
        fd: OwnedFd,
    },
    /// The Offer set by [`Watcher::serve`] was replaced or cleared.
    Cancelled { which: Selection },
    /// Everything sent before [`Watcher::sync`] with this token is applied.
    Synced(u64),
}

/// One Offer on a Selection, as the source advertised it.
#[derive(Debug, Clone)]
pub struct Offer {
    proxy: OfferProxy,
    /// MIME types in the source's order.
    pub mime_types: Vec<String>,
    /// Whether this is the Daemon's own Offer (set by [`Watcher::serve`]).
    pub own: bool,
    /// The `app_id` focused when the Selection changed, if known.
    pub focused_app: Option<String>,
}

/// A data-control connection for the Daemon.
#[derive(Debug)]
pub struct Watcher {
    conn: Connection,
    queue: EventQueue<State>,
    qh: QueueHandle<State>,
    state: State,
    manager: Manager,
    device: Device,
}

impl Watcher {
    /// Connect to `$WAYLAND_DISPLAY` and watch the first seat.
    pub fn connect() -> Result<Self> {
        let conn =
            Connection::connect_to_env().map_err(|e| ClipboardError::NoWayland(e.to_string()))?;
        let (globals, queue) = registry_queue_init::<State>(&conn)
            .map_err(|e| ClipboardError::NoWayland(e.to_string()))?;
        let qh = queue.handle();
        let hidden = hidden_globals();
        let available = |name: &str| !hidden.contains(name) && advertises(&globals, name);

        let manager = if available(EXT_DATA_CONTROL) {
            Manager::Ext(bind(&globals, &qh, 1)?)
        } else if available(WLR_DATA_CONTROL) {
            Manager::Wlr(bind(&globals, &qh, 2)?)
        } else {
            return Err(ClipboardError::Unsupported(
                "this compositor offers neither ext-data-control-v1 nor \
                 wlr-data-control-unstable-v1, which the Daemon needs to watch the clipboard"
                    .into(),
            ));
        };
        let seat: WlSeat = bind(&globals, &qh, 2)
            .map_err(|_| ClipboardError::Failed("the compositor has no seat".into()))?;
        if available(WLR_FOREIGN_TOPLEVEL) {
            let _: ZwlrForeignToplevelManagerV1 = bind(&globals, &qh, 3)?;
        }
        let device = manager.get_data_device(&seat, &qh);
        let mut watcher = Watcher {
            conn,
            queue,
            qh,
            state: State::default(),
            manager,
            device,
        };
        // The device announces the current Selections right away; those
        // first events are returned by the first `dispatch`.
        watcher
            .queue
            .roundtrip(&mut watcher.state)
            .map_err(failed)?;
        Ok(watcher)
    }

    /// The data-control protocol in use.
    pub fn data_control(&self) -> DataControl {
        match self.manager {
            Manager::Ext(_) => DataControl::Ext,
            Manager::Wlr(_) => DataControl::Wlr,
        }
    }

    /// A duplicate of the connection's fd, to poll for readability.
    pub fn poll_fd(&self) -> std::io::Result<OwnedFd> {
        self.conn.backend().poll_fd().try_clone_to_owned()
    }

    /// Read whatever the compositor has sent, without blocking, and return
    /// what happened, in order.
    pub fn dispatch(&mut self) -> Result<Vec<WatchEvent>> {
        self.queue
            .dispatch_pending(&mut self.state)
            .map_err(failed)?;
        if let Some(guard) = self.queue.prepare_read() {
            match guard.read() {
                Ok(_) => {}
                Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(failed(e)),
            }
        }
        self.queue
            .dispatch_pending(&mut self.state)
            .map_err(failed)?;
        if self.state.finished {
            return Err(ClipboardError::Failed(
                "the compositor ended data control (the seat went away)".into(),
            ));
        }
        Ok(std::mem::take(&mut self.state.events))
    }

    /// Send buffered requests to the compositor.
    pub fn flush(&mut self) -> Result<()> {
        match self.conn.flush() {
            Ok(()) => Ok(()),
            Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(failed(e)),
        }
    }

    /// Ask the source of `offer` for `mime_type`. Returns the non-blocking
    /// read end of the pipe the data arrives on; it ends at EOF.
    pub fn receive(&mut self, offer: &Offer, mime_type: &str) -> Result<OwnedFd> {
        let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)
            .map_err(std::io::Error::from)?;
        // Only our end is non-blocking: `O_NONBLOCK` on both would make the
        // source's writes fail as soon as the pipe fills up.
        rustix::io::ioctl_fionbio(&read, true).map_err(std::io::Error::from)?;
        offer.proxy.receive(mime_type, write.as_fd());
        drop(write);
        self.flush()?;
        Ok(read)
    }

    /// Become the owner of `which`, offering `mime_types` in this order.
    /// Pastes arrive as [`WatchEvent::Send`]. Replaces any earlier Offer of ours.
    pub fn serve(&mut self, which: Selection, mime_types: &[String]) {
        let source = self.manager.create_data_source(&self.qh, which);
        for m in mime_types {
            source.offer(m);
        }
        self.device.set_selection(which, Some(&source));
        if let Some(old) = self.state.ours[which as usize].replace(source) {
            old.destroy();
        }
    }

    /// Leave `which` empty.
    pub fn clear(&mut self, which: Selection) {
        self.device.set_selection(which, None);
    }

    /// Ask for a [`WatchEvent::Synced`] once the compositor has applied
    /// every request sent so far.
    pub fn sync(&mut self, token: u64) {
        self.conn.display().sync(&self.qh, token);
    }
}

fn bind<I>(globals: &GlobalList, qh: &QueueHandle<State>, version: u32) -> Result<I>
where
    I: Proxy + 'static,
    State: Dispatch<I, ()>,
{
    globals.bind(qh, 1..=version, ()).map_err(|e| {
        ClipboardError::Unsupported(format!("cannot bind {}: {e}", I::interface().name))
    })
}

fn failed(e: impl std::fmt::Display) -> ClipboardError {
    ClipboardError::Failed(format!("lost the compositor: {e}"))
}

fn advertises(globals: &GlobalList, interface: &str) -> bool {
    globals
        .contents()
        .with_list(|list| list.iter().any(|g| g.interface == interface))
}

/// The two data-control protocols are the same shape; these enums let the
/// rest of the module ignore which one is in use.
#[derive(Debug)]
enum Manager {
    Ext(ExtDataControlManagerV1),
    Wlr(ZwlrDataControlManagerV1),
}

#[derive(Debug)]
enum Device {
    Ext(ExtDataControlDeviceV1),
    Wlr(ZwlrDataControlDeviceV1),
}

#[derive(Debug, Clone, PartialEq)]
enum OfferProxy {
    Ext(ExtDataControlOfferV1),
    Wlr(ZwlrDataControlOfferV1),
}

#[derive(Debug)]
enum Source {
    Ext(ExtDataControlSourceV1),
    Wlr(ZwlrDataControlSourceV1),
}

impl Manager {
    fn get_data_device(&self, seat: &WlSeat, qh: &QueueHandle<State>) -> Device {
        match self {
            Manager::Ext(m) => Device::Ext(m.get_data_device(seat, qh, ())),
            Manager::Wlr(m) => Device::Wlr(m.get_data_device(seat, qh, ())),
        }
    }

    fn create_data_source(&self, qh: &QueueHandle<State>, which: Selection) -> Source {
        match self {
            Manager::Ext(m) => Source::Ext(m.create_data_source(qh, which)),
            Manager::Wlr(m) => Source::Wlr(m.create_data_source(qh, which)),
        }
    }
}

impl Device {
    fn set_selection(&self, which: Selection, source: Option<&Source>) {
        match (self, which) {
            (Device::Ext(d), Selection::Clipboard) => d.set_selection(source.map(Source::ext)),
            (Device::Ext(d), Selection::Primary) => {
                d.set_primary_selection(source.map(Source::ext))
            }
            (Device::Wlr(d), Selection::Clipboard) => d.set_selection(source.map(Source::wlr)),
            (Device::Wlr(d), Selection::Primary) => {
                d.set_primary_selection(source.map(Source::wlr))
            }
        }
    }
}

impl OfferProxy {
    fn id(&self) -> ObjectId {
        match self {
            OfferProxy::Ext(o) => o.id(),
            OfferProxy::Wlr(o) => o.id(),
        }
    }

    fn receive(&self, mime_type: &str, fd: BorrowedFd<'_>) {
        match self {
            OfferProxy::Ext(o) => o.receive(mime_type.to_owned(), fd),
            OfferProxy::Wlr(o) => o.receive(mime_type.to_owned(), fd),
        }
    }

    fn destroy(&self) {
        match self {
            OfferProxy::Ext(o) => o.destroy(),
            OfferProxy::Wlr(o) => o.destroy(),
        }
    }
}

impl Source {
    fn id(&self) -> ObjectId {
        match self {
            Source::Ext(s) => s.id(),
            Source::Wlr(s) => s.id(),
        }
    }

    fn offer(&self, mime_type: &str) {
        match self {
            Source::Ext(s) => s.offer(mime_type.to_owned()),
            Source::Wlr(s) => s.offer(mime_type.to_owned()),
        }
    }

    fn destroy(&self) {
        match self {
            Source::Ext(s) => s.destroy(),
            Source::Wlr(s) => s.destroy(),
        }
    }

    fn ext(&self) -> &ExtDataControlSourceV1 {
        match self {
            Source::Ext(s) => s,
            Source::Wlr(_) => unreachable!("sources match the device's protocol"),
        }
    }

    fn wlr(&self) -> &ZwlrDataControlSourceV1 {
        match self {
            Source::Wlr(s) => s,
            Source::Ext(_) => unreachable!("sources match the device's protocol"),
        }
    }
}

/// A toplevel as wlr-foreign-toplevel describes it.
#[derive(Debug, Default)]
struct Toplevel {
    app_id: Option<String>,
    /// `activated` as of the latest `state` event, applied at `done`.
    pending_active: bool,
}

#[derive(Debug, Default)]
struct State {
    events: Vec<WatchEvent>,
    /// MIME types of each live offer, in the order the source gave them.
    mime_types: HashMap<ObjectId, Vec<String>>,
    /// The offer on each Selection, so it can be destroyed when replaced.
    current: [Option<OfferProxy>; 2],
    /// Our own source on each Selection, until it is cancelled.
    ours: [Option<Source>; 2],
    toplevels: HashMap<ObjectId, Toplevel>,
    /// The activated toplevel, as of its latest `done`.
    focused: Option<ObjectId>,
    finished: bool,
}

impl State {
    fn selection(&mut self, which: Selection, offer: Option<OfferProxy>) {
        let slot = which as usize;
        let old = std::mem::replace(&mut self.current[slot], offer.clone());
        if let Some(old) = old {
            let still_used = self.current.iter().any(|c| c.as_ref() == Some(&old));
            if !still_used {
                self.mime_types.remove(&old.id());
                old.destroy();
            }
        }
        let offer = offer.map(|proxy| Offer {
            mime_types: self
                .mime_types
                .get(&proxy.id())
                .cloned()
                .unwrap_or_default(),
            // A replaced source is cancelled before the new Selection is
            // announced, so while ours is alive the Selection is ours.
            own: self.ours[slot].is_some(),
            focused_app: self
                .focused
                .as_ref()
                .and_then(|id| self.toplevels.get(id))
                .and_then(|t| t.app_id.clone()),
            proxy,
        });
        self.events.push(WatchEvent::Changed { which, offer });
    }

    fn cancelled(&mut self, which: Selection, id: ObjectId) {
        let slot = &mut self.ours[which as usize];
        if slot.as_ref().is_some_and(|s| s.id() == id) {
            if let Some(source) = slot.take() {
                source.destroy();
            }
            self.events.push(WatchEvent::Cancelled { which });
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

delegate_noop!(State: ignore WlSeat);
delegate_noop!(State: ignore ExtDataControlManagerV1);
delegate_noop!(State: ignore ZwlrDataControlManagerV1);

impl Dispatch<WlCallback, u64> for State {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        token: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.events.push(WatchEvent::Synced(*token));
        }
    }
}

/// The device, offer and source handlers, once per data-control protocol.
macro_rules! data_control {
    ($variant:ident, $device:ty, $device_mod:ident, $offer:ty, $offer_mod:ident, $source:ty, $source_mod:ident) => {
        impl Dispatch<$device, ()> for State {
            fn event(
                state: &mut Self,
                _: &$device,
                event: $device_mod::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                use $device_mod::Event;
                match event {
                    Event::DataOffer { id } => {
                        state.mime_types.insert(id.id(), Vec::new());
                    }
                    Event::Selection { id } => {
                        state.selection(Selection::Clipboard, id.map(OfferProxy::$variant))
                    }
                    Event::PrimarySelection { id } => {
                        state.selection(Selection::Primary, id.map(OfferProxy::$variant))
                    }
                    Event::Finished => state.finished = true,
                    _ => {}
                }
            }

            event_created_child!(State, $device, [
                $device_mod::EVT_DATA_OFFER_OPCODE => ($offer, ()),
            ]);
        }

        impl Dispatch<$offer, ()> for State {
            fn event(
                state: &mut Self,
                offer: &$offer,
                event: $offer_mod::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                if let $offer_mod::Event::Offer { mime_type } = event {
                    state
                        .mime_types
                        .entry(offer.id())
                        .or_default()
                        .push(mime_type);
                }
            }
        }

        impl Dispatch<$source, Selection> for State {
            fn event(
                state: &mut Self,
                source: &$source,
                event: $source_mod::Event,
                which: &Selection,
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                match event {
                    $source_mod::Event::Send { mime_type, fd } => {
                        state.events.push(WatchEvent::Send {
                            which: *which,
                            mime_type,
                            fd,
                        });
                    }
                    $source_mod::Event::Cancelled => state.cancelled(*which, source.id()),
                    _ => {}
                }
            }
        }
    };
}

data_control!(
    Ext,
    ExtDataControlDeviceV1,
    ext_data_control_device_v1,
    ExtDataControlOfferV1,
    ext_data_control_offer_v1,
    ExtDataControlSourceV1,
    ext_data_control_source_v1
);
data_control!(
    Wlr,
    ZwlrDataControlDeviceV1,
    zwlr_data_control_device_v1,
    ZwlrDataControlOfferV1,
    zwlr_data_control_offer_v1,
    ZwlrDataControlSourceV1,
    zwlr_data_control_source_v1
);

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } = event {
            state.toplevels.insert(toplevel.id(), Toplevel::default());
        }
    }

    event_created_child!(State, ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        handle: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Event;
        let id = handle.id();
        match event {
            Event::AppId { app_id } => {
                state.toplevels.entry(id).or_default().app_id = Some(app_id);
            }
            Event::State { state: states } => {
                let activated = zwlr_foreign_toplevel_handle_v1::State::Activated as u32;
                state.toplevels.entry(id).or_default().pending_active = states
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|c| u32::from_ne_bytes(*c) == activated);
            }
            Event::Done => {
                if state
                    .toplevels
                    .entry(id.clone())
                    .or_default()
                    .pending_active
                {
                    state.focused = Some(id);
                } else if state.focused.as_ref() == Some(&id) {
                    state.focused = None;
                }
            }
            Event::Closed => {
                state.toplevels.remove(&id);
                if state.focused.as_ref() == Some(&id) {
                    state.focused = None;
                }
                handle.destroy();
            }
            _ => {}
        }
    }
}
