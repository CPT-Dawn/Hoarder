//! Freezing: one copy of every output, at its native buffer size and format,
//! taken before anything else happens (ADR-0003).
//!
//! The best protocol the compositor offers is used: `ext-image-copy-capture-v1`
//! with the output capture source, falling back to `wlr-screencopy-unstable-v1`
//! (ADR-0002). All outputs are captured in parallel. Nothing here touches the
//! pixels: each [`FrozenFrame`] is the buffer exactly as the compositor wrote
//! it, with the transform and y-invert needed to show it upright.

use std::collections::HashSet;
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;

use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer::WlBuffer,
    wl_output::{self, WlOutput},
    wl_registry::WlRegistry,
    wl_shm::{self, WlShm},
    wl_shm_pool::WlShmPool,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1},
    ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1, Options},
    ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
};
use wayland_protocols::xdg::xdg_output::zv1::client::{
    zxdg_output_manager_v1::ZxdgOutputManagerV1,
    zxdg_output_v1::{self, ZxdgOutputV1},
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

/// Comma-separated global interface names Hoarder pretends the compositor
/// does not advertise. Lets tests (and bug reports) force the screencopy
/// fallback or simulate a compositor without capture protocols.
pub const HIDE_GLOBALS_ENV: &str = "HOARDER_HIDE_GLOBALS";

const EXT_CAPTURE: &str = "ext_image_copy_capture_manager_v1";
const EXT_OUTPUT_SOURCE: &str = "ext_output_image_capture_source_manager_v1";
const WLR_SCREENCOPY: &str = "zwlr_screencopy_manager_v1";

/// Which protocol took the Frozen Frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    ExtImageCopyCapture,
    WlrScreencopy,
}

/// Everything that can stop a freeze.
#[derive(Debug)]
pub enum CaptureError {
    /// No Wayland compositor to talk to.
    NoWayland(String),
    /// The compositor lacks a protocol Hoarder needs (exit code 3).
    Unsupported(String),
    /// The compositor refused or botched a capture.
    Failed(String),
    Io(std::io::Error),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureError::NoWayland(m) => write!(f, "cannot connect to a Wayland compositor: {m}"),
            CaptureError::Unsupported(m) => f.write_str(m),
            CaptureError::Failed(m) => write!(f, "capture failed: {m}"),
            CaptureError::Io(e) => write!(f, "capture failed: {e}"),
        }
    }
}

impl std::error::Error for CaptureError {}

impl From<std::io::Error> for CaptureError {
    fn from(e: std::io::Error) -> Self {
        CaptureError::Io(e)
    }
}

/// The `wl_shm` formats Hoarder can read. Each pixel is a little-endian `u32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShmFormat {
    Xrgb8888,
    Argb8888,
    Xbgr8888,
    Abgr8888,
    Xrgb2101010,
    Argb2101010,
    Xbgr2101010,
    Abgr2101010,
}

impl ShmFormat {
    /// In order of preference when a compositor offers several.
    const ALL: [ShmFormat; 8] = [
        ShmFormat::Xrgb2101010,
        ShmFormat::Xbgr2101010,
        ShmFormat::Argb2101010,
        ShmFormat::Abgr2101010,
        ShmFormat::Xrgb8888,
        ShmFormat::Argb8888,
        ShmFormat::Xbgr8888,
        ShmFormat::Abgr8888,
    ];

    fn wl(self) -> wl_shm::Format {
        use wl_shm::Format as F;
        match self {
            ShmFormat::Xrgb8888 => F::Xrgb8888,
            ShmFormat::Argb8888 => F::Argb8888,
            ShmFormat::Xbgr8888 => F::Xbgr8888,
            ShmFormat::Abgr8888 => F::Abgr8888,
            ShmFormat::Xrgb2101010 => F::Xrgb2101010,
            ShmFormat::Argb2101010 => F::Argb2101010,
            ShmFormat::Xbgr2101010 => F::Xbgr2101010,
            ShmFormat::Abgr2101010 => F::Abgr2101010,
        }
    }

