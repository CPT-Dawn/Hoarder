//! The helper Wayland client.
//!
//! Each [`Client`] is its own Wayland connection, driven by a worker thread
//! that keeps dispatching events (so Selections stay served) until the client
//! is told to [`exit`](Client::exit) or dropped. Closing the connection is
//! exactly what the compositor sees when an app quits.

use std::collections::HashMap;
use std::io::Write as _;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec};
use wayland_client::backend::ObjectId;
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_callback::WlCallback, wl_compositor::WlCompositor, wl_output::WlOutput,
    wl_registry::WlRegistry, wl_shm, wl_shm::WlShm, wl_shm_pool::WlShmPool, wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, ZwlrLayerSurfaceV1},
};

use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

use wayland_client::protocol::wl_seat::WlSeat;
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::{self, ExtDataControlDeviceV1},
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_offer_v1::ExtDataControlOfferV1,
    ext_data_control_source_v1::{self, ExtDataControlSourceV1},
};

mod field;
mod input;

pub use field::TextField;
pub use input::{Input, InputEvent};

use crate::frame::{FrameFormat, RawFrame};
use crate::{Error, Output, Pattern, PixelFormat, Result, Session};

/// How long any single helper operation may take before the test fails.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

type Job = Box<dyn FnOnce(&mut Worker) + Send>;

/// A scriptable Wayland client living inside a [`Session`].
#[derive(Debug)]
pub struct Client {
    jobs: Option<mpsc::Sender<Job>>,
    wake: OwnedFd,
    thread: Option<JoinHandle<()>>,
}

impl Session {
    /// Connect a new helper client to this session.
    pub fn client(&self) -> Result<Client> {
        Client::connect(&self.runtime_dir().join(self.wayland_display()))
    }
}

