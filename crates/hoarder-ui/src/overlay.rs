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
//!
//! With `--live` ([`run_live`]) there is no Frozen Frame: the chrome is drawn
//! over the live desktop, and before returning the Overlay turns transparent
//! and waits for the compositor to present that, so a Capture taken next
//! holds none of its pixels.

use std::cell::Cell;
use std::io::Write as _;
use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use hoarder_wayland::{Frozen, FrozenFrame, Window};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
    delegate_registry,
    dispatch2::Dispatch2,
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
    Connection, QueueHandle, delegate_noop,
    protocol::{
        wl_buffer::WlBuffer, wl_callback::WlCallback, wl_keyboard::WlKeyboard, wl_output::WlOutput,
        wl_pointer::WlPointer, wl_seat::WlSeat, wl_shm, wl_shm_pool::WlShmPool,
        wl_subsurface::WlSubsurface, wl_surface::WlSurface,
    },
};
use wayland_protocols::wp::{
    cursor_shape::v1::client::wp_cursor_shape_device_v1::{Shape, WpCursorShapeDeviceV1},
    fractional_scale::v1::client::{
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        wp_fractional_scale_v1::{self, WpFractionalScaleV1},
    },
    single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1,
    viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
};

use crate::chrome::{CLEAR, DIM, Fonts, Highlight, Palette, Scene};
use crate::geom::Rect;
use crate::layers::{Context, Layers, ViewLayers};
use crate::select::{Action, Key, Selector, Tool};
use crate::snap::Hover;
use crate::surface::{self, Error, Layered, failed, timing};
use crate::theme::Theme;

pub use crate::surface::TIMINGS_ENV;

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
    /// A Shift+click on the highlighted window: its own buffer, not what
    /// the Frozen Frame shows of it.
    WindowBuffer(Window),
    /// `A`.
    AllOutputs,
    /// With [`Options::pick`], a click: physical pixel `(x, y)` of the
    /// upright Frozen Frame of the Output named `output`.
    Pixel { output: String, x: u32, y: u32 },
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
    /// What the chrome and labels are drawn in (Midnight Bloom by default).
    pub theme: Theme,
    /// Show the hint bar (it retires after 20 uses; the caller counts).
    pub hint_bar: bool,
    /// Whether the cursor layer is in the Capture (`--cursor`,
    /// `screenshot.cursor`). The Overlay flips it as the user presses `C`,
    /// so after [`run`] it holds the user's final choice.
    pub cursor: Cell<bool>,
    /// A selection cannot leave the output where its drag started
    /// (`capture.mixed_scale = "clamp"`). With `run_live`, only when the
    /// outputs differ in scale.
    pub clamp_to_output: bool,
    /// The windows hovering highlights and a click captures, topmost first,
    /// as a [`WindowGeometry`](hoarder_wayland::WindowGeometry) gives them.
    /// Empty without a provider: no highlighting.
    pub windows: Vec<Window>,
    /// Pick a pixel (`hoarder pick`): the Loupe always shows, a click ends
    /// the Overlay with [`Outcome::Pixel`], and there is no selection. Only
    /// Esc and right-click do anything else (they cancel).
    pub pick: bool,
}

/// How long to wait for the compositor to present the hidden Overlay before
/// capturing anyway.
const HIDE_TIMEOUT: Duration = Duration::from_millis(500);

/// Show the Overlay over `frozen` and wait for the user to pick a Target or
/// cancel, drawn in `options.theme`. `started` is when the user asked for the
/// Capture; with [`TIMINGS_ENV`] set, the time until the Overlay is visible is
/// printed.
pub fn run(frozen: &Frozen, started: Instant, options: &Options) -> Result<Outcome, Error> {
    timing(started, "frozen");
    show(Some(frozen), started, options)
}

/// Show the Overlay over the live desktop (`--live`) and wait for the user to
/// pick a Target or cancel. On return the compositor has presented a frame
/// without the Overlay, so the desktop can be captured as the user sees it.
pub fn run_live(started: Instant, options: &Options) -> Result<Outcome, Error> {
    show(None, started, options)
}

/// What one output's Overlay shows and where.
struct Screen<'a> {
    output: WlOutput,
    /// The output's name, e.g. `DP-1`.
    name: String,
    /// Layout position and size of the output (logical).
    logical: (f64, f64, f64, f64),
    /// Physical size of the output, upright.
    size: (u32, u32),
    /// The Frozen Frame to show under the chrome, or none for the live desktop.
    frame: Option<&'a FrozenFrame>,
}

