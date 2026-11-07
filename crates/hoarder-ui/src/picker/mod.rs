//! The Picker (PLAN.md §6): a centred layer-shell panel listing History.
//! Typing searches it (with the filter prefixes `img:`, `txt:`, `file:`,
//! `pin:` and `app:<id>`), Up and Down select, Enter picks the selected
//! Clip, Shift+Enter picks it as plain text, Ctrl+1–9 pick a row at once,
//! Ctrl+P pins or unpins, Del deletes and Esc closes.
//!
//! The panel knows nothing of the Daemon: [`run`] reads Clips and full-text
//! hits through a [`History`], pins and deletes through it, and returns the
//! [`Pick`], after the panel is gone. Restoring it is the caller's business.

mod model;
mod paint;

use std::time::Instant;

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_registry,
    dispatch2::Dispatch2,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
    },
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
        wl_keyboard::WlKeyboard,
        wl_output::{Transform, WlOutput},
        wl_seat::WlSeat,
        wl_shm,
        wl_surface::WlSurface,
    },
};

use crate::chrome::Fonts;
use crate::overlay::{OverlayError, timing};
use crate::theme::Theme;
pub use hoarder_ipc::Kind;
use model::Model;

/// One Clip as the Picker lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    pub id: i64,
    pub kind: Kind,
    /// A line of text, an image's size or the file names; for a
    /// Concealed Clip, only dots.
    pub preview: String,
    /// The app focused when it was copied, if known.
    pub app: Option<String>,
    /// When it was last copied or restored, in ms since the Unix epoch.
    pub at_ms: Option<i64>,
    pub pinned: bool,
    /// Held by the Daemon in memory only; shown masked until Alt is held.
    pub concealed: bool,
}

/// Where the Picker reads History from.
pub trait History {
    /// Every Clip, most recently used first. Called once, when the panel
    /// is about to draw its first frame, so it can be fetched meanwhile.
    fn clips(&mut self) -> Result<Vec<Clip>, String>;

    /// The ids of Clips whose whole text contains `query`, most recently
    /// used first (full-text search, for long text whose match lies past
    /// its preview). Called as the query changes; a failure finds nothing.
    fn search(&mut self, query: &str) -> Vec<i64>;

    /// Pin Clip `id` or unpin it; whether that worked.
    fn pin(&mut self, id: i64, pinned: bool) -> bool;

    /// Delete Clip `id`; whether that worked.
    fn delete(&mut self, id: i64) -> bool;

    /// The text of Concealed Clip `id`. Called only while the user holds
    /// Alt, for the Concealed Clips on screen.
    fn reveal(&mut self, id: i64) -> Option<String>;

    /// What the preview pane shows for `clip`, a Clip that is not
    /// Concealed. Called once the first frame is up, as the selection
    /// moves.
    fn detail(&mut self, clip: &Clip) -> Detail;
}

/// A Clip in full, for the preview pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detail {
    /// Its text (or as much of it as fits).
    Text(String),
    /// An image in straight RGBA, `width × height` pixels, shown at its
    /// real pixel size.
    Image {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    /// The paths of a file list, shown as chips.
    Files(Vec<String>),
    /// Nothing beyond its preview line.
    Nothing,
}

/// What the user picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pick {
    /// The Clip to Restore.
    pub id: i64,
    /// Restore only its plain text (Shift+Enter).
    pub plain: bool,
}

/// How the Picker looks and reports.
#[derive(Debug, Clone)]
pub struct Options {
    pub theme: Theme,
    /// When the user asked for the Picker; with `HOARDER_TIMINGS` set, the
    /// time from here to the first frame on screen is printed as
    /// `picker_visible_ms`.
    pub started: Instant,
}