impl Client {
    fn connect(socket: &std::path::Path) -> Result<Self> {
        let stream = UnixStream::connect(socket)?;
        let conn = Connection::from_socket(stream).map_err(|e| Error::Wayland(e.to_string()))?;
        let (wake_rx, wake_tx) = rustix::pipe::pipe().map_err(std::io::Error::from)?;
        let (jobs_tx, jobs_rx) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();

        let thread = std::thread::Builder::new()
            .name("hoarder-e2e-client".into())
            .spawn(move || match Worker::new(conn) {
                Ok(mut worker) => {
                    let _ = ready_tx.send(Ok(()));
                    worker.run(jobs_rx, wake_rx);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })?;
        ready_rx
            .recv_timeout(CALL_TIMEOUT)
            .map_err(|_| Error::Wayland("helper client did not start".into()))??;
        Ok(Client {
            jobs: Some(jobs_tx),
            wake: wake_tx,
            thread: Some(thread),
        })
    }

    /// Run `f` on the worker thread and wait for its result.
    fn call<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Worker) -> Result<R> + Send + 'static,
    ) -> Result<R> {
        let (tx, rx) = mpsc::channel();
        let job: Job = Box::new(move |w| {
            let _ = tx.send(f(w));
        });
        self.jobs
            .as_ref()
            .expect("client already exited")
            .send(job)
            .map_err(|_| Error::Wayland("helper client thread is gone".into()))?;
        rustix::io::write(&self.wake, &[1]).map_err(std::io::Error::from)?;
        rx.recv_timeout(CALL_TIMEOUT)
            .map_err(|_| Error::Wayland("helper client call timed out".into()))?
    }

    /// Cover `output` with `pattern`, in 8-bit `XRGB8888`.
    ///
    /// Returns once the compositor has presented the frame, so a screenshot
    /// taken afterwards sees it. Painting the same output again replaces it.
    pub fn paint(&self, output: &Output, pattern: &Pattern) -> Result<()> {
        self.paint_as(output, pattern, PixelFormat::Xrgb8888)
    }

    /// Like [`paint`](Self::paint), but with an explicit buffer pixel format.
    ///
    /// The buffer is the output's physical size and is mapped 1:1 onto its
    /// physical pixels through `wp_viewporter`, whatever the scale or rotation.
    pub fn paint_as(&self, output: &Output, pattern: &Pattern, format: PixelFormat) -> Result<()> {
        let output = output.clone();
        let pattern = pattern.clone();
        self.call(move |w| w.paint(&output, &pattern, format))
    }

    /// Read back `output`'s buffer exactly as the compositor holds it
    /// (`wlr-screencopy`, no cursor): native format and depth, physical size,
    /// output transform *not* undone. Use [`Session::screenshot`] for what the
    /// user sees.
    pub fn capture(&self, output: &Output) -> Result<RawFrame> {
        let output = output.clone();
        self.call(move |w| w.capture(&output))
    }

    /// Become the owner of a Selection, offering `offer` as `(MIME type, bytes)`
    /// pairs in that order, and serve it until the client exits or someone
    /// else takes the Selection over.
    ///
    /// Uses `ext-data-control-v1`, so it needs no keyboard focus. Returns once
    /// the compositor has processed the request.
    pub fn set_selection(&self, selection: Selection, offer: &[(&str, &[u8])]) -> Result<()> {
        let offer: Offer = offer
            .iter()
            .map(|(m, d)| (m.to_string(), Arc::new(Reply::Now(d.to_vec()))))
            .collect();
        self.call(move |w| w.set_selection(selection, offer))
    }

    /// Copy again the way Chromium and GLFW apps do: destroy the source this
    /// client owns, then set a new one, in the same flush. The compositor
    /// sees the Selection go empty for an instant in between.
    pub fn recopy(&self, selection: Selection, offer: &[(&str, &[u8])]) -> Result<()> {
        let offer: Offer = offer
            .iter()
            .map(|(m, d)| (m.to_string(), Arc::new(Reply::Now(d.to_vec()))))
            .collect();
        self.call(move |w| {
            if let Some(old) = w.state.owned[selection as usize].take() {
                w.state.sources.remove(&old.id());
                old.destroy();
            }
            w.set_selection(selection, offer)
        })
    }

    /// Like [`set_selection`](Self::set_selection), but each type answers a
    /// paste the way its [`Reply`] says: at once, late, or never. Use it to
    /// play a slow or stuck source app.
    pub fn set_selection_replying(
        &self,
        selection: Selection,
        offer: Vec<(&str, Reply)>,
    ) -> Result<()> {
        let offer: Offer = offer
            .into_iter()
            .map(|(m, r)| (m.to_string(), Arc::new(r)))
            .collect();
        self.call(move |w| w.set_selection(selection, offer))
    }

    /// Whether this client still owns `selection`: true from
    /// [`set_selection`](Self::set_selection) until the compositor cancels the
    /// source because someone else set the Selection (or cleared it).
    pub fn owns_selection(&self, selection: Selection) -> Result<bool> {
        self.call(move |w| {
            w.roundtrip()?;
            Ok(w.state.owned[selection as usize].is_some())
        })
    }

    /// Every MIME type pasted from this client's Selections so far, in
    /// order: who read what the app offered, and how often.
    pub fn pasted(&self) -> Result<Vec<String>> {
        self.call(|w| {
            w.roundtrip()?;
            Ok(w.state.pasted.clone())
        })
    }

    /// Disconnect from the compositor, as if the app had quit.
    ///
    /// Everything the client owned (surfaces, Selections) goes away with it.
    pub fn exit(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        drop(self.jobs.take());
        let _ = rustix::io::write(&self.wake, &[1]);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The representations a data source serves: `(MIME type, reply)` in order.
type Offer = Vec<(String, Arc<Reply>)>;

/// How a helper's data source answers a paste of one MIME type.
#[derive(Debug, Clone)]
pub enum Reply {
    /// Write these bytes straight away.
    Now(Vec<u8>),
    /// Wait this long, then write the bytes.
    After(Duration, Vec<u8>),
    /// Keep the pipe open and write nothing until the client exits.
    Never,
}

/// One of the two Wayland Selections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    /// Ctrl+C / Ctrl+V.
    Clipboard,
    /// Highlight / middle-click.
    Primary,
}

/// A surface the client keeps alive on an output.
struct Painted {
    surface: WlSurface,
    layer: ZwlrLayerSurfaceV1,
    viewport: WpViewport,
}