fn show(frozen: Option<&Frozen>, started: Instant, options: &Options) -> Result<Outcome, Error> {
    let Layered {
        globals,
        mut queue,
        qh,
        compositor,
        layer_shell,
        shm,
        ..
    } = surface::connect::<App>("Overlay")?;
    let missing = |what: &'static str| move |e| surface::missing("Overlay", what, e);
    let subcompositor = SubcompositorState::bind(compositor.wl_compositor().clone(), &globals, &qh)
        .map_err(missing("wl_subcompositor"))?;
    let viewporter: WpViewporter = globals
        .bind(&qh, 1..=1, ())
        .map_err(missing("wp_viewporter"))?;
    let cursor_shapes = CursorShapeManager::bind(&globals, &qh).ok();
    let single_pixel: Option<WpSinglePixelBufferManagerV1> = globals.bind(&qh, 1..=1, ()).ok();
    // The scale the compositor prefers for each output, for the chrome's
    // text and strokes; the buffers are physical size either way.
    let fractional: Option<WpFractionalScaleManagerV1> = globals.bind(&qh, 1..=1, ()).ok();

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
        theme: options.theme.clone(),
        layers: Layers::new(options.hint_bar, options.cursor.get()),
        selector: Selector::new(options.confirm, options.last_region),
        pointer_at: None,
        hover: Hover::new(options.windows.clone()),
        outcome: None,
        pick: options.pick,
        started,
        visible_reported: false,
        single_pixel,
        icons: Default::default(),
    };
    if options.pick {
        app.layers.pin_loupe();
    }
    // Learn the outputs' names so each Frozen Frame lands on its own output.
    queue.roundtrip(&mut app).map_err(failed)?;

    let screens: Vec<Screen> = match frozen {
        Some(frozen) => frozen
            .frames
            .iter()
            .map(|frame| {
                let output = app
                    .outputs
                    .outputs()
                    .find(|o| {
                        app.outputs.info(o).and_then(|i| i.name).as_deref() == Some(&frame.output)
                    })
                    .ok_or_else(|| failed(format!("output {} went away", frame.output)))?;
                let l = frame.logical;
                Ok(Screen {
                    output,
                    name: frame.output.clone(),
                    logical: (l.x as f64, l.y as f64, l.width as f64, l.height as f64),
                    size: frame.upright_size(),
                    frame: Some(frame),
                })
            })
            .collect::<Result<_, Error>>()?,
        None => {
            // xdg-output's logical geometry arrives a roundtrip later.
            queue.roundtrip(&mut app).map_err(failed)?;
            app.outputs
                .outputs()
                .filter_map(|output| live_screen(output, &app.outputs))
                .collect()
        }
    };
    if screens.is_empty() {
        return Err(failed("the compositor has no outputs"));
    }

    for screen in &screens {
        // One black pixel at DIM's alpha, shown at full strength from the
        // first frame so the Overlay is unmistakably up (PLAN.md §4.2).
        let alpha = DIM[3] as u32;
        let dim = match &app.single_pixel {
            // Premultiplied, full range.
            Some(m) => Ok(m.create_u32_rgba_buffer(0, 0, 0, alpha * 0x0101_0101, &qh, ())),
            None => pixel_buffer([0, 0, 0, DIM[3]], &app.shm, &qh),
        }
        .map_err(failed)?;
        let view = View::new(
            screen,
            &compositor,
            &subcompositor,
            &layer_shell,
            &viewporter,
            &app.shm,
            dim,
            Palette::of(&options.theme),
            &qh,
        )
        .map_err(failed)?;
        let index = app.views.len();
        app.views.push(view);
        if let Some(m) = &fractional {
            let surface = app.views[index].layer.wl_surface().clone();
            app.views[index].fractional = Some(m.get_fractional_scale(&surface, &qh, index));
        }
    }
    // Live, the caller had no Frozen Frames to judge the scales by, so the
    // outputs' own scales decide whether a selection must stay on one.
    let mixed = || {
        app.views
            .windows(2)
            .any(|w| w[0].scale().0 != w[1].scale().0)
    };
    if options.clamp_to_output && (frozen.is_some() || mixed()) {
        app.selector
            .clamp_to(app.views.iter().map(|v| v.logical).collect());
    }
    timing(started, "surfaces_created");

    loop {
        queue.dispatch_pending(&mut app).map_err(failed)?;
        let now = Instant::now();
        app.layers.tick(now);
        app.refresh_layers(frozen, now);
        if app.outcome.is_some() {
            break;
        }
        // Sleep until the compositor speaks or a layer's timer is due.
        let deadline = app.layers.deadline(now);
        hoarder_wayland::wait::dispatch_once(&mut queue, deadline).map_err(failed)?;
    }
    options.cursor.set(app.layers.cursor);
    for view in &mut app.views {
        view.layers.hide();
    }
    let outcome = app.outcome.take().expect("loop ended on an outcome");
    if frozen.is_none() && outcome != Outcome::Cancelled {
        // The caller captures the live desktop next: it must not see us.
        app.hide(&qh).map_err(failed)?;
        let deadline = Instant::now() + HIDE_TIMEOUT;
        let hidden = hoarder_wayland::wait::dispatch_until(&mut queue, &mut app, deadline, |a| {
            a.views.iter().all(|v| v.hidden == Hidden::Shown)
        })
        .map_err(failed)?;
        if !hidden {
            eprintln!("hoarder: the compositor did not confirm the Overlay was hidden");
        }
        timing(started, "overlay_hidden");
    }
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
    /// The Loupe, hint bar and cursor layer; dropped before the surfaces
    /// they float over.
    layers: ViewLayers,
    /// Physical size of the chrome buffer: the upright Frozen Frame size.
    size: (u32, u32),
    layer: LayerSurface,
    root_viewport: WpViewport,
    /// The Frozen Frame below the chrome; none over the live desktop.
    frame: Option<FrameLayer>,
    /// Until there is a selection, the chrome is this one dim pixel stretched
    /// over the output: nothing to paint, nothing to upload.
    dim_buffer: WlBuffer,
    /// Whether the dim pixel is committed.
    dim_shown: bool,
    /// `wp_fractional_scale_v1` for the chrome, and the scale it prefers.
    fractional: Option<WpFractionalScaleV1>,
    preferred_scale: Option<f64>,
    palette: Palette,
    buffers: Vec<ChromeBuffer>,
    drawn: Scene,
    wanted: Scene,
    configured: bool,
    frame_pending: bool,
    presented: bool,
    hidden: Hidden,
    /// What the chrome shows once hidden: one transparent pixel.
    clear_buffer: Option<WlBuffer>,
}

