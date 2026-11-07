//! The Overlay (PLAN.md §4.2): one layer surface per output showing its Frozen
//! Frame 1:1, with the chrome drawn on top, and a selection that is global
//! across outputs.
//!
//! Each output gets an Overlay-layer `zwlr_layer_surface_v1` whose own buffer
//! is the chrome (`Argb8888`, physical size, shown through `wp_viewporter`).
//! The Frozen Frame is a subsurface placed below it, with the captured buffer
//! attached as is: same format, same transform, one buffer pixel per physical
//! pixel. So the compositor blends the frame, and the CPU only repaints the
//! chrome where it changed.

use std::io::Write as _;
use std::os::fd::AsFd;
use std::time::Instant;

use hoarder_wayland::{Frozen, FrozenFrame, Window};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
    delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        pointer::{
            PointerEvent, PointerEventKind, PointerHandler, cursor_shape::CursorShapeManager,
        },
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{
        Shm, ShmHandler,
        slot::{Buffer, SlotPool},
    },
    subcompositor::SubcompositorState,
};
use wayland_client::{
    Connection, EventQueue, QueueHandle, delegate_noop,
    globals::registry_queue_init,
    protocol::{
        wl_buffer::WlBuffer, wl_keyboard::WlKeyboard, wl_output::WlOutput, wl_pointer::WlPointer,
        wl_seat::WlSeat, wl_shm, wl_shm_pool::WlShmPool, wl_subsurface::WlSubsurface,
        wl_surface::WlSurface,
    },
};
use wayland_protocols::wp::{
    cursor_shape::v1::client::wp_cursor_shape_device_v1::{Shape, WpCursorShapeDeviceV1},
    single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1,
    viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
};

use crate::chrome::{DIM, Fonts, Highlight, Scene};
use crate::geom::Rect;
use crate::select::{Action, Key, Selector};
use crate::snap::Hover;

/// Set to print how long the Overlay took to become visible.
pub const TIMINGS_ENV: &str = "HOARDER_TIMINGS";

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

/// A rectangle in global logical coordinates (the compositor's layout), as
/// drawn on the Overlay. It may span outputs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// How the user left the Overlay.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// A Region: dragged out and released, confirmed in Refine, or the last
    /// Region (Enter with no selection).
    Region(Region),
    /// The Output with this name (`O`, or a click on empty desktop).
    Output(String),
    /// A click on the highlighted window.
    Window(Window),
    /// `A`.
    AllOutputs,
    /// Esc or right-click.
    Cancelled,
}

/// How the Overlay behaves for one run.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Enter Refine after every drag, not only after Shift+release
    /// (`screenshot.confirm`).
    pub confirm: bool,
    /// What Enter with no selection captures.
    pub last_region: Option<Region>,
    /// The windows hovering highlights and a click captures, topmost first,
    /// as a [`WindowGeometry`](hoarder_wayland::WindowGeometry) gives them.
    /// Empty without a provider: no highlighting.
    pub windows: Vec<Window>,
}

/// Everything that can stop the Overlay.
#[derive(Debug)]
pub enum OverlayError {
    /// The compositor lacks a protocol the Overlay needs.
    Unsupported(String),
    /// The Wayland connection failed.
    Failed(String),
}

impl std::fmt::Display for OverlayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OverlayError::Unsupported(m) => f.write_str(m),
            OverlayError::Failed(m) => write!(f, "overlay failed: {m}"),
        }
    }
}

impl std::error::Error for OverlayError {}

/// With [`TIMINGS_ENV`] set, print how long `stage` took to reach since `started`.
fn timing(started: Instant, stage: &str) {
    if std::env::var_os(TIMINGS_ENV).is_some() {
        eprintln!(
            "hoarder: timing {stage}_ms={:.1}",
            started.elapsed().as_secs_f64() * 1000.0
        );
    }
}

fn failed(e: impl std::fmt::Display) -> OverlayError {
    OverlayError::Failed(e.to_string())
}