impl Painted {
    fn destroy(self) {
        self.viewport.destroy();
        self.layer.destroy();
        self.surface.destroy();
    }
}

/// Event-side state, filled in by the `Dispatch` impls.
#[derive(Default)]
pub(crate) struct State {
    output_names: HashMap<ObjectId, String>,
    shm_formats: Vec<wl_shm::Format>,
    layer_configured: HashMap<ObjectId, (u32, u32)>,
    layer_closed: Vec<ObjectId>,
    callbacks_done: Vec<ObjectId>,
    copies: HashMap<ObjectId, Copy>,
    /// What each live data source offers, by MIME type.
    sources: HashMap<ObjectId, Offer>,
    /// The source currently set on each Selection, until it is cancelled.
    owned: [Option<ExtDataControlSourceV1>; 2],
    /// Pipes of [`Reply::Never`] pastes, held open until the client exits.
    stalled: Vec<OwnedFd>,
    /// Every MIME type anyone has pasted from this client, in order.
    pasted: Vec<String>,
    input: input::InputState,
    fields: field::FieldState,
}

/// Progress of one screencopy.
#[derive(Default)]
struct Copy {
    /// `(format, width, height, stride)` of the shm buffer the compositor wants.
    shm: Option<(u32, u32, u32, u32)>,
    buffer_done: bool,
    y_invert: bool,
    ready: bool,
    failed: bool,
}

/// The worker-thread side of a [`Client`].
pub(crate) struct Worker {
    conn: Connection,
    queue: EventQueue<State>,
    qh: QueueHandle<State>,
    globals: GlobalList,
    state: State,
    shm: WlShm,
    outputs: Vec<WlOutput>,
    painted: HashMap<String, Painted>,
    data_device: Option<ExtDataControlDeviceV1>,
    seat: WlSeat,
    windows: Vec<input::Window>,
    devices: Option<input::Devices>,
    /// The `wl_data_device` text fields paste through.
    text_device: Option<wayland_client::protocol::wl_data_device::WlDataDevice>,
}

impl Worker {
    fn new(conn: Connection) -> Result<Self> {
        let (globals, queue) =
            registry_queue_init::<State>(&conn).map_err(|e| Error::Wayland(e.to_string()))?;
        let qh = queue.handle();
        let outputs = globals.contents().with_list(|list| {
            list.iter()
                .filter(|g| g.interface == WlOutput::interface().name)
                .map(|g| {
                    globals
                        .registry()
                        .bind::<WlOutput, _, _>(g.name, g.version.min(4), &qh, ())
                })
                .collect()
        });
        let shm: WlShm = Self::bind_from(&globals, &qh, 1)?;
        let seat: WlSeat = Self::bind_from(&globals, &qh, 7)?;
        let mut worker = Worker {
            conn,
            queue,
            qh,
            globals,
            state: State::default(),
            shm,
            outputs,
            painted: HashMap::new(),
            data_device: None,
            seat,
            windows: Vec::new(),
            devices: None,
            text_device: None,
        };
        // First roundtrip: output names, shm formats, seat capabilities.
        // Second: the compositor has seen our wl_pointer / wl_keyboard.
        worker.roundtrip()?;
        worker.roundtrip()?;
        Ok(worker)
    }

    fn bind_from<I>(globals: &GlobalList, qh: &QueueHandle<State>, version: u32) -> Result<I>
    where
        I: Proxy + 'static,
        State: Dispatch<I, ()>,
    {
        globals
            .bind::<I, State, ()>(qh, 1..=version, ())
            .map_err(|e| Error::Wayland(format!("{} is not available: {e}", I::interface().name)))
    }

    fn bind<I>(&self, version: u32) -> Result<I>
    where
        I: Proxy + 'static,
        State: Dispatch<I, ()>,
    {
        Self::bind_from(&self.globals, &self.qh, version)
    }

    fn run(&mut self, jobs: mpsc::Receiver<Job>, wake: OwnedFd) {
        loop {
            loop {
                match jobs.try_recv() {
                    Ok(job) => job(self),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => return,
                }
            }
            match self.dispatch_once(Some(&wake), None) {
                Ok(woken) => {
                    if woken {
                        let mut buf = [0u8; 64];
                        let _ = rustix::io::read(&wake, &mut buf);
                    }
                }
                Err(_) => return,
            }
        }
    }