/// Show the Picker and wait for the user. Returns what they picked, or
/// `None` if they closed it; either way the panel is off the screen by then.
pub fn run(history: &mut dyn History, options: &Options) -> Result<Option<Pick>, OverlayError> {
    // The fonts take the longest; they load while everything else gets ready.
    let fonts = Fonts::load();
    let failed = |e: &dyn std::fmt::Display| OverlayError::Failed(e.to_string());
    let conn = Connection::connect_to_env().map_err(|e| failed(&e))?;
    let (globals, mut queue) = registry_queue_init::<App>(&conn).map_err(|e| failed(&e))?;
    let qh = queue.handle();
    let missing = |what: &str, e: &dyn std::fmt::Display| {
        OverlayError::Unsupported(format!(
            "the compositor does not offer {what}, which the Picker needs: {e}"
        ))
    };
    let compositor =
        CompositorState::bind(&globals, &qh).map_err(|e| missing("wl_compositor", &e))?;
    let layer_shell =
        LayerShell::bind(&globals, &qh).map_err(|e| missing("wlr-layer-shell", &e))?;
    let shm = Shm::bind(&globals, &qh).map_err(|e| missing("wl_shm", &e))?;

    // No anchors and no output: the compositor centres it on the output
    // the user is on.
    let layer = layer_shell.create_layer_surface(
        &qh,
        compositor.create_surface(&qh),
        Layer::Overlay,
        Some("hoarder-picker"),
        None,
    );
    layer.set_size(paint::WIDTH, paint::HEIGHT);
    layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
    layer.commit();

    let mut app = App {
        registry: RegistryState::new(&globals),
        outputs: OutputState::new(&globals, &qh),
        seats: SeatState::new(&globals, &qh),
        pool: SlotPool::new((paint::WIDTH * paint::HEIGHT * 4) as usize, &shm)
            .map_err(|e| failed(&e))?,
        shm,
        layer: Some(layer),
        keyboard: None,
        fonts,
        theme: options.theme.clone(),
        started: options.started,
        model: None,
        detail: None,
        keys: Vec::new(),
        mods: Modifiers::default(),
        scale: 1,
        configured: false,
        frame_pending: false,
        dirty: true,
        shown: false,
        outcome: None,
    };

    while app.outcome.is_none() {
        crate::wait::dispatch_once(&mut queue, None).map_err(|e| failed(&e))?;
        queue.dispatch_pending(&mut app).map_err(|e| failed(&e))?;
        if app.configured && app.model.is_none() {
            timing(app.started, "picker_configured");
            app.model = Some(Model::new(history.clips().map_err(|e| failed(&e))?));
            timing(app.started, "picker_history");
            app.dirty = true;
        }
        // Keys in the order typed, each against the list as the keys before
        // it left it: Enter right after typing picks from the new matches.
        if app.model.is_some() {
            for event in std::mem::take(&mut app.keys) {
                if app.outcome.is_none() {
                    app.key(&event, history);
                }
            }
        }
        app.reveal(history);
        app.fetch_detail(history);
        app.draw(&qh);
    }

    let picked = app.outcome.take().flatten();
    // Off the screen before the caller acts on the pick.
    if let Some(layer) = app.layer.take() {
        drop(layer);
    }
    conn.roundtrip().map_err(|e| failed(&e))?;
    Ok(picked)
}

struct App {
    registry: RegistryState,
    outputs: OutputState,
    seats: SeatState,
    shm: Shm,
    pool: SlotPool,
    layer: Option<LayerSurface>,
    keyboard: Option<WlKeyboard>,
    fonts: Fonts,
    theme: Theme,
    started: Instant,
    /// `None` until History has arrived.
    model: Option<Model>,
    /// The selected Clip's detail, by its id.
    detail: Option<(i64, Detail)>,
    /// Keys pressed and not yet acted on, with the modifiers held. The
    /// main loop acts on them, since it can ask History for full-text hits.
    keys: Vec<(KeyEvent, Modifiers)>,
    mods: Modifiers,
    /// Buffer scale of the output the panel is on.
    scale: i32,
    configured: bool,
    frame_pending: bool,
    /// Something changed since the last frame.
    dirty: bool,
    /// The first frame has been presented.
    shown: bool,
    /// `Some(pick)` once the user is done.
    outcome: Option<Option<Pick>>,
}

/// The frame callback of a committed frame.
struct Presented;

impl Dispatch2<WlCallback, App> for Presented {
    fn event(
        &self,
        app: &mut App,
        _: &WlCallback,
        event: wl_callback::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
        if !matches!(event, wl_callback::Event::Done { .. }) {
            return;
        }
        app.frame_pending = false;
        if !app.shown {
            app.shown = true;
            timing(app.started, "picker_visible");
        }
    }
}