/// Show the Overlay over `frozen` and wait for the user to pick a Target or
/// cancel. `started` is when the user asked for the Capture; with
/// [`TIMINGS_ENV`] set, the time until the Overlay is visible is printed.
pub fn run(frozen: &Frozen, started: Instant, options: &Options) -> Result<Outcome, OverlayError> {
    timing(started, "frozen");
    let conn = Connection::connect_to_env().map_err(failed)?;
    let (globals, mut queue): (_, EventQueue<App>) = registry_queue_init(&conn).map_err(failed)?;
    let qh = queue.handle();
    fn missing(what: &'static str) -> impl Fn(wayland_client::globals::BindError) -> OverlayError {
        move |e| {
            OverlayError::Unsupported(format!(
                "the compositor does not offer {what}, which the Overlay needs: {e}"
            ))
        }
    }
    let compositor = CompositorState::bind(&globals, &qh).map_err(missing("wl_compositor"))?;
    let subcompositor = SubcompositorState::bind(compositor.wl_compositor().clone(), &globals, &qh)
        .map_err(missing("wl_subcompositor"))?;
    let layer_shell = LayerShell::bind(&globals, &qh).map_err(missing("wlr-layer-shell"))?;
    let shm = Shm::bind(&globals, &qh).map_err(missing("wl_shm"))?;
    let viewporter: WpViewporter = globals
        .bind(&qh, 1..=1, ())
        .map_err(missing("wp_viewporter"))?;
    let cursor_shapes = CursorShapeManager::bind(&globals, &qh).ok();
    let single_pixel: Option<WpSinglePixelBufferManagerV1> = globals.bind(&qh, 1..=1, ()).ok();

    let mut app = App {
        registry: RegistryState::new(&globals),
        outputs: OutputState::new(&globals, &qh),
        seats: SeatState::new(&globals, &qh),
        pool: SlotPool::new(1, &shm).map_err(failed)?,
        shm,
        cursor_shapes,
        pointer: None,
        cursor_device: None,
        keyboard: None,
        views: Vec::new(),
        fonts: Fonts::load(),
        selector: Selector::new(options.confirm, options.last_region),
        pointer_at: None,
        hover: Hover::new(options.windows.clone()),
        outcome: None,
        started,
        visible_reported: false,
    };
    // Learn the outputs' names so each Frozen Frame lands on its own output.
    queue.roundtrip(&mut app).map_err(failed)?;

    for frame in &frozen.frames {
        let output = app
            .outputs
            .outputs()
            .find(|o| app.outputs.info(o).and_then(|i| i.name).as_deref() == Some(&frame.output))
            .ok_or_else(|| failed(format!("output {} went away", frame.output)))?;
        let dim = match &single_pixel {
            // Premultiplied, full range: black at DIM's alpha.
            Some(m) => m.create_u32_rgba_buffer(0, 0, 0, DIM[3] as u32 * 0x0101_0101, &qh, ()),
            None => pixel_buffer(DIM, &app.shm, &qh).map_err(failed)?,
        };
        let view = View::new(
            frame,
            &output,
            &compositor,
            &subcompositor,
            &layer_shell,
            &viewporter,
            &app.shm,
            dim,
            &qh,
        )
        .map_err(failed)?;
        app.views.push(view);
    }
    timing(started, "surfaces_created");

    while app.outcome.is_none() {
        queue.blocking_dispatch(&mut app).map_err(failed)?;
    }
    let outcome = app.outcome.take().expect("loop ended on an outcome");
    // Take the Overlay down before the caller encodes and delivers.
    app.views.clear();
    queue.roundtrip(&mut app).map_err(failed)?;
    Ok(outcome)
}

/// One output's Overlay surfaces.
struct View {
    /// The output's name, e.g. `DP-1`.
    output: String,
    /// Layout position and size of the output (logical).
    logical: (f64, f64, f64, f64),
    /// Physical size of the chrome buffer: the upright Frozen Frame size.
    size: (u32, u32),
    layer: LayerSurface,
    root_viewport: WpViewport,
    frame_surface: WlSurface,
    frame_subsurface: WlSubsurface,
    frame_viewport: WpViewport,
    frame_buffer: WlBuffer,
    /// Until there is a selection, the chrome is this one dim pixel stretched
    /// over the output: nothing to paint, nothing to upload.
    dim_buffer: WlBuffer,
    buffers: Vec<ChromeBuffer>,
    drawn: Scene,
    wanted: Scene,
    configured: bool,
    frame_pending: bool,
    presented: bool,
}