    /// Flush, wait for Wayland events (or `wake`, or the timeout) and dispatch them.
    /// Returns whether `wake` became readable.
    fn dispatch_once(&mut self, wake: Option<&OwnedFd>, timeout: Option<Duration>) -> Result<bool> {
        let werr = |e: &dyn std::fmt::Display| Error::Wayland(e.to_string());
        self.queue
            .dispatch_pending(&mut self.state)
            .map_err(|e| werr(&e))?;
        self.conn.flush().map_err(|e| werr(&e))?;
        let Some(guard) = self.queue.prepare_read() else {
            return Ok(false);
        };
        let ts = timeout.map(|t| Timespec {
            tv_sec: t.as_secs() as _,
            tv_nsec: t.subsec_nanos() as _,
        });
        let (wl_ready, wake_ready) = {
            let wl_fd = guard.connection_fd();
            let mut fds = vec![PollFd::new(&wl_fd, PollFlags::IN)];
            if let Some(w) = wake {
                fds.push(PollFd::new(w, PollFlags::IN));
            }
            match rustix::event::poll(&mut fds, ts.as_ref()) {
                Ok(_) | Err(rustix::io::Errno::INTR) => {}
                Err(e) => return Err(Error::Io(e.into())),
            }
            (
                !fds[0].revents().is_empty(),
                fds.get(1).is_some_and(|f| !f.revents().is_empty()),
            )
        };
        if wl_ready {
            guard.read().map_err(|e| werr(&e))?;
            self.queue
                .dispatch_pending(&mut self.state)
                .map_err(|e| werr(&e))?;
        }
        Ok(wake_ready)
    }