    /// 8 or 10.
    pub fn bits_per_channel(self) -> u32 {
        match self {
            ShmFormat::Xrgb8888
            | ShmFormat::Argb8888
            | ShmFormat::Xbgr8888
            | ShmFormat::Abgr8888 => 8,
            _ => 10,
        }
    }

    /// Split one pixel into `[r, g, b]` at [`bits_per_channel`](Self::bits_per_channel).
    pub fn rgb(self, pixel: u32) -> [u16; 3] {
        let (hi, mid, lo) = if self.bits_per_channel() == 8 {
            ((pixel >> 16) & 0xff, (pixel >> 8) & 0xff, pixel & 0xff)
        } else {
            ((pixel >> 20) & 0x3ff, (pixel >> 10) & 0x3ff, pixel & 0x3ff)
        };
        let bgr = matches!(
            self,
            ShmFormat::Xbgr8888
                | ShmFormat::Abgr8888
                | ShmFormat::Xbgr2101010
                | ShmFormat::Abgr2101010
        );
        if bgr {
            [lo as u16, mid as u16, hi as u16]
        } else {
            [hi as u16, mid as u16, lo as u16]
        }
    }
}

/// An output transform, with `wl_output` semantics: the buffer must be rotated
/// clockwise by the angle (then mirrored horizontally, for `Flipped*`) to be
/// upright. Sway's config names are the inverse for 90 and 270.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transform {
    Normal,
    Rotate90,
    Rotate180,
    Rotate270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

impl Transform {
    fn from_wl(t: WEnum<wl_output::Transform>) -> Self {
        use wl_output::Transform as T;
        match t {
            WEnum::Value(T::_90) => Transform::Rotate90,
            WEnum::Value(T::_180) => Transform::Rotate180,
            WEnum::Value(T::_270) => Transform::Rotate270,
            WEnum::Value(T::Flipped) => Transform::Flipped,
            WEnum::Value(T::Flipped90) => Transform::Flipped90,
            WEnum::Value(T::Flipped180) => Transform::Flipped180,
            WEnum::Value(T::Flipped270) => Transform::Flipped270,
            _ => Transform::Normal,
        }
    }

    /// Whether width and height swap between buffer and upright image.
    pub fn swaps_axes(self) -> bool {
        matches!(
            self,
            Transform::Rotate90
                | Transform::Rotate270
                | Transform::Flipped90
                | Transform::Flipped270
        )
    }
}

/// An output's place in the compositor's layout, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogicalRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// One output's buffer exactly as the compositor holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrozenFrame {
    /// Connector name, e.g. `DP-1`.
    pub output: String,
    pub logical: LogicalRect,
    pub format: ShmFormat,
    /// Buffer size in pixels, before the transform is undone.
    pub width: u32,
    pub height: u32,
    /// Bytes per row.
    pub stride: u32,
    pub transform: Transform,
    /// Rows are stored bottom-up.
    pub y_invert: bool,
    pub data: Vec<u8>,
}

impl FrozenFrame {
    /// The raw pixel at buffer coordinates `(x, y)`, rows as stored.
    pub fn pixel(&self, x: u32, y: u32) -> u32 {
        let i = (y * self.stride + x * 4) as usize;
        u32::from_le_bytes([
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ])
    }

    /// Size in pixels once upright.
    pub fn upright_size(&self) -> (u32, u32) {
        if self.transform.swaps_axes() {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }
}

/// The Frozen Frames of every output, in the compositor's advertising order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frozen {
    pub backend: Backend,
    pub frames: Vec<FrozenFrame>,
}