/// Whether a View has been made transparent for a live Capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hidden {
    No,
    /// A transparent buffer is committed; its frame callback is pending.
    Committed,
    /// The compositor has presented the transparent Overlay.
    Shown,
}

/// A Frozen Frame shown 1:1 in a subsurface below the chrome.
struct FrameLayer {
    surface: WlSurface,
    subsurface: WlSubsurface,
    viewport: WpViewport,
    buffer: WlBuffer,
}

impl Drop for FrameLayer {
    fn drop(&mut self) {
        self.viewport.destroy();
        self.subsurface.destroy();
        self.surface.destroy();
        self.buffer.destroy();
    }
}

/// A chrome buffer and the areas that went stale while it was not on screen.
struct ChromeBuffer {
    buffer: Buffer,
    stale: Vec<Rect>,
}

impl View {
    #[allow(clippy::too_many_arguments)]
    fn new(
        screen: &Screen,
        compositor: &CompositorState,
        subcompositor: &SubcompositorState,
        layer_shell: &LayerShell,
        viewporter: &WpViewporter,
        shm: &Shm,
        dim_buffer: WlBuffer,
        palette: Palette,
        qh: &QueueHandle<App>,
    ) -> std::io::Result<View> {
        let root = compositor.create_surface(qh);
        let layer = layer_shell.create_layer_surface(
            qh,
            root,
            Layer::Overlay,
            Some("hoarder-overlay"),
            Some(&screen.output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        let root_viewport = viewporter.get_viewport(layer.wl_surface(), qh, ());

        let frame = match screen.frame {
            Some(frame) => {
                let (subsurface, surface) =
                    subcompositor.create_subsurface(layer.wl_surface().clone(), qh);
                subsurface.place_below(layer.wl_surface());
                subsurface.set_position(0, 0);
                let viewport = viewporter.get_viewport(&surface, qh, ());
                let buffer = frozen_buffer(frame, shm, qh)?;
                let l = frame.logical;
                surface.set_buffer_transform(frame.transform.to_wl());
                viewport.set_destination(l.width as i32, l.height as i32);
                surface.attach(Some(&buffer), 0, 0);
                surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
                surface.commit();
                Some(FrameLayer {
                    surface,
                    subsurface,
                    viewport,
                    buffer,
                })
            }
            None => None,
        };
        let layers = ViewLayers::new(
            layer.wl_surface(),
            frame.as_ref().map(|f| &f.surface),
            compositor,
            subcompositor,
            viewporter,
            qh,
        );
        layer.commit();

        Ok(View {
            output: screen.name.clone(),
            logical: screen.logical,
            layers,
            size: screen.size,
            layer,
            root_viewport,
            frame,
            dim_buffer,
            dim_shown: false,
            fractional: None,
            preferred_scale: None,
            palette,
            buffers: Vec::new(),
            drawn: Scene::default(),
            wanted: Scene::default(),
            configured: false,
            frame_pending: false,
            presented: false,
            hidden: Hidden::No,
            clear_buffer: None,
        })
    }

    /// Physical pixels per logical pixel, horizontally and vertically.
    fn scale(&self) -> (f64, f64) {
        (
            self.size.0 as f64 / self.logical.2.max(1.0),
            self.size.1 as f64 / self.logical.3.max(1.0),
        )
    }

    /// The scale to size the chrome's text, handles and strokes by: what
    /// the compositor prefers (`wp_fractional_scale_v1`), or else what the
    /// buffer's size says.
    fn ui_scale(&self) -> f64 {
        self.preferred_scale.unwrap_or(self.scale().0)
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
        if !self.configured || self.frame_pending || self.hidden != Hidden::No {
            return Ok(());
        }
        let first = self.buffers.is_empty();
        if first && self.wanted == Scene::default() {
            // Nothing to paint yet: the dim pixel, stretched over the output.
            if !self.dim_shown {
                let surface = self.layer.wl_surface();
                self.root_viewport
                    .set_destination(self.logical.2 as i32, self.logical.3 as i32);
                surface.attach(Some(&self.dim_buffer), 0, 0);
                surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
                surface.frame(qh, FrameCallbackData(surface.clone()));
                self.layer.commit();
                self.frame_pending = true;
                self.dim_shown = true;
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
            self.wanted.paint(canvas, w, h, area, self.palette);
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
        self.frame.take();
        self.dim_buffer.destroy();
        if let Some(f) = self.fractional.take() {
            f.destroy();
        }
        if let Some(b) = self.clear_buffer.take() {
            b.destroy();
        }
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

pub(crate) struct App {
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
    theme: Theme,
    /// What the Loupe, the hint bar and the cursor layer show.
    layers: Layers,
    selector: Selector,
    /// Where the pointer last was, in global logical coordinates.
    pointer_at: Option<(f64, f64)>,
    hover: Hover,
    outcome: Option<Outcome>,
    /// [`Options::pick`].
    pick: bool,
    started: Instant,
    visible_reported: bool,
    single_pixel: Option<WpSinglePixelBufferManagerV1>,
    /// App icons for window labels, by app_id, looked up once each.
    icons: std::collections::HashMap<String, Option<tiny_skia::Pixmap>>,
}

impl App {
    /// Bring every output's Loupe, hint bar and cursor layer up to date.
    fn refresh_layers(&mut self, frozen: Option<&Frozen>, now: Instant) {
        for (i, view) in self.views.iter_mut().enumerate() {
            if !view.presented {
                continue;
            }
            let cx = Context {
                frame: frozen.map(|f| &f.frames[i]),
                logical: (view.logical.2, view.logical.3),
                scale: view.ui_scale(),
                fonts: &mut self.fonts,
                theme: &self.theme,
                pool: &mut self.pool,
            };
            match view.layers.refresh(&self.layers, i, now, cx) {
                Ok(true) => view.layer.commit(),
                Ok(false) => {}
                Err(e) => {
                    eprintln!("hoarder: cannot draw the Overlay: {e}");
                    self.outcome = Some(Outcome::Cancelled);
                }
            }
        }
    }

    /// Make every View transparent and ask to hear when that is on screen.
    fn hide(&mut self, qh: &QueueHandle<App>) -> std::io::Result<()> {
        for (i, view) in self.views.iter_mut().enumerate() {
            let clear = match &self.single_pixel {
                Some(m) => m.create_u32_rgba_buffer(0, 0, 0, 0, qh, ()),
                None => pixel_buffer(CLEAR, &self.shm, qh)?,
            };
            view.frame.take();
            let surface = view.layer.wl_surface();
            view.root_viewport
                .set_destination(view.logical.2 as i32, view.logical.3 as i32);
            surface.attach(Some(&clear), 0, 0);
            surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
            surface.frame(qh, HiddenFrame(i));
            view.layer.commit();
            view.hidden = Hidden::Committed;
            view.clear_buffer = Some(clear);
        }
        Ok(())
    }

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
            Action::ToggleCursor => {
                self.layers.toggle_cursor();
                None
            }
            Action::Cancel => Some(Outcome::Cancelled),
        };
    }

    /// A key went down (or up, for `released`).
    fn key_event(&mut self, keysym: Keysym, released: bool, qh: &QueueHandle<App>) {
        if self.pick {
            if keysym == Keysym::Escape && !released {
                self.outcome = Some(Outcome::Cancelled);
            }
            return;
        }
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
        if key == Key::Cursor {
            if !released {
                self.layers.toggle_cursor();
            }
            return;
        }
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
        if let Some(action) = self.selector.key(key, released, px) {
            self.act(action);
        }
        if self.outcome.is_none() {
            self.update(qh);
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
                            let sx = view.ui_scale();
                            let mut l = self.fonts.label(&text, sx, sel.x, 0, &self.theme);
                            l.rect.y = label_y(sel, l.rect.h, view.size.1);
                            Some(l)
                        }
                    }
                }
                _ => None,
            };
            let handles = match selection {
                Some(sel) if refining => handles(sel, view.ui_scale()),
                _ => Vec::new(),
            };
            let toolbar = match (&label, self.selector.tool()) {
                (Some(size), Some(focused)) => {
                    let (scale, start) = (view.ui_scale(), size.rect);
                    self.toolbar(focused, scale, start)
                }
                _ => Vec::new(),
            };
            let view = &mut self.views[i];
            view.wanted = Scene {
                window,
                selection,
                label,
                handles,
                toolbar,
            };
            if let Err(e) = view.redraw(&mut self.pool, qh) {
                self.outcome = Some(Outcome::Cancelled);
                eprintln!("hoarder: cannot draw the Overlay: {e}");
            }
        }
    }