    /// Dispatch until `done` holds, or fail after [`CALL_TIMEOUT`].
    fn wait_until(&mut self, what: &str, mut done: impl FnMut(&State) -> bool) -> Result<()> {
        let deadline = Instant::now() + CALL_TIMEOUT;
        while !done(&self.state) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Wayland(format!("timed out waiting for {what}")));
            }
            self.dispatch_once(None, Some(left.min(Duration::from_millis(100))))?;
        }
        Ok(())
    }

    fn roundtrip(&mut self) -> Result<()> {
        self.queue
            .roundtrip(&mut self.state)
            .map(|_| ())
            .map_err(|e| Error::Wayland(e.to_string()))
    }

    fn wl_output(&self, name: &str) -> Result<WlOutput> {
        self.outputs
            .iter()
            .find(|o| {
                self.state
                    .output_names
                    .get(&o.id())
                    .is_some_and(|n| n == name)
            })
            .cloned()
            .ok_or_else(|| Error::Command(format!("no wl_output named {name}")))
    }

    /// Make an SHM buffer holding `data` (`w`×`h`, stride `4 * w`).
    fn shm_buffer(
        &mut self,
        data: &[u8],
        w: u32,
        h: u32,
        format: wl_shm::Format,
    ) -> Result<WlBuffer> {
        if !self.state.shm_formats.contains(&format) {
            return Err(Error::Command(format!(
                "compositor does not offer wl_shm {format:?} (has {:?})",
                self.state.shm_formats
            )));
        }
        let fd = rustix::fs::memfd_create("hoarder-e2e", rustix::fs::MemfdFlags::CLOEXEC)
            .map_err(std::io::Error::from)?;
        std::fs::File::from(fd.try_clone()?).write_all(data)?;
        let pool = self
            .shm
            .create_pool(fd.as_fd(), data.len() as i32, &self.qh, ());
        let buffer =
            pool.create_buffer(0, w as i32, h as i32, (w * 4) as i32, format, &self.qh, ());
        pool.destroy();
        Ok(buffer)
    }

    fn set_selection(&mut self, selection: Selection, offer: Offer) -> Result<()> {
        let manager: ExtDataControlManagerV1 = self.bind(1)?;
        if self.data_device.is_none() {
            self.data_device = Some(manager.get_data_device(&self.seat, &self.qh, ()));
        }
        let source = manager.create_data_source(&self.qh, ());
        for (mime, _) in &offer {
            source.offer(mime.clone());
        }
        self.state.sources.insert(source.id(), offer);
        let device = self.data_device.as_ref().expect("created above");
        match selection {
            Selection::Clipboard => device.set_selection(Some(&source)),
            Selection::Primary => device.set_primary_selection(Some(&source)),
        }
        self.state.owned[selection as usize] = Some(source);
        self.roundtrip()
    }

    /// Read back `output`'s current buffer through `zwlr_screencopy_manager_v1`.
    fn capture(&mut self, output: &Output) -> Result<RawFrame> {
        let wl_output = self.wl_output(&output.name)?;
        let manager: ZwlrScreencopyManagerV1 = self.bind(3)?;
        let frame = manager.capture_output(0, &wl_output, &self.qh, ());
        let id = frame.id();
        self.state.copies.insert(id.clone(), Copy::default());
        self.wait_until("screencopy buffer_done", |s| s.copies[&id].buffer_done)?;

        let Some((code, width, height, stride)) = self.state.copies[&id].shm else {
            return Err(Error::Wayland("screencopy offered no shm buffer".into()));
        };
        let format = FrameFormat::from_shm(code)
            .ok_or_else(|| Error::Wayland(format!("unexpected screencopy format {code:#x}")))?;
        let size = (stride * height) as usize;
        let fd = rustix::fs::memfd_create("hoarder-e2e-copy", rustix::fs::MemfdFlags::CLOEXEC)
            .map_err(std::io::Error::from)?;
        let file = std::fs::File::from(fd);
        file.set_len(size as u64)?;
        let pool = self
            .shm
            .create_pool(file.as_fd(), size as i32, &self.qh, ());
        let wl_format = wl_shm::Format::try_from(code).expect("validated above");
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride as i32,
            wl_format,
            &self.qh,
            (),
        );
        pool.destroy();
        frame.copy(&buffer);
        self.wait_until("screencopy ready", |s| {
            s.copies[&id].ready || s.copies[&id].failed
        })?;
        let copy = self.state.copies.remove(&id).expect("inserted above");
        frame.destroy();
        buffer.destroy();
        if copy.failed {
            return Err(Error::Wayland("screencopy failed".into()));
        }

        let mut data = vec![0u8; size];
        std::os::unix::fs::FileExt::read_exact_at(&file, &mut data, 0)?;
        Ok(RawFrame {
            format,
            width,
            height,
            stride,
            y_invert: copy.y_invert,
            data,
        })
    }

    /// Commit `surface` with a frame callback and wait until it is presented.
    fn commit_and_wait_for_frame(&mut self, surface: &WlSurface) -> Result<()> {
        let cb = surface.frame(&self.qh, ());
        surface.commit();
        let id = cb.id();
        self.wait_until("frame callback", |s| s.callbacks_done.contains(&id))?;
        self.state.callbacks_done.retain(|d| *d != id);
        Ok(())
    }

    fn paint(&mut self, output: &Output, pattern: &Pattern, format: PixelFormat) -> Result<()> {
        let wl_output = self.wl_output(&output.name)?;
        if let Some(old) = self.painted.remove(&output.name) {
            old.destroy();
        }

        let compositor: WlCompositor = self.bind(4)?;
        let layer_shell: ZwlrLayerShellV1 = self.bind(1)?;
        let viewporter: WpViewporter = self.bind(1)?;

        let surface = compositor.create_surface(&self.qh, ());
        let layer = layer_shell.get_layer_surface(
            &surface,
            Some(&wl_output),
            Layer::Background,
            "hoarder-e2e".into(),
            &self.qh,
            (),
        );
        layer.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
        layer.set_exclusive_zone(-1);
        layer.set_size(0, 0);
        surface.commit();
        let layer_id = layer.id();
        self.wait_until("layer surface configure", |s| {
            s.layer_configured.contains_key(&layer_id)
        })?;

        let (lw, lh) = self.state.layer_configured[&layer_id];
        let (pw, ph) = output.spec.physical_size();
        let wl_format = match format {
            PixelFormat::Xrgb8888 => wl_shm::Format::Xrgb8888,
            PixelFormat::Xrgb2101010 => wl_shm::Format::Xrgb2101010,
        };
        let buffer = self.shm_buffer(&format.render(pattern, pw, ph), pw, ph, wl_format)?;
        let viewport = viewporter.get_viewport(&surface, &self.qh, ());
        viewport.set_destination(lw as i32, lh as i32);
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, pw as i32, ph as i32);
        self.commit_and_wait_for_frame(&surface)?;

        self.painted.insert(
            output.name.clone(),
            Painted {
                surface,
                layer,
                viewport,
            },
        );
        Ok(())
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        output: &WlOutput,
        event: wayland_client::protocol::wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wayland_client::protocol::wl_output::Event::Name { name } = event {
            state.output_names.insert(output.id(), name);
        }
    }
}