/// Connect to `$WAYLAND_DISPLAY` and freeze every output.
pub fn freeze() -> Result<Frozen, CaptureError> {
    let conn = Connection::connect_to_env().map_err(|e| CaptureError::NoWayland(e.to_string()))?;
    let (globals, mut queue) =
        registry_queue_init::<State>(&conn).map_err(|e| CaptureError::NoWayland(e.to_string()))?;
    let hidden: HashSet<String> = std::env::var(HIDE_GLOBALS_ENV)
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    let mut freezer = Freezer {
        globals,
        hidden,
        qh: queue.handle(),
        shm: None,
        state: State::default(),
    };
    freezer.discover_outputs(&mut queue)?;

    let backend = if freezer.has(EXT_CAPTURE) && freezer.has(EXT_OUTPUT_SOURCE) {
        Backend::ExtImageCopyCapture
    } else if freezer.has(WLR_SCREENCOPY) {
        Backend::WlrScreencopy
    } else {
        return Err(CaptureError::Unsupported(
            "this compositor offers no screen capture protocol \
             (neither ext-image-copy-capture-v1 nor wlr-screencopy-unstable-v1). \
             Hoarder supports wlroots-based compositors, Hyprland, niri and COSMIC; \
             KWin and GNOME only offer capture through the desktop portal, which Hoarder does not use."
                .into(),
        ));
    };
    match backend {
        Backend::ExtImageCopyCapture => freezer.capture_ext(&mut queue)?,
        Backend::WlrScreencopy => freezer.capture_wlr(&mut queue)?,
    }

    let frames = freezer
        .state
        .outputs
        .into_iter()
        .map(|mut o| {
            let copy = o.copy.take().expect("every output was captured");
            let (format, width, height, stride) = copy.buffer.expect("buffer chosen");
            let transform = copy.transform.unwrap_or(o.transform);
            let logical = o.logical_rect(transform);
            FrozenFrame {
                output: o.name.unwrap_or_default(),
                logical,
                format,
                width,
                height,
                stride,
                transform,
                y_invert: copy.y_invert,
                data: copy.data,
            }
        })
        .collect();
    Ok(Frozen { backend, frames })
}

struct Freezer {
    globals: GlobalList,
    hidden: HashSet<String>,
    qh: QueueHandle<State>,
    shm: Option<WlShm>,
    state: State,
}

impl Freezer {
    fn has(&self, interface: &str) -> bool {
        !self.hidden.contains(interface)
            && self
                .globals
                .contents()
                .with_list(|list| list.iter().any(|g| g.interface == interface))
    }

    fn bind<P>(
        &self,
        interface: &str,
        versions: std::ops::RangeInclusive<u32>,
    ) -> Result<P, CaptureError>
    where
        P: Proxy + 'static,
        State: Dispatch<P, ()>,
    {
        if !self.has(interface) {
            return Err(CaptureError::Unsupported(format!(
                "the compositor does not offer {interface}"
            )));
        }
        self.globals
            .bind(&self.qh, versions, ())
            .map_err(|e| CaptureError::Unsupported(format!("cannot bind {interface}: {e}")))
    }

    fn shm(&mut self) -> Result<WlShm, CaptureError> {
        if self.shm.is_none() {
            self.shm = Some(self.bind("wl_shm", 1..=1)?);
        }
        Ok(self.shm.clone().expect("bound above"))
    }

    fn dispatch_until(
        &mut self,
        queue: &mut EventQueue<State>,
        what: &str,
        done: impl Fn(&State) -> bool,
    ) -> Result<(), CaptureError> {
        while !done(&self.state) {
            queue
                .blocking_dispatch(&mut self.state)
                .map_err(|e| CaptureError::Failed(format!("{what}: {e}")))?;
            if let Some(why) = self.state.failure.take() {
                return Err(CaptureError::Failed(why));
            }
        }
        Ok(())
    }