impl App {
    /// Draw a new frame if anything changed and the compositor is ready.
    fn draw(&mut self, qh: &QueueHandle<App>) {
        if !self.dirty || !self.configured || self.frame_pending || self.outcome.is_some() {
            return;
        }
        let (Some(layer), Some(model)) = (&self.layer, &mut self.model) else {
            return;
        };
        let scale = self.scale.max(1) as u32;
        let (w, h) = (paint::WIDTH * scale, paint::HEIGHT * scale);
        let (buffer, canvas) = match self.pool.create_buffer(
            w as i32,
            h as i32,
            w as i32 * 4,
            wl_shm::Format::Argb8888,
        ) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("hoarder: cannot draw the Picker: {e}");
                return;
            }
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        let selected = model.selected().map(|e| e.id);
        let extra = paint::Extra {
            detail: (self.detail.as_ref())
                .filter(|(id, _)| Some(*id) == selected)
                .map(|(_, d)| d),
            now_ms,
        };
        paint::paint(canvas, scale, model, &extra, &self.theme, &mut self.fonts);
        if !self.shown {
            timing(self.started, "picker_painted");
        }
        let surface = layer.wl_surface();
        surface.set_buffer_scale(scale as i32);
        surface.damage_buffer(0, 0, w as i32, h as i32);
        surface.frame(qh, Presented);
        if let Err(e) = buffer.attach_to(surface) {
            eprintln!("hoarder: cannot draw the Picker: {e}");
            return;
        }
        layer.commit();
        self.frame_pending = true;
        self.dirty = false;
    }

    /// Once the first frame is up, fetch the selected Clip's detail if it
    /// is not there yet.
    fn fetch_detail(&mut self, history: &mut dyn History) {
        let Some(model) = &self.model else { return };
        if !self.shown || self.outcome.is_some() {
            return;
        }
        let Some(clip) = model.selected().filter(|e| !e.concealed) else {
            return;
        };
        if self.detail.as_ref().is_some_and(|(id, _)| *id == clip.id) {
            return;
        }
        self.detail = Some((clip.id, history.detail(clip)));
        self.dirty = true;
    }

    /// While Alt is held, fetch the text of the Concealed Clips on screen;
    /// once it is let go, wipe it.
    fn reveal(&mut self, history: &mut dyn History) {
        let Some(model) = &mut self.model else {
            return;
        };
        if !self.mods.alt {
            self.dirty |= model.hide();
            return;
        }
        for id in model.unrevealed(paint::rows()) {
            // A Clip that expired meanwhile just stays masked.
            if let Some(text) = history.reveal(id) {
                model.reveal(id, text);
                self.dirty = true;
            }
        }
    }

    /// Act on one key, pressed with `mods` held.
    fn key(&mut self, (event, mods): &(KeyEvent, Modifiers), history: &mut dyn History) {
        let Some(model) = &mut self.model else {
            return;
        };
        let ctrl = mods.ctrl;
        let rows = paint::rows() as isize;
        let mut query = model.query().to_owned();
        match event.keysym {
            Keysym::Escape => self.outcome = Some(None),
            Keysym::Return | Keysym::KP_Enter => {
                if let Some(clip) = model.selected() {
                    self.outcome = Some(Some(Pick {
                        id: clip.id,
                        plain: mods.shift,
                    }));
                }
            }
            Keysym::Delete | Keysym::KP_Delete => {
                if let Some(id) = model.selected().map(|e| e.id)
                    && history.delete(id)
                {
                    model.remove(id);
                }
            }
            Keysym::p | Keysym::P if ctrl => {
                if let Some((id, pinned)) = model.selected().map(|e| (e.id, e.pinned))
                    && history.pin(id, !pinned)
                {
                    model.set_pinned(id, !pinned);
                }
            }
            sym if ctrl && quick_row(sym).is_some() => {
                let n = quick_row(sym).unwrap_or_default();
                if let Some(clip) = model.row(n, rows as usize) {
                    self.outcome = Some(Some(Pick {
                        id: clip.id,
                        plain: mods.shift,
                    }));
                }
            }
            Keysym::Up | Keysym::KP_Up => model.move_by(-1),
            Keysym::Down | Keysym::KP_Down => model.move_by(1),
            Keysym::Page_Up | Keysym::KP_Page_Up => model.move_by(-rows),
            Keysym::Page_Down | Keysym::KP_Page_Down => model.move_by(rows),
            Keysym::BackSpace if ctrl => {
                let kept = query.trim_end().rfind(' ').map_or(0, |i| i + 1);
                query.truncate(kept);
            }
            Keysym::BackSpace => {
                query.pop();
            }
            Keysym::u | Keysym::U if ctrl => query.clear(),
            _ if ctrl || mods.alt => {}
            _ => {
                let typed = event.utf8.as_deref();
                if let Some(text) = typed.filter(|t| !t.chars().any(char::is_control)) {
                    query.push_str(text);
                }
            }
        }
        if query != model.query() {
            let found = history.search(&model::Filter::parse(&query).text);
            model.set_query(&query, &found);
        }
        self.dirty = true;
    }
}

/// The row (from 0) Ctrl+`sym` quick-restores, for the digits 1–9.
fn quick_row(sym: Keysym) -> Option<usize> {
    let digit = sym.key_char()?.to_digit(10)?;
    (1..=9).contains(&digit).then(|| digit as usize - 1)
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        factor: i32,
    ) {
        if factor != self.scale && factor > 0 {
            self.scale = factor;
            self.dirty = true;
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

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        // The output went away, or the compositor said so: nothing picked.
        self.outcome = Some(None);
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &LayerSurface,
        _: LayerSurfaceConfigure,
        _: u32,
    ) {
        self.configured = true;
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
        if capability == Capability::Keyboard
            && let Some(k) = self.keyboard.take()
        {
            k.release();
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
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
        // No more modifier news: hide anything Alt revealed.
        self.mods = Modifiers::default();
        self.dirty = true;
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        if self.outcome.is_none() {
            self.keys.push((event, self.mods));
        }
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
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
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
        if modifiers.alt != self.mods.alt {
            self.dirty = true;
        }
        self.mods = modifiers;
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
wayland_client::delegate_noop!(App: wayland_client::protocol::wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(App: ignore wayland_client::protocol::wl_buffer::WlBuffer);
