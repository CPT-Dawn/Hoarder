//! The Overlay's floating layers (PLAN.md §4.2): the Loupe, the hint bar and
//! the cursor layer. Each one is a [`Sprite`] per output, so none of them
//! ever repaints the full-output chrome.
//!
//! [`Layers`] holds what they should show (fed by pointer, modifier and key
//! events, and by timers); [`ViewLayers::refresh`] brings one output's
//! sprites in line with it.

use std::time::{Duration, Instant};

use hoarder_wayland::FrozenFrame;
use smithay_client_toolkit::compositor::CompositorState;
use smithay_client_toolkit::{shm::slot::SlotPool, subcompositor::SubcompositorState};
use wayland_client::{QueueHandle, protocol::wl_surface::WlSurface};
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;

use crate::chrome::Fonts;
use crate::loupe;
use crate::overlay::App;
use crate::sprite::Sprite;
use crate::theme::Theme;

/// Stillness before the Loupe appears.
pub(crate) const LOUPE_DELAY: Duration = Duration::from_millis(300);
/// How long the hint bar stays before it fades.
pub(crate) const HINT_STAYS: Duration = Duration::from_millis(1500);
/// How long its fade takes, in [`HINT_FADE_STEPS`] steps.
const HINT_FADE: Duration = Duration::from_millis(240);
const HINT_FADE_STEPS: u32 = 8;
/// What the hint bar says.
pub(crate) const HINT_TEXT: &str = "drag · click window · O output · A all · C cursor · Esc";
/// Logical gap between the hint bar and the bottom of the output.
const HINT_MARGIN: f64 = 24.0;

/// What the layers should show, across all outputs.
#[derive(Debug)]
pub(crate) struct Layers {
    /// Alt is held: the Loupe shows at once.
    alt: bool,
    /// The Loupe always shows (`hoarder pick`).
    pinned: bool,
    /// The pointer: which view it is on, and where (view-local logical).
    pointer: Option<(usize, (f64, f64))>,
    /// When the pointer will have been still long enough for the Loupe.
    loupe_due: Option<Instant>,
    still: bool,
    /// When the hint bar appeared; `None` if it is not shown at all.
    hint_since: Option<Instant>,
    hint_wanted: bool,
    /// Whether the cursor layer is on.
    pub(crate) cursor: bool,
}

impl Layers {
    pub(crate) fn new(hint_bar: bool, cursor: bool) -> Layers {
        Layers {
            alt: false,
            pinned: false,
            pointer: None,
            loupe_due: None,
            still: false,
            hint_since: None,
            hint_wanted: hint_bar,
            cursor,
        }
    }

    /// The Overlay became visible: the hint bar's clock starts.
    pub(crate) fn visible(&mut self, now: Instant) {
        if self.hint_wanted && self.hint_since.is_none() {
            self.hint_since = Some(now);
        }
    }

    /// The pointer is at view-local logical `at` on view `view` (it moved or
    /// entered).
    pub(crate) fn pointer_at(&mut self, view: usize, at: (f64, f64), now: Instant) {
        self.pointer = Some((view, at));
        self.still = false;
        self.loupe_due = Some(now + LOUPE_DELAY);
    }

    /// The pointer left view `view`.
    pub(crate) fn pointer_left(&mut self, view: usize) {
        if self.pointer.is_some_and(|(v, _)| v == view) {
            self.pointer = None;
            self.loupe_due = None;
            self.still = false;
        }
    }

    /// Show the Loupe wherever the pointer is, still or not (`hoarder pick`).
    pub(crate) fn pin_loupe(&mut self) {
        self.pinned = true;
    }

    pub(crate) fn set_alt(&mut self, held: bool) {
        self.alt = held;
    }

    pub(crate) fn toggle_cursor(&mut self) {
        self.cursor = !self.cursor;
    }

    /// Let time pass: the Loupe's stillness timer.
    pub(crate) fn tick(&mut self, now: Instant) {
        if self.loupe_due.is_some_and(|due| due <= now) {
            self.loupe_due = None;
            self.still = true;
        }
    }

    /// The next time [`tick`](Self::tick) or a refresh has something to do.
    pub(crate) fn deadline(&self, now: Instant) -> Option<Instant> {
        let hint = self.hint_since.and_then(|since| {
            let fade_from = since + HINT_STAYS;
            if now < fade_from {
                return Some(fade_from);
            }
            let step = HINT_FADE / HINT_FADE_STEPS;
            let done = ((now - fade_from).as_nanos() / step.as_nanos()) as u32 + 1;
            (done <= HINT_FADE_STEPS).then(|| fade_from + step * done)
        });
        [self.loupe_due, hint].into_iter().flatten().min()
    }

    /// The hint bar's opacity out of [`HINT_FADE_STEPS`], 0 once it is gone.
    fn hint_opacity(&self, now: Instant) -> u32 {
        let Some(since) = self.hint_since else {
            return 0;
        };
        let fade_from = since + HINT_STAYS;
        if now < fade_from {
            return HINT_FADE_STEPS;
        }
        let step = HINT_FADE / HINT_FADE_STEPS;
        let done = ((now - fade_from).as_nanos() / step.as_nanos()) as u32;
        HINT_FADE_STEPS.saturating_sub(done)
    }

    /// Where the Loupe goes on `view`, if anywhere (view-local logical).
    fn loupe_on(&self, view: usize) -> Option<(f64, f64)> {
        match self.pointer {
            Some((v, at)) if v == view && (self.alt || self.still || self.pinned) => Some(at),
            _ => None,
        }
    }
}