    fn discover_outputs(&mut self, queue: &mut EventQueue<State>) -> Result<(), CaptureError> {
        let outputs: Vec<(u32, u32)> = self.globals.contents().with_list(|list| {
            list.iter()
                .filter(|g| g.interface == "wl_output")
                .map(|g| (g.name, g.version))
                .collect()
        });
        if outputs.is_empty() {
            return Err(CaptureError::Failed("the compositor has no outputs".into()));
        }
        for (i, (name, version)) in outputs.into_iter().enumerate() {
            let wl: WlOutput = self
                .globals
                .registry()
                .bind(name, version.min(4), &self.qh, i);
            self.state.outputs.push(OutputState::new(wl));
        }
        if self.has("zxdg_output_manager_v1") {
            let manager: ZxdgOutputManagerV1 = self.bind("zxdg_output_manager_v1", 2..=3)?;
            for (i, o) in self.state.outputs.iter().enumerate() {
                manager.get_xdg_output(&o.wl, &self.qh, i);
            }
        }
        let conn_err = |e: wayland_client::DispatchError| CaptureError::Failed(e.to_string());
        queue.roundtrip(&mut self.state).map_err(conn_err)?;
        queue.roundtrip(&mut self.state).map_err(conn_err)?;
        Ok(())
    }

    fn capture_ext(&mut self, queue: &mut EventQueue<State>) -> Result<(), CaptureError> {
        let manager: ExtImageCopyCaptureManagerV1 = self.bind(EXT_CAPTURE, 1..=1)?;
        let sources: ExtOutputImageCaptureSourceManagerV1 = self.bind(EXT_OUTPUT_SOURCE, 1..=1)?;
        let mut sessions = Vec::new();
        for i in 0..self.state.outputs.len() {
            self.state.outputs[i].copy = Some(Copy::default());
            let source = sources.create_source(&self.state.outputs[i].wl, &self.qh, ());
            sessions.push(manager.create_session(&source, Options::empty(), &self.qh, i));
            source.destroy();
        }
        self.dispatch_until(queue, "capture constraints", |s| {
            s.copies().all(|c| c.constraints_done)
        })?;

        let wl_shm = self.shm()?;
        let mut frames = Vec::new();
        for (i, session) in sessions.iter().enumerate() {
            let copy = self.state.outputs[i].copy.as_mut().expect("set above");
            let format = ShmFormat::ALL
                .into_iter()
                .find(|f| copy.offered.contains(&(f.wl() as u32)))
                .ok_or_else(|| {
                    CaptureError::Unsupported(format!(
                        "the compositor offers no readable shm format for an output (offered: {:x?})",
                        copy.offered
                    ))
                })?;
            let (width, height) = copy.size;
            let shm = Shm::new(&wl_shm, &self.qh, format, width, height, width * 4)?;
            let frame = session.create_frame(&self.qh, i);
            frame.attach_buffer(&shm.buffer);
            frame.damage_buffer(0, 0, width as i32, height as i32);
            frame.capture();
            copy.buffer = Some((format, width, height, width * 4));
            frames.push((frame, shm));
        }
        self.dispatch_until(queue, "capture", |s| s.copies().all(|c| c.ready))?;

        for (i, (frame, shm)) in frames.into_iter().enumerate() {
            frame.destroy();
            self.state.outputs[i].copy.as_mut().expect("set above").data = shm.read()?;
        }
        for session in sessions {
            session.destroy();
        }
        Ok(())
    }

