//! The `--delay` countdown (PLAN.md §4.2): a small ring on the output the
//! user last interacted with (the one under the pointer), counting down to
//! the freeze.
//!
//! The ring is an Overlay-layer surface the compositor centres on the output
//! of its choosing (layer-shell picks the one the user most recently
//! interacted with when given none). It takes no input, so the user can keep
//! working, open a menu, hover a tooltip, until zero. At zero it turns
//! transparent and waits for the compositor to present that, so the freeze
//! that follows does not catch it.

use std::time::{Duration, Instant};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    dispatch2::Dispatch2,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{
            KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use wayland_client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{
        wl_callback::{self, WlCallback},
        wl_output::{Transform, WlOutput},
        wl_shm,
        wl_surface::WlSurface,
    },
};

use crate::chrome::Fonts;
use crate::geom::Rect;
use crate::overlay::{Options, OverlayError};
use crate::theme::Theme;

/// The ring's diameter in logical pixels.
const SIZE: u32 = 96;
/// How long to wait for the compositor to present the cleared ring.
const CLEAR_TIMEOUT: Duration = Duration::from_millis(500);

/// The ring's colours, as straight (not premultiplied) RGBA.
///
/// These are the Overlay's chrome tokens: `surface` behind, `muted` for the
/// track, `accent` for the time left. They come from the same [`Options`] as
/// the Overlay, so the ring follows whatever look the Overlay has.
#[derive(Debug, Clone)]
struct RingPalette {
    surface: [u8; 4],
    muted: [u8; 4],
    accent: [u8; 4],
    /// For the seconds label's surface, text colour, font and radius.
    theme: Theme,
}

impl RingPalette {
    fn of(options: &Options) -> RingPalette {
        let theme = &options.theme;
        let straight = |c: hoarder_config::Colour| [c.r, c.g, c.b, c.a];
        // The track is the muted colour at about a fifth of its opacity, so
        // the arc for the time left stands out against it.
        let muted = theme.muted;
        RingPalette {
            surface: straight(theme.surface),
            muted: [muted.r, muted.g, muted.b, (muted.a as u32 * 56 / 255) as u8],
            accent: straight(theme.accent),
            theme: theme.clone(),
        }
    }
}