/// A chrome buffer and the areas that went stale while it was not on screen.
struct ChromeBuffer {
    buffer: Buffer,
    stale: Vec<Rect>,
}

impl View {
    #[allow(clippy::too_many_arguments)]
    fn new(
        frame: &FrozenFrame,
        output: &WlOutput,
        compositor: &CompositorState,
        subcompositor: &SubcompositorState,
        layer_shell: &LayerShell,
        viewporter: &WpViewporter,
        shm: &Shm,
        dim_buffer: WlBuffer,
        qh: &QueueHandle<App>,
    ) -> std::io::Result<View> {
        let root = compositor.create_surface(qh);
        let layer = layer_shell.create_layer_surface(
            qh,
            root,
            Layer::Overlay,
            Some("hoarder-overlay"),
            Some(output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        let root_viewport = viewporter.get_viewport(layer.wl_surface(), qh, ());

        let (frame_subsurface, frame_surface) =
            subcompositor.create_subsurface(layer.wl_surface().clone(), qh);
        frame_subsurface.place_below(layer.wl_surface());
        frame_subsurface.set_position(0, 0);
        let frame_viewport = viewporter.get_viewport(&frame_surface, qh, ());
        let frame_buffer = frozen_buffer(frame, shm, qh)?;
        let l = frame.logical;
        frame_surface.set_buffer_transform(frame.transform.to_wl());
        frame_viewport.set_destination(l.width as i32, l.height as i32);
        frame_surface.attach(Some(&frame_buffer), 0, 0);
        frame_surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        frame_surface.commit();
        layer.commit();

        Ok(View {
            output: frame.output.clone(),
            logical: (l.x as f64, l.y as f64, l.width as f64, l.height as f64),
            size: frame.upright_size(),
            layer,
            root_viewport,
            frame_surface,
            frame_subsurface,
            frame_viewport,
            frame_buffer,
            dim_buffer,
            buffers: Vec::new(),
            drawn: Scene::default(),
            wanted: Scene::default(),
            configured: false,
            frame_pending: false,
            presented: false,
        })
    }

    /// Physical pixels per logical pixel, horizontally and vertically.
    fn scale(&self) -> (f64, f64) {
        (
            self.size.0 as f64 / self.logical.2.max(1.0),
            self.size.1 as f64 / self.logical.3.max(1.0),
        )
    }

    /// Whether the global logical point `(x, y)` lies on this output.
    fn contains(&self, (x, y): (f64, f64)) -> bool {
        let (ox, oy, w, h) = self.logical;
        x >= ox && x < ox + w && y >= oy && y < oy + h
    }

    /// A global logical rectangle in this output's physical pixels.
    fn to_physical(&self, r: &Region) -> Rect {
        let (sx, sy) = self.scale();
        let px = |v: f64, origin: f64, s: f64| ((v - origin) * s).round() as i32;
        let x0 = px(r.x, self.logical.0, sx);
        let y0 = px(r.y, self.logical.1, sy);
        let x1 = px(r.x + r.width, self.logical.0, sx);
        let y1 = px(r.y + r.height, self.logical.1, sy);
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// Draw `wanted` if the compositor is ready for a new frame.
    fn redraw(&mut self, pool: &mut SlotPool, qh: &QueueHandle<App>) -> std::io::Result<()> {
        if !self.configured || self.frame_pending {
            return Ok(());
        }
        let first = self.buffers.is_empty();
        if first && self.wanted == Scene::default() {
            if !self.presented {
                let surface = self.layer.wl_surface();
                self.root_viewport
                    .set_destination(self.logical.2 as i32, self.logical.3 as i32);
                surface.attach(Some(&self.dim_buffer), 0, 0);
                surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
                surface.frame(qh, FrameCallbackData(surface.clone()));
                self.layer.commit();
                self.frame_pending = true;
            }
            return Ok(());
        }
        let full = Rect::new(0, 0, self.size.0 as i32, self.size.1 as i32);
        let damage = if first {
            vec![full]
        } else {
            self.drawn.damage(&self.wanted)
        };
        if damage.is_empty() {
            return Ok(());
        }
        let (w, h) = self.size;
        let free = self
            .buffers
            .iter()
            .position(|b| b.buffer.canvas(pool).is_some());
        let (index, repaint) = match free {
            Some(i) => {
                let stale = std::mem::take(&mut self.buffers[i].stale);
                (i, stale.into_iter().chain(damage.iter().copied()).collect())
            }
            None => {
                let (buffer, _) = pool
                    .create_buffer(w as i32, h as i32, w as i32 * 4, wl_shm::Format::Argb8888)
                    .map_err(std::io::Error::other)?;
                self.buffers.push(ChromeBuffer {
                    buffer,
                    stale: Vec::new(),
                });
                (self.buffers.len() - 1, vec![full])
            }
        };
        for (i, b) in self.buffers.iter_mut().enumerate() {
            if i != index {
                b.stale.extend(&damage);
                if b.stale.len() > 32 {
                    let all = b.stale.iter().fold(Rect::default(), |a, r| a.union(r));
                    b.stale = vec![all];
                }
            }
        }
        let target = &self.buffers[index];
        let canvas = target.buffer.canvas(pool).expect("free or new");
        for area in repaint {
            self.wanted.paint(canvas, w, h, area);
        }

        let surface = self.layer.wl_surface();
        if first {
            self.root_viewport
                .set_destination(self.logical.2 as i32, self.logical.3 as i32);
        }
        for r in &damage {
            surface.damage_buffer(r.x, r.y, r.w, r.h);
        }
        surface.frame(qh, FrameCallbackData(surface.clone()));
        target
            .buffer
            .attach_to(surface)
            .map_err(std::io::Error::other)?;
        self.layer.commit();
        self.frame_pending = true;
        self.drawn = self.wanted.clone();
        Ok(())
    }
}

impl Drop for View {
    fn drop(&mut self) {
        self.frame_viewport.destroy();
        self.frame_subsurface.destroy();
        self.frame_surface.destroy();
        self.frame_buffer.destroy();
        self.dim_buffer.destroy();
        self.root_viewport.destroy();
    }
}

/// The Frozen Frame as a `wl_buffer`, byte for byte (rows flipped if the
/// compositor handed them over bottom-up).
fn frozen_buffer(
    frame: &FrozenFrame,
    shm: &Shm,
    qh: &QueueHandle<App>,
) -> std::io::Result<WlBuffer> {
    let size = frame.stride as usize * frame.height as usize;
    let fd = rustix::fs::memfd_create("hoarder-overlay-frame", rustix::fs::MemfdFlags::CLOEXEC)?;
    let mut file = std::fs::File::from(fd);
    if frame.y_invert {
        let stride = frame.stride as usize;
        for row in frame.data[..size].chunks_exact(stride).rev() {
            file.write_all(row)?;
        }
    } else {
        file.write_all(&frame.data[..size])?;
    }
    let format = frame.format.wl();
    Ok(file_buffer(
        &file,
        (frame.width, frame.height, frame.stride),
        format,
        shm,
        qh,
    ))
}

/// One `Argb8888` pixel, for the compositor to stretch over a whole surface.
fn pixel_buffer(pixel: [u8; 4], shm: &Shm, qh: &QueueHandle<App>) -> std::io::Result<WlBuffer> {
    let fd = rustix::fs::memfd_create("hoarder-overlay-dim", rustix::fs::MemfdFlags::CLOEXEC)?;
    let mut file = std::fs::File::from(fd);
    file.write_all(&pixel)?;
    Ok(file_buffer(
        &file,
        (1, 1, 4),
        wl_shm::Format::Argb8888,
        shm,
        qh,
    ))
}

/// A `wl_buffer` over the whole of `file`, `(width, height, stride)`.
fn file_buffer(
    file: &std::fs::File,
    (width, height, stride): (u32, u32, u32),
    format: wl_shm::Format,
    shm: &Shm,
    qh: &QueueHandle<App>,
) -> WlBuffer {
    let size = stride as i32 * height as i32;
    let pool = shm.wl_shm().create_pool(file.as_fd(), size, qh, ());
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        stride as i32,
        format,
        qh,
        (),
    );
    pool.destroy();
    buffer
}

struct App {
    registry: RegistryState,
    outputs: OutputState,
    seats: SeatState,
    shm: Shm,
    pool: SlotPool,
    cursor_shapes: Option<CursorShapeManager>,
    pointer: Option<WlPointer>,
    cursor_device: Option<WpCursorShapeDeviceV1>,
    keyboard: Option<WlKeyboard>,
    views: Vec<View>,
    fonts: Fonts,
    selector: Selector,
    /// Where the pointer last was, in global logical coordinates.
    pointer_at: Option<(f64, f64)>,
    hover: Hover,
    outcome: Option<Outcome>,
    started: Instant,
    visible_reported: bool,
}

impl App {
    fn view_of(&self, surface: &WlSurface) -> Option<usize> {
        self.views
            .iter()
            .position(|v| v.layer.wl_surface() == surface)
    }

    /// The output under the global logical point `at`, or the nearest one.
    fn view_at(&self, at: (f64, f64)) -> Option<&View> {
        self.views.iter().find(|v| v.contains(at)).or_else(|| {
            let distance = |v: &View| {
                let (ox, oy, w, h) = v.logical;
                let dx = (ox - at.0).max(at.0 - (ox + w)).max(0.0);
                let dy = (oy - at.1).max(at.1 - (oy + h)).max(0.0);
                dx * dx + dy * dy
            };
            self.views
                .iter()
                .min_by(|a, b| distance(a).total_cmp(&distance(b)))
        })
    }

    /// The size of one physical pixel at `at`, in logical pixels.
    fn pixel_at(&self, at: (f64, f64)) -> f64 {
        self.view_at(at).map_or(1.0, |v| 1.0 / v.scale().0)
    }

    /// Act on what the selection asked for.
    fn act(&mut self, action: Action) {
        self.outcome = match action {
            Action::Region(region) => Some(Outcome::Region(region)),
            Action::OutputAt(at) => self.view_at(at).map(|v| Outcome::Output(v.output.clone())),
            Action::AllOutputs => Some(Outcome::AllOutputs),
            Action::Cancel => Some(Outcome::Cancelled),
        };
    }

    /// A key went down (or up, for `released`).
    fn key_event(&mut self, keysym: Keysym, released: bool, qh: &QueueHandle<App>) {
        if matches!(keysym, Keysym::w | Keysym::W) {
            if !released {
                self.hover.cycle();
                self.update(qh);
            }
            return;
        }
        let Some(key) = key_of(keysym) else {
            return;
        };
        if key == Key::Output {
            if !released {
                let at = self.pointer_at.unwrap_or_else(|| {
                    self.views
                        .first()
                        .map_or((0.0, 0.0), |v| (v.logical.0, v.logical.1))
                });
                self.act(Action::OutputAt(at));
            }
            return;
        }
        let px = self
            .selector
            .moving_point()
            .map_or(1.0, |at| self.pixel_at(at));
        match self.selector.key(key, released, px) {
            Some(action) => self.act(action),
            None => self.update(qh),
        }
    }

    /// Recompute every output's scene from the selection and draw what changed.
    fn update(&mut self, qh: &QueueHandle<App>) {
        let region = self.selector.region();
        let refining = self.selector.refining();
        // Hovering highlights a window until a selection takes over.
        let hovered = self
            .hover
            .current()
            .filter(|_| !refining && region.is_none_or(|r| r.width == 0.0 && r.height == 0.0));
        for i in 0..self.views.len() {
            let window = hovered.and_then(|w| self.highlight(i, w));
            let view = &self.views[i];
            let selection = region
                .map(|r| view.to_physical(&r))
                .filter(|r| !r.is_empty());
            let label = match (selection, region) {
                (Some(sel), Some(_)) => {
                    let text = format!("{} × {}", sel.w, sel.h);
                    let keep = view.drawn.label.as_ref().filter(|l| {
                        l.text == text
                            && l.rect.x == sel.x
                            && l.rect.y == label_y(sel, l.rect.h, view.size.1)
                    });
                    match keep {
                        Some(l) => Some(l.clone()),
                        None => {
                            let (sx, _) = view.scale();
                            let mut l = self.fonts.label(&text, sx, sel.x, 0);
                            l.rect.y = label_y(sel, l.rect.h, view.size.1);
                            Some(l)
                        }
                    }
                }
                _ => None,
            };
            let handles = match selection {
                Some(sel) if refining => handles(sel, view.scale().0),
                _ => Vec::new(),
            };
            let view = &mut self.views[i];
            view.wanted = Scene {
                window,
                selection,
                label,
                handles,
            };
            if let Err(e) = view.redraw(&mut self.pool, qh) {
                self.outcome = Some(Outcome::Cancelled);
                eprintln!("hoarder: cannot draw the Overlay: {e}");
            }
        }
    }

    /// Window `index` as highlighted on output `view`, if it shows there.
    fn highlight(&mut self, view: usize, index: usize) -> Option<Highlight> {
        let view = &self.views[view];
        let window = self.hover.window(index);
        let r = window.rect;
        let rect = view.to_physical(&Region {
            x: r.x as f64,
            y: r.y as f64,
            width: r.width as f64,
            height: r.height as f64,
        });
        let shown = rect.intersect(&Rect::new(0, 0, view.size.0 as i32, view.size.1 as i32));
        if shown.is_empty() {
            return None;
        }
        if let Some(h) = &view.drawn.window
            && h.index == index
            && h.rect == rect
        {
            return Some(h.clone());
        }
        let text = if window.title.is_empty() {
            &window.app_id
        } else {
            &window.title
        };
        let label = (!text.is_empty()).then(|| {
            let (sx, _) = view.scale();
            let mut l = self.fonts.label(&ellipsize(text, 80), sx, 0, 0);
            // Centred on the part of the window this output shows.
            l.rect.x = shown.x + ((shown.w - l.rect.w) / 2).max(0);
            l.rect.y = shown.y + ((shown.h - l.rect.h) / 2).max(0);
            l
        });
        Some(Highlight { index, rect, label })
    }

    fn pointer_event(&mut self, event: &PointerEvent, qh: &QueueHandle<App>) {
        let Some(i) = self.view_of(&event.surface) else {
            return;
        };
        let (ox, oy, _, _) = self.views[i].logical;
        let at = (ox + event.position.0, oy + event.position.1);
        self.pointer_at = Some(at);
        match event.kind {
            PointerEventKind::Enter { serial } => {
                if let Some(device) = &self.cursor_device {
                    device.set_shape(serial, Shape::Crosshair);
                }
                self.hover.point(at);
                self.update(qh);
            }
            PointerEventKind::Press {
                button: BTN_LEFT, ..
            } => {
                self.selector.press(at);
                self.update(qh);
            }
            PointerEventKind::Motion { .. } => {
                self.selector.motion(at);
                self.hover.point(at);
                self.update(qh);
            }
            PointerEventKind::Release {
                button: BTN_LEFT, ..
            } => match self.selector.release(at, self.pixel_at(at)) {
                // A click: the highlighted window, if any, else the Output.
                Some(Action::OutputAt(_)) if let Some(w) = self.hover.current() => {
                    self.outcome = Some(Outcome::Window(self.hover.window(w).clone()));
                }
                Some(action) => self.act(action),
                None => self.update(qh),
            },
            PointerEventKind::Press {
                button: BTN_RIGHT, ..
            } => self.outcome = Some(Outcome::Cancelled),
            _ => {}
        }
    }
}

/// `text`, cut to at most `max` characters with an ellipsis.
fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// The key a keysym stands for in the Overlay, if any.
fn key_of(keysym: Keysym) -> Option<Key> {
    Some(match keysym {
        Keysym::Escape => Key::Escape,
        Keysym::Return | Keysym::KP_Enter => Key::Enter,
        Keysym::space => Key::Space,
        Keysym::o | Keysym::O => Key::Output,
        Keysym::a | Keysym::A => Key::AllOutputs,
        Keysym::Left | Keysym::KP_Left => Key::Left,
        Keysym::Right | Keysym::KP_Right => Key::Right,
        Keysym::Up | Keysym::KP_Up => Key::Up,
        Keysym::Down | Keysym::KP_Down => Key::Down,
        _ => return None,
    })
}

/// Refine's eight resize handles around `sel` (physical pixels): squares
/// centred on the border's corners and the middle of each side.
fn handles(sel: Rect, scale: f64) -> Vec<Rect> {
    let size = ((7.0 * scale).round() as i32) | 1;
    let ring = sel.inflate(1);
    let xs = [ring.x, (ring.x + ring.right() - 1) / 2, ring.right() - 1];
    let ys = [ring.y, (ring.y + ring.bottom() - 1) / 2, ring.bottom() - 1];
    let mut out = Vec::with_capacity(8);
    for (j, &y) in ys.iter().enumerate() {
        for (i, &x) in xs.iter().enumerate() {
            if i != 1 || j != 1 {
                out.push(Rect::new(x - size / 2, y - size / 2, size, size));
            }
        }
    }
    out
}

/// Where the size label goes: just below the selection, or above it when
/// there is no room below, or inside its top edge as a last resort.
fn label_y(sel: Rect, h: i32, output_h: u32) -> i32 {
    let gap = 6;
    let below = sel.bottom() + 1 + gap;
    let above = sel.y - 1 - gap - h;
    if below + h <= output_h as i32 {
        below
    } else if above >= 0 {
        above
    } else {
        sel.y + gap
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: wayland_client::protocol::wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, qh: &QueueHandle<Self>, surface: &WlSurface, _: u32) {
        let Some(i) = self.view_of(surface) else {
            return;
        };
        let view = &mut self.views[i];
        view.frame_pending = false;
        view.presented = true;
        if !self.visible_reported && self.views.iter().all(|v| v.presented) {
            self.visible_reported = true;
            timing(self.started, "overlay_visible");
        }
        if let Err(e) = self.views[i].redraw(&mut self.pool, qh) {
            eprintln!("hoarder: cannot draw the Overlay: {e}");
            self.outcome = Some(Outcome::Cancelled);
        }
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &WlOutput,
    ) {
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.outcome = Some(Outcome::Cancelled);
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        _: LayerSurfaceConfigure,
        _: u32,
    ) {
        let Some(i) = self.view_of(layer.wl_surface()) else {
            return;
        };
        self.views[i].configured = true;
        if let Err(e) = self.views[i].redraw(&mut self.pool, qh) {
            eprintln!("hoarder: cannot draw the Overlay: {e}");
            self.outcome = Some(Outcome::Cancelled);
        }
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer
            && self.pointer.is_none()
            && let Ok(pointer) = self.seats.get_pointer(qh, &seat)
        {
            self.cursor_device = self
                .cursor_shapes
                .as_ref()
                .map(|m| m.get_shape_device(&pointer, qh));
            self.pointer = Some(pointer);
        }
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = self.seats.get_keyboard(qh, &seat, None).ok();
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(d) = self.cursor_device.take() {
                d.destroy();
            }
            if let Some(p) = self.pointer.take() {
                p.release();
            }
        }
        if capability == Capability::Keyboard
            && let Some(k) = self.keyboard.take()
        {
            k.release();
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            if self.outcome.is_some() {
                break;
            }
            self.pointer_event(event, qh);
        }
    }
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: &WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: &WlSurface,
        _: u32,
    ) {
    }

    fn press_key(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.key_event(event.keysym, false, qh);
    }

    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }

    fn release_key(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.key_event(event.keysym, true, qh);
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
        self.selector.set_shift(modifiers.shift);
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

delegate_registry!(App);
smithay_client_toolkit::delegate_dispatch2!(App);
delegate_noop!(App: WpViewporter);
delegate_noop!(App: WpViewport);
delegate_noop!(App: WpSinglePixelBufferManagerV1);
delegate_noop!(App: WlShmPool);
delegate_noop!(App: ignore WlBuffer);