    fn capture_wlr(&mut self, queue: &mut EventQueue<State>) -> Result<(), CaptureError> {
        let manager: ZwlrScreencopyManagerV1 = self.bind(WLR_SCREENCOPY, 1..=3)?;
        let v3 = manager.version() >= 3;
        let mut frames = Vec::new();
        for i in 0..self.state.outputs.len() {
            self.state.outputs[i].copy = Some(Copy::default());
            frames.push(manager.capture_output(0, &self.state.outputs[i].wl, &self.qh, i));
        }
        // Before v3 there is no buffer_done: the first shm `buffer` event is all we get.
        self.dispatch_until(queue, "screencopy buffer", |s| {
            s.copies()
                .all(|c| c.constraints_done || (!v3 && !c.wlr_buffers.is_empty()))
        })?;

        let wl_shm = self.shm()?;
        let mut shms = Vec::new();
        for (i, frame) in frames.iter().enumerate() {
            let copy = self.state.outputs[i].copy.as_mut().expect("set above");
            let (format, width, height, stride) = ShmFormat::ALL
                .into_iter()
                .find_map(|f| {
                    copy.wlr_buffers
                        .iter()
                        .find(|b| b.0 == f.wl() as u32)
                        .map(|&(_, w, h, s)| (f, w, h, s))
                })
                .ok_or_else(|| {
                    CaptureError::Unsupported(
                        "the compositor offers no readable shm format for screencopy".into(),
                    )
                })?;
            let shm = Shm::new(&wl_shm, &self.qh, format, width, height, stride)?;
            frame.copy(&shm.buffer);
            copy.buffer = Some((format, width, height, stride));
            shms.push(shm);
        }
        self.dispatch_until(queue, "screencopy", |s| s.copies().all(|c| c.ready))?;

        for (i, (frame, shm)) in frames.into_iter().zip(shms).enumerate() {
            frame.destroy();
            self.state.outputs[i].copy.as_mut().expect("set above").data = shm.read()?;
        }
        Ok(())
    }
}

/// A `wl_buffer` backed by a memfd that we read back after the copy.
struct Shm {
    file: std::fs::File,
    buffer: WlBuffer,
}

impl Shm {
    fn new(
        shm: &WlShm,
        qh: &QueueHandle<State>,
        format: ShmFormat,
        width: u32,
        height: u32,
        stride: u32,
    ) -> Result<Self, CaptureError> {
        let size = stride as usize * height as usize;
        let fd = rustix::fs::memfd_create("hoarder-frozen-frame", rustix::fs::MemfdFlags::CLOEXEC)
            .map_err(std::io::Error::from)?;
        let file = std::fs::File::from(fd);
        file.set_len(size as u64)?;
        let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride as i32,
            format.wl(),
            qh,
            (),
        );
        pool.destroy();
        Ok(Shm { file, buffer })
    }

    fn read(self) -> Result<Vec<u8>, CaptureError> {
        let size = self.file.metadata()?.len() as usize;
        let mut data = vec![0u8; size];
        self.file.read_exact_at(&mut data, 0)?;
        self.buffer.destroy();
        Ok(data)
    }
}

#[derive(Default)]
struct State {
    outputs: Vec<OutputState>,
    failure: Option<String>,
}

impl State {
    fn copies(&self) -> impl Iterator<Item = &Copy> {
        self.outputs.iter().filter_map(|o| o.copy.as_ref())
    }

    fn copy(&mut self, i: usize) -> &mut Copy {
        self.outputs[i].copy.get_or_insert_with(Copy::default)
    }
}

struct OutputState {
    wl: WlOutput,
    name: Option<String>,
    transform: Transform,
    mode: (i32, i32),
    position: (i32, i32),
    scale: i32,
    logical_position: Option<(i32, i32)>,
    logical_size: Option<(i32, i32)>,
    copy: Option<Copy>,
}

impl OutputState {
    fn new(wl: WlOutput) -> Self {
        OutputState {
            wl,
            name: None,
            transform: Transform::Normal,
            mode: (0, 0),
            position: (0, 0),
            scale: 1,
            logical_position: None,
            logical_size: None,
            copy: None,
        }
    }

    /// From xdg-output when the compositor has it, else from `wl_output`.
    fn logical_rect(&self, transform: Transform) -> LogicalRect {
        let (x, y) = self.logical_position.unwrap_or(self.position);
        let (w, h) = self.logical_size.unwrap_or_else(|| {
            let (w, h) = (
                self.mode.0 / self.scale.max(1),
                self.mode.1 / self.scale.max(1),
            );
            if transform.swaps_axes() {
                (h, w)
            } else {
                (w, h)
            }
        });
        LogicalRect {
            x,
            y,
            width: w.max(0) as u32,
            height: h.max(0) as u32,
        }
    }
}