impl Dispatch<WlShm, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlShm,
        event: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_shm::Event::Format {
            format: wayland_client::WEnum::Value(f),
        } = event
            && !state.shm_formats.contains(&f)
        {
            state.shm_formats.push(f);
        }
    }
}

impl Dispatch<WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        cb: &WlCallback,
        _: wayland_client::protocol::wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.callbacks_done.push(cb.id());
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        state: &mut Self,
        layer: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                layer.ack_configure(serial);
                state.layer_configured.insert(layer.id(), (width, height));
            }
            zwlr_layer_surface_v1::Event::Closed => state.layer_closed.push(layer.id()),
            _ => {}
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for State {
    fn event(
        state: &mut Self,
        frame: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_screencopy_frame_v1::Event as E;
        let Some(copy) = state.copies.get_mut(&frame.id()) else {
            return;
        };
        match event {
            E::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                let code = match format {
                    wayland_client::WEnum::Value(f) => f as u32,
                    wayland_client::WEnum::Unknown(c) => c,
                };
                copy.shm = Some((code, width, height, stride));
            }
            E::BufferDone => copy.buffer_done = true,
            E::Flags { flags } => {
                copy.y_invert = matches!(flags, wayland_client::WEnum::Value(f)
                    if f.contains(zwlr_screencopy_frame_v1::Flags::YInvert));
            }
            E::Ready { .. } => copy.ready = true,
            E::Failed => copy.failed = true,
            _ => {}
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
                state.pasted.push(mime_type.clone());
                let data = state.sources.get(&source.id()).and_then(|offer| {
                    offer
                        .iter()
                        .find(|(m, _)| *m == mime_type)
                        .map(|(_, d)| d.clone())
                });
                match data {
                    Some(reply) if matches!(*reply, Reply::Never) => state.stalled.push(fd),
                    // Write off-thread so a slow reader never stalls the event loop.
                    Some(reply) => {
                        std::thread::spawn(move || {
                            let bytes = match &*reply {
                                Reply::Now(b) => b,
                                Reply::After(delay, b) => {
                                    std::thread::sleep(*delay);
                                    b
                                }
                                Reply::Never => return,
                            };
                            let _ = std::fs::File::from(fd).write_all(bytes);
                        });
                    }
                    None => {}
                }
            }
            ext_data_control_source_v1::Event::Cancelled => {
                for owned in &mut state.owned {
                    if owned.as_ref() == Some(source) {
                        *owned = None;
                    }
                }
                state.sources.remove(&source.id());
                source.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtDataControlDeviceV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ExtDataControlDeviceV1,
        event: ext_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Other clients' offers are none of our business; drop them.
        if let ext_data_control_device_v1::Event::DataOffer { id } = event {
            id.destroy();
        }
    }

    wayland_client::event_created_child!(State, ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, ()),
    ]);
}

delegate_noop!(State: ignore ExtDataControlOfferV1);
delegate_noop!(State: ignore ExtDataControlManagerV1);
delegate_noop!(State: ignore ZwlrScreencopyManagerV1);
delegate_noop!(State: ignore WlCompositor);
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WlShmPool);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: ignore ZwlrLayerShellV1);
delegate_noop!(State: ignore WpViewporter);
delegate_noop!(State: ignore WpViewport);