/// What one output's layers need from the Overlay to draw.
pub(crate) struct Context<'a> {
    /// The Frozen Frame; none over the live desktop (`--live`), where there
    /// is neither a Loupe nor a cursor layer.
    pub frame: Option<&'a FrozenFrame>,
    /// Logical size of the output.
    pub logical: (f64, f64),
    /// Physical pixels per logical pixel.
    pub scale: f64,
    pub fonts: &'a mut Fonts,
    pub theme: &'a Theme,
    pub pool: &'a mut SlotPool,
}

/// One output's sprites and what each shows now.
#[derive(Debug)]
pub(crate) struct ViewLayers {
    loupe: Sprite,
    /// The physical pixel the Loupe shows, and its logical size.
    loupe_drawn: Option<((u32, u32), (i32, i32))>,
    hint: Sprite,
    hint_drawn: u32,
    /// The hint bar, rendered once at full opacity.
    hint_picture: Option<(loupe::Picture, (i32, i32))>,
    cursor: Sprite,
}

impl ViewLayers {
    /// Sprites over `root`; the cursor layer goes just above `frame`, so the
    /// Overlay dims it with the frame.
    pub(crate) fn new(
        root: &WlSurface,
        frame: Option<&WlSurface>,
        compositor: &CompositorState,
        subcompositor: &SubcompositorState,
        viewporter: &WpViewporter,
        qh: &QueueHandle<App>,
    ) -> ViewLayers {
        let sprite =
            |sibling| Sprite::new(root, sibling, compositor, subcompositor, viewporter, qh);
        ViewLayers {
            cursor: sprite(frame),
            hint: sprite(None),
            loupe: sprite(None),
            loupe_drawn: None,
            hint_drawn: 0,
            hint_picture: None,
        }
    }

    /// Bring view `index`'s sprites in line with `layers`. Returns whether
    /// the Overlay surface must commit for new positions to apply.
    pub(crate) fn refresh(
        &mut self,
        layers: &Layers,
        index: usize,
        now: Instant,
        cx: Context<'_>,
    ) -> std::io::Result<bool> {
        let mut commit = false;
        let s = cx.scale;
        let logical_of = |(w, h): (u32, u32)| {
            (
                (w as f64 / s).round().max(1.0) as i32,
                (h as f64 / s).round().max(1.0) as i32,
            )
        };

        // The cursor layer.
        let cursor = cx.frame.and_then(|f| f.cursor.as_ref());
        match (cursor, layers.cursor) {
            (Some(c), true) if !self.cursor.is_shown() => {
                let at = (
                    (c.x as f64 / s).floor() as i32,
                    (c.y as f64 / s).floor() as i32,
                );
                commit |= self.cursor.show(
                    cx.pool,
                    (c.width, c.height),
                    logical_of((c.width, c.height)),
                    at,
                    |canvas| canvas.copy_from_slice(&c.pixels),
                )?;
            }
            (_, false) => self.cursor.hide(),
            _ => {}
        }

        // The hint bar.
        let opacity = layers.hint_opacity(now);
        if opacity != self.hint_drawn {
            if opacity == 0 {
                self.hint.hide();
            } else {
                if self.hint_picture.is_none() {
                    let label = cx.fonts.label(HINT_TEXT, s, 0, 0, cx.theme);
                    let size = (label.rect.w as u32, label.rect.h as u32);
                    let mut pixels = vec![0; (size.0 * size.1 * 4) as usize];
                    label.blend_into(&mut pixels, size.0, label.rect);
                    let (lw, lh) = logical_of(size);
                    let at = (
                        ((cx.logical.0 - lw as f64) / 2.0).round() as i32,
                        (cx.logical.1 - HINT_MARGIN - lh as f64).round() as i32,
                    );
                    self.hint_picture = Some((loupe::Picture { size, pixels }, at));
                }
                let (picture, at) = self.hint_picture.as_ref().expect("rendered above");
                commit |= self.hint.show(
                    cx.pool,
                    picture.size,
                    logical_of(picture.size),
                    *at,
                    |canvas| {
                        for (d, &v) in canvas.iter_mut().zip(&picture.pixels) {
                            *d = (v as u32 * opacity / HINT_FADE_STEPS) as u8;
                        }
                    },
                )?;
            }
            self.hint_drawn = opacity;
        }

        // The Loupe.
        match layers.loupe_on(index).zip(cx.frame) {
            Some((at, frame)) => {
                let pixel = loupe::pixel_under(at, s, frame.upright_size());
                let unchanged =
                    self.loupe.is_shown() && self.loupe_drawn.is_some_and(|(p, _)| p == pixel);
                if unchanged {
                    let (_, size) = self.loupe_drawn.expect("checked");
                    let place = loupe::place(at, size, cx.logical);
                    commit |= self.loupe.move_to(place);
                } else {
                    let picture = loupe::render(frame, pixel, s, cx.fonts, cx.theme);
                    let size = logical_of(picture.size);
                    let place = loupe::place(at, size, cx.logical);
                    commit |= self
                        .loupe
                        .show(cx.pool, picture.size, size, place, |canvas| {
                            canvas.copy_from_slice(&picture.pixels)
                        })?;
                    self.loupe_drawn = Some((pixel, size));
                }
            }
            None => self.loupe.hide(),
        }
        Ok(commit)
    }

    /// Hide everything, e.g. before the live desktop is captured.
    pub(crate) fn hide(&mut self) {
        self.loupe.hide();
        self.hint.hide();
        self.cursor.hide();
    }
}