/// Progress of one output's capture, whichever protocol is in use.
#[derive(Default)]
struct Copy {
    /// ext: `BufferSize`.
    size: (u32, u32),
    /// ext: `ShmFormat` codes offered.
    offered: Vec<u32>,
    /// wlr: every `buffer` event (format, width, height, stride).
    wlr_buffers: Vec<(u32, u32, u32, u32)>,
    /// ext: session `done`; wlr: `buffer_done`.
    constraints_done: bool,
    buffer: Option<(ShmFormat, u32, u32, u32)>,
    transform: Option<Transform>,
    y_invert: bool,
    ready: bool,
    data: Vec<u8>,
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

impl Dispatch<WlOutput, usize> for State {
    fn event(
        state: &mut Self,
        _: &WlOutput,
        event: wl_output::Event,
        &i: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let o = &mut state.outputs[i];
        match event {
            wl_output::Event::Geometry {
                x, y, transform, ..
            } => {
                o.position = (x, y);
                o.transform = Transform::from_wl(transform);
            }
            wl_output::Event::Mode {
                flags,
                width,
                height,
                ..
            } => {
                if matches!(flags, WEnum::Value(f) if f.contains(wl_output::Mode::Current)) {
                    o.mode = (width, height);
                }
            }
            wl_output::Event::Scale { factor } => o.scale = factor,
            wl_output::Event::Name { name } => o.name = Some(name),
            _ => {}
        }
    }
}

impl Dispatch<ZxdgOutputV1, usize> for State {
    fn event(
        state: &mut Self,
        _: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        &i: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let o = &mut state.outputs[i];
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => o.logical_position = Some((x, y)),
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                o.logical_size = Some((width, height))
            }
            zxdg_output_v1::Event::Name { name } if o.name.is_none() => o.name = Some(name),
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, usize> for State {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        &i: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_session_v1::Event as E;
        let copy = state.copy(i);
        match event {
            E::BufferSize { width, height } => copy.size = (width, height),
            E::ShmFormat { format } => copy.offered.push(match format {
                WEnum::Value(f) => f as u32,
                WEnum::Unknown(code) => code,
            }),
            E::Done => copy.constraints_done = true,
            E::Stopped if !copy.ready => {
                state.failure = Some("the compositor stopped the capture session".into());
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, usize> for State {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        &i: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_frame_v1::Event as E;
        match event {
            E::Transform { transform } => {
                state.copy(i).transform = Some(Transform::from_wl(transform))
            }
            E::Ready => state.copy(i).ready = true,
            E::Failed { reason } => {
                state.failure = Some(format!(
                    "the compositor could not copy an output ({reason:?})"
                ))
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, usize> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        &i: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_screencopy_frame_v1::Event as E;
        let copy = state.copy(i);
        match event {
            E::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                let code = match format {
                    WEnum::Value(f) => f as u32,
                    WEnum::Unknown(code) => code,
                };
                copy.wlr_buffers.push((code, width, height, stride));
            }
            E::BufferDone => copy.constraints_done = true,
            E::Flags { flags } => {
                copy.y_invert = matches!(flags, WEnum::Value(f)
                    if f.contains(zwlr_screencopy_frame_v1::Flags::YInvert));
            }
            E::Ready { .. } => copy.ready = true,
            E::Failed => state.failure = Some("the compositor could not copy an output".into()),
            _ => {}
        }
    }
}

delegate_noop!(State: ignore WlShm);
delegate_noop!(State: WlShmPool);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: ZxdgOutputManagerV1);
delegate_noop!(State: ExtImageCopyCaptureManagerV1);
delegate_noop!(State: ExtOutputImageCaptureSourceManagerV1);
delegate_noop!(State: ExtImageCaptureSourceV1);
delegate_noop!(State: ZwlrScreencopyManagerV1);