    /// Refine's toolbar pills, in [`Tool::ALL`]'s order, on the row of the
    /// size label `after` and to its right; `focused` in the accent.
    fn toolbar(&mut self, focused: Tool, scale: f64, after: Rect) -> Vec<crate::chrome::Label> {
        let gap = (6.0 * scale).round() as i32;
        let mut x = after.right() + gap;
        let lit = Theme {
            surface: self.theme.accent,
            text: hoarder_config::Colour {
                a: 255,
                ..self.theme.surface
            },
            ..self.theme.clone()
        };
        Tool::ALL
            .iter()
            .map(|&tool| {
                let text = match tool {
                    Tool::Capture => "Capture",
                    Tool::Cursor if self.layers.cursor => "Cursor: on",
                    Tool::Cursor => "Cursor: off",
                    Tool::Cancel => "Cancel",
                };
                let theme = if tool == focused { &lit } else { &self.theme };
                let pill = self.fonts.label(text, scale, x, after.y, theme);
                x = pill.rect.right() + gap;
                pill
            })
            .collect()
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
        let icon = self
            .icons
            .entry(window.app_id.clone())
            .or_insert_with(|| crate::icon::lookup(&window.app_id));
        let label = (!text.is_empty()).then(|| {
            let sx = view.ui_scale();
            let mut l = self.fonts.window_label(
                &ellipsize(text, 80),
                icon.as_ref(),
                &window.app_id,
                sx,
                &self.theme,
            );
            // Centred on the part of the window this output shows.
            l.rect.x = shown.x + ((shown.w - l.rect.w) / 2).max(0);
            l.rect.y = shown.y + ((shown.h - l.rect.h) / 2).max(0);
            l
        });
        Some(Highlight { index, rect, label })
    }