/// Show the countdown ring for `delay`, then take it off the screen. Returns
/// once the compositor has presented a frame without it, ready to freeze.
/// `options` are the Overlay's, for its look.
pub fn run(delay: Duration, options: &Options) -> Result<(), OverlayError> {
    let failed = |e: &dyn std::fmt::Display| OverlayError::Failed(e.to_string());
    let deadline = Instant::now() + delay;
    let conn = Connection::connect_to_env().map_err(|e| failed(&e))?;
    let (globals, mut queue) = registry_queue_init::<Countdown>(&conn).map_err(|e| failed(&e))?;
    let qh = queue.handle();
    let missing = |what: &str, e: &dyn std::fmt::Display| {
        OverlayError::Unsupported(format!(
            "the compositor does not offer {what}, which the countdown needs: {e}"
        ))
    };
    let compositor =
        CompositorState::bind(&globals, &qh).map_err(|e| missing("wl_compositor", &e))?;
    let layer_shell =
        LayerShell::bind(&globals, &qh).map_err(|e| missing("wlr-layer-shell", &e))?;
    let shm = Shm::bind(&globals, &qh).map_err(|e| missing("wl_shm", &e))?;

    let surface = compositor.create_surface(&qh);
    let layer = layer_shell.create_layer_surface(
        &qh,
        surface,
        Layer::Overlay,
        Some("hoarder-countdown"),
        None,
    );
    layer.set_size(SIZE, SIZE);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    // Clicks go through to whatever is below.
    let region = compositor.wl_compositor().create_region(&qh, ());
    layer.set_input_region(Some(&region));
    region.destroy();
    layer.commit();

    let mut app = Countdown {
        registry: RegistryState::new(&globals),
        outputs: OutputState::new(&globals, &qh),
        pool: SlotPool::new((SIZE * SIZE * 4) as usize, &shm).map_err(|e| failed(&e))?,
        shm,
        layer,
        fonts: Fonts::load(),
        palette: RingPalette::of(options),
        scale: 1,
        configured: false,
        frame_pending: false,
        cleared: Cleared::No,
        delay,
        deadline,
    };
    let ticking = crate::wait::dispatch_until(&mut queue, &mut app, deadline, |_| false);
    ticking.map_err(|e| failed(&e))?;

    app.clear(&qh).map_err(|e| failed(&e))?;
    let shown =
        crate::wait::dispatch_until(&mut queue, &mut app, Instant::now() + CLEAR_TIMEOUT, |a| {
            a.cleared == Cleared::Shown || !a.configured
        })
        .map_err(|e| failed(&e))?;
    if !shown {
        eprintln!("hoarder: the compositor did not confirm the countdown was hidden");
    }
    drop(app);
    conn.roundtrip().ok();
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cleared {
    No,
    Committed,
    Shown,
}

struct Countdown {
    registry: RegistryState,
    outputs: OutputState,
    shm: Shm,
    pool: SlotPool,
    layer: LayerSurface,
    fonts: Fonts,
    palette: RingPalette,
    /// Buffer scale of the output the ring is on.
    scale: i32,
    configured: bool,
    frame_pending: bool,
    cleared: Cleared,
    delay: Duration,
    deadline: Instant,
}

/// A frame callback; `clear` marks the one for the transparent commit.
struct Tick {
    clear: bool,
}

impl Dispatch2<WlCallback, Countdown> for Tick {
    fn event(
        &self,
        app: &mut Countdown,
        _: &WlCallback,
        event: wl_callback::Event,
        _: &Connection,
        qh: &QueueHandle<Countdown>,
    ) {
        if !matches!(event, wl_callback::Event::Done { .. }) {
            return;
        }
        if self.clear {
            app.cleared = Cleared::Shown;
        } else {
            app.frame_pending = false;
            app.draw(qh);
        }
    }
}

impl Countdown {
    /// Draw the ring as it stands now, if the compositor wants a frame.
    fn draw(&mut self, qh: &QueueHandle<Countdown>) {
        if !self.configured || self.frame_pending || self.cleared != Cleared::No {
            return;
        }
        let left = self.deadline.saturating_duration_since(Instant::now());
        let fraction = left.as_secs_f64() / self.delay.as_secs_f64().max(f64::EPSILON);
        let seconds = left.as_secs_f64().ceil() as u32;
        let (scale, palette) = (self.scale, self.palette.clone());
        let side = SIZE * scale as u32;
        if let Err(e) = self.commit(qh, side, false, |canvas, fonts| {
            paint_ring(
                canvas,
                side,
                scale as f64,
                fraction,
                seconds,
                &palette,
                fonts,
            )
        }) {
            eprintln!("hoarder: cannot draw the countdown: {e}");
        }
    }

    /// Commit a fully transparent buffer and ask to hear when it is shown.
    fn clear(&mut self, qh: &QueueHandle<Countdown>) -> Result<(), String> {
        if !self.configured {
            return Ok(());
        }
        let side = SIZE * self.scale as u32;
        self.commit(qh, side, true, |canvas, _| canvas.fill(0))?;
        self.cleared = Cleared::Committed;
        Ok(())
    }

    fn commit(
        &mut self,
        qh: &QueueHandle<Countdown>,
        side: u32,
        clear: bool,
        paint: impl FnOnce(&mut [u8], &mut Fonts),
    ) -> Result<(), String> {
        let (buffer, canvas) = self
            .pool
            .create_buffer(
                side as i32,
                side as i32,
                side as i32 * 4,
                wl_shm::Format::Argb8888,
            )
            .map_err(|e| e.to_string())?;
        paint(canvas, &mut self.fonts);
        let surface = self.layer.wl_surface();
        surface.set_buffer_scale(self.scale);
        surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        surface.frame(qh, Tick { clear });
        buffer.attach_to(surface).map_err(|e| e.to_string())?;
        self.layer.commit();
        self.frame_pending = true;
        Ok(())
    }
}

fn rgba([r, g, b, a]: [u8; 4]) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(r, g, b, a)
}