    /// A pointer event while picking a pixel on view `i`.
    fn pick_event(&mut self, i: usize, event: &PointerEvent) {
        match event.kind {
            PointerEventKind::Enter { serial } => {
                if let Some(device) = &self.cursor_device {
                    device.set_shape(serial, Shape::Crosshair);
                }
            }
            PointerEventKind::Release {
                button: BTN_LEFT, ..
            } => {
                let view = &self.views[i];
                let (x, y) = crate::loupe::pixel_under(event.position, view.scale().0, view.size);
                self.outcome = Some(Outcome::Pixel {
                    output: view.output.clone(),
                    x,
                    y,
                });
            }
            PointerEventKind::Press {
                button: BTN_RIGHT, ..
            } => self.outcome = Some(Outcome::Cancelled),
            _ => {}
        }
    }

    fn pointer_event(&mut self, event: &PointerEvent, qh: &QueueHandle<App>) {
        let Some(i) = self.view_of(&event.surface) else {
            return;
        };
        let (ox, oy, _, _) = self.views[i].logical;
        let at = (ox + event.position.0, oy + event.position.1);
        self.pointer_at = Some(at);
        match event.kind {
            PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                self.layers.pointer_at(i, event.position, Instant::now());
            }
            PointerEventKind::Leave { .. } => self.layers.pointer_left(i),
            _ => {}
        }
        if self.pick {
            return self.pick_event(i, event);
        }
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
                // A click on a Refine toolbar pill runs its action.
                let view = &self.views[i];
                let (sx, sy) = view.scale();
                let (px, py) = (
                    (event.position.0 * sx) as i32,
                    (event.position.1 * sy) as i32,
                );
                let hit = view.drawn.toolbar.iter().position(|pill| {
                    let r = pill.rect;
                    px >= r.x && px < r.right() && py >= r.y && py < r.bottom()
                });
                if let Some(tool) = hit.and_then(|j| Tool::ALL.get(j))
                    && let Some(action) = self.selector.run_tool(*tool)
                {
                    self.act(action);
                    if self.outcome.is_none() {
                        self.update(qh);
                    }
                    return;
                }
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
                    let window = self.hover.window(w).clone();
                    self.outcome = Some(if self.selector.shift_held() {
                        Outcome::WindowBuffer(window)
                    } else {
                        Outcome::Window(window)
                    });
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

/// The frame callback of a View's transparent commit.
struct HiddenFrame(usize);

impl Dispatch2<WlCallback, App> for HiddenFrame {
    fn event(
        &self,
        app: &mut App,
        _: &WlCallback,
        _: wayland_client::protocol::wl_callback::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
        if let Some(view) = app.views.get_mut(self.0) {
            view.hidden = Hidden::Shown;
        }
    }
}

/// A live output as the Overlay covers it, from what the compositor reports.
fn live_screen(output: WlOutput, outputs: &OutputState) -> Option<Screen<'static>> {
    use wayland_client::protocol::wl_output::Transform as T;
    let info = outputs.info(&output)?;
    let mode = info.modes.iter().find(|m| m.current)?;
    let (w, h) = (mode.dimensions.0 as u32, mode.dimensions.1 as u32);
    let size = match info.transform {
        T::_90 | T::_270 | T::Flipped90 | T::Flipped270 => (h, w),
        _ => (w, h),
    };
    let (x, y) = info.logical_position.unwrap_or(info.location);
    let scale = info.scale_factor.max(1);
    let (lw, lh) = info
        .logical_size
        .unwrap_or((size.0 as i32 / scale, size.1 as i32 / scale));
    Some(Screen {
        output,
        name: info.name.clone().unwrap_or_default(),
        logical: (x as f64, y as f64, lw as f64, lh as f64),
        size,
        frame: None,
    })
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
        Keysym::c | Keysym::C => Key::Cursor,
        Keysym::Left | Keysym::KP_Left => Key::Left,
        Keysym::Right | Keysym::KP_Right => Key::Right,
        Keysym::Up | Keysym::KP_Up => Key::Up,
        Keysym::Down | Keysym::KP_Down => Key::Down,
        Keysym::Tab | Keysym::ISO_Left_Tab => Key::Tab,
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
            self.layers.visible(Instant::now());
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
        self.layers.set_alt(modifiers.alt);
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
delegate_noop!(App: wayland_client::protocol::wl_region::WlRegion);
delegate_noop!(App: WpSinglePixelBufferManagerV1);
delegate_noop!(App: WpFractionalScaleManagerV1);

impl wayland_client::Dispatch<WpFractionalScaleV1, usize> for App {
    fn event(
        app: &mut Self,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        &view: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event
            && let Some(v) = app.views.get_mut(view)
        {
            v.preferred_scale = Some(f64::from(scale) / 120.0);
        }
    }
}
delegate_noop!(App: WlShmPool);
delegate_noop!(App: ignore WlBuffer);