/// Paint the ring into `canvas` (`side`×`side` `Argb8888`): a dark disc, a
/// faint track, an accent arc for the `fraction` of time left and the
/// whole `seconds` left in the middle.
fn paint_ring(
    canvas: &mut [u8],
    side: u32,
    scale: f64,
    fraction: f64,
    seconds: u32,
    palette: &RingPalette,
    fonts: &mut Fonts,
) {
    use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};
    let mut pixmap = Pixmap::new(side, side).expect("non-empty ring");
    let s = side as f32;
    let c = s / 2.0;
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    paint.set_color(rgba(palette.surface));
    if let Some(disc) = PathBuilder::from_circle(c, c, c - 1.0) {
        pixmap.fill_path(
            &disc,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    let width = 4.0 * scale as f32;
    let r = c - width * 1.5;
    let stroke = Stroke {
        width,
        line_cap: tiny_skia::LineCap::Round,
        ..Stroke::default()
    };
    paint.set_color(rgba(palette.muted));
    if let Some(track) = PathBuilder::from_circle(c, c, r) {
        pixmap.stroke_path(&track, &paint, &stroke, Transform::identity(), None);
    }
    let fraction = fraction.clamp(0.0, 1.0) as f32;
    if fraction > 0.0 {
        let mut pb = PathBuilder::new();
        let steps = (fraction * 96.0).ceil().max(1.0) as u32;
        for i in 0..=steps {
            // Clockwise from twelve o'clock, shrinking as time runs out.
            let a = -std::f32::consts::FRAC_PI_2
                + fraction * std::f32::consts::TAU * i as f32 / steps as f32;
            let (x, y) = (c + r * a.cos(), c + r * a.sin());
            if i == 0 {
                pb.move_to(x, y);
            } else {
                pb.line_to(x, y);
            }
        }
        if let Some(arc) = pb.finish() {
            paint.set_color(rgba(palette.accent));
            pixmap.stroke_path(&arc, &paint, &stroke, Transform::identity(), None);
        }
    }
    // tiny-skia is RGBA; Argb8888 is BGRA in memory.
    let pixels = pixmap.data();
    for (dst, src) in canvas
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(pixels.as_chunks::<4>().0)
    {
        *dst = [src[2], src[1], src[0], src[3]];
    }
    let mut label = fonts.label(&seconds.to_string(), scale, 0, 0, &palette.theme);
    label.rect.x = (side as i32 - label.rect.w) / 2;
    label.rect.y = (side as i32 - label.rect.h) / 2;
    label.blend_into(canvas, side, Rect::new(0, 0, side as i32, side as i32));
}

impl CompositorHandler for Countdown {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &WlSurface,
        factor: i32,
    ) {
        if factor != self.scale && factor > 0 {
            self.scale = factor;
            self.draw(qh);
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: u32) {}

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

impl LayerShellHandler for Countdown {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        // The output went away; the countdown carries on without a ring.
        self.configured = false;
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &LayerSurface,
        _: LayerSurfaceConfigure,
        _: u32,
    ) {
        self.configured = true;
        self.draw(qh);
    }
}

impl OutputHandler for Countdown {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
}

impl ShmHandler for Countdown {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for Countdown {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState];
}

smithay_client_toolkit::delegate_registry!(Countdown);
smithay_client_toolkit::delegate_dispatch2!(Countdown);
wayland_client::delegate_noop!(Countdown: wayland_client::protocol::wl_region::WlRegion);
wayland_client::delegate_noop!(Countdown: wayland_client::protocol::wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(Countdown: ignore wayland_client::protocol::wl_buffer::WlBuffer);
