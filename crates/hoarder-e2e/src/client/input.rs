//! Probe windows that record the input they receive, and virtual devices that inject it.

use std::os::fd::AsFd;
use std::time::Instant;

use wayland_client::protocol::{
    wl_keyboard::{self, WlKeyboard},
    wl_pointer::{self, WlPointer},
    wl_seat::{self, WlSeat},
    wl_shm,
    wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, backend::ObjectId};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::{self, XdgToplevel},
    xdg_wm_base::{self, XdgWmBase},
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

use super::{Client, State, Worker};
use crate::{Output, Result, Rgb8};

/// What a probe window shows unless asked otherwise.
const PROBE_GREY: Rgb8 = Rgb8(0x80, 0x80, 0x80);

/// An input event a helper client's windows received.
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    /// The pointer entered a window, at surface-local logical coordinates.
    PointerEnter {
        x: f64,
        y: f64,
    },
    PointerLeave,
    /// Surface-local logical coordinates.
    PointerMotion {
        x: f64,
        y: f64,
    },
    /// `button` is a Linux `BTN_*` code.
    PointerButton {
        button: u32,
        pressed: bool,
    },
    KeyboardEnter,
    KeyboardLeave,
    /// `key` is a Linux `KEY_*` code.
    Key {
        key: u32,
        pressed: bool,
    },
    /// The xkb depressed-modifiers mask (Shift = 1, Ctrl = 4, Alt = 8, Super = 64).
    Modifiers {
        depressed: u32,
    },
}

/// Input-related event state of one client.
#[derive(Default)]
pub(super) struct InputState {
    events: Vec<InputEvent>,
    pointer: Option<WlPointer>,
    keyboard: Option<WlKeyboard>,
    /// `xdg_surface`s that have received at least one configure, with the
    /// toplevel size suggested by the compositor.
    configured: Vec<(ObjectId, (i32, i32))>,
    pending_size: (i32, i32),
    /// Windows with a buffer attached, re-committed on every configure.
    live: Vec<(ObjectId, WlSurface, WpViewport)>,
}

/// Size of a probe window when the compositor leaves it to us.
const DEFAULT_WINDOW_SIZE: (i32, i32) = (640, 480);

/// A probe window: a plain `xdg_toplevel` that records its input.
pub(super) struct Window {
    _xdg_surface: XdgSurface,
    _toplevel: XdgToplevel,
}

/// Virtual pointer and keyboard, owned by the [`Session`](crate::Session).
///
/// Every call returns only after the compositor has processed the injected
/// events, so whatever a test checks next already sees their effect.
#[derive(Debug)]
pub struct Input {
    client: Client,
    /// Bounding box of all outputs in the layout: `(x, y, width, height)`.
    layout: (i32, i32, u32, u32),
}

/// Keymap handed to the compositor for the virtual keyboard: plain US layout,
/// resolved from the system's xkeyboard-config by the compositor.
const KEYMAP: &str = "xkb_keymap {
    xkb_keycodes { include \"evdev+aliases(qwerty)\" };
    xkb_types { include \"complete\" };
    xkb_compat { include \"complete\" };
    xkb_symbols { include \"pc+us+inet(evdev)\" };
};
";

impl Input {
    pub(crate) fn new(client: Client, outputs: &[Output]) -> Result<Self> {
        let x0 = outputs.iter().map(|o| o.position.0).min().unwrap_or(0);
        let y0 = outputs.iter().map(|o| o.position.1).min().unwrap_or(0);
        let x1 = outputs
            .iter()
            .map(|o| o.position.0 + o.spec.logical_size().0 as i32)
            .max()
            .unwrap_or(0);
        let y1 = outputs
            .iter()
            .map(|o| o.position.1 + o.spec.logical_size().1 as i32)
            .max()
            .unwrap_or(0);
        client.call(|w| w.create_virtual_devices())?;
        Ok(Input {
            client,
            layout: (x0, y0, (x1 - x0) as u32, (y1 - y0) as u32),
        })
    }

    /// Warp the pointer to `(x, y)` in `output`'s logical coordinates.
    pub fn move_to(&self, output: &Output, x: f64, y: f64) -> Result<()> {
        // Absolute motion is integer-valued; scale the extents up for sub-pixel precision.
        const PRECISION: f64 = 256.0;
        let (lx, ly, lw, lh) = self.layout;
        let ax = ((output.position.0 - lx) as f64 + x) * PRECISION;
        let ay = ((output.position.1 - ly) as f64 + y) * PRECISION;
        let (ew, eh) = (
            (lw as f64 * PRECISION) as u32,
            (lh as f64 * PRECISION) as u32,
        );
        self.client.call(move |w| {
            let p = w.devices().pointer.clone();
            p.motion_absolute(w.now_ms(), ax.round() as u32, ay.round() as u32, ew, eh);
            p.frame();
            w.roundtrip()
        })
    }

    /// Press (`true`) or release a pointer button (`BTN_*`, see [`keys`](crate::keys)).
    pub fn button(&self, button: u32, pressed: bool) -> Result<()> {
        self.client.call(move |w| {
            let p = w.devices().pointer.clone();
            let state = if pressed {
                wl_pointer::ButtonState::Pressed
            } else {
                wl_pointer::ButtonState::Released
            };
            p.button(w.now_ms(), button, state);
            p.frame();
            w.roundtrip()
        })
    }

    /// Press and release a pointer button.
    pub fn click(&self, button: u32) -> Result<()> {
        self.button(button, true)?;
        self.button(button, false)
    }

    /// Press (`true`) or release a key (`KEY_*`, see [`keys`](crate::keys)).
    pub fn key(&self, key: u32, pressed: bool) -> Result<()> {
        self.client.call(move |w| {
            let k = w.devices().keyboard.clone();
            k.key(w.now_ms(), key, if pressed { 1 } else { 0 });
            let bit = modifier_bit(key);
            if bit != 0 {
                let devices = w.devices.as_mut().expect("created with the Input");
                if pressed {
                    devices.depressed |= bit;
                } else {
                    devices.depressed &= !bit;
                }
                k.modifiers(devices.depressed, 0, 0, 0);
            }
            w.roundtrip()
        })
    }

    pub fn key_down(&self, key: u32) -> Result<()> {
        self.key(key, true)
    }

    pub fn key_up(&self, key: u32) -> Result<()> {
        self.key(key, false)
    }

    /// Press and release one key.
    pub fn tap(&self, key: u32) -> Result<()> {
        self.chord(&[key])
    }

    /// Press `keys` in order, then release them in reverse (e.g. Ctrl+V).
    pub fn chord(&self, keys: &[u32]) -> Result<()> {
        for &k in keys {
            self.key(k, true)?;
        }
        for &k in keys.iter().rev() {
            self.key(k, false)?;
        }
        Ok(())
    }
}

/// The virtual devices of the session's input client.
pub(super) struct Devices {
    pointer: ZwlrVirtualPointerV1,
    keyboard: ZwpVirtualKeyboardV1,
    epoch: Instant,
    /// xkb depressed-modifiers mask; the virtual keyboard protocol leaves
    /// tracking it to the client.
    depressed: u32,
}

/// The xkb modifier bit a key drives in the US keymap, if any.
fn modifier_bit(key: u32) -> u32 {
    use crate::keys::*;
    match key {
        KEY_LEFTSHIFT | KEY_RIGHTSHIFT => 1,
        KEY_LEFTCTRL | KEY_RIGHTCTRL => 4,
        KEY_LEFTALT | KEY_RIGHTALT => 8,
        KEY_LEFTMETA => 64,
        _ => 0,
    }
}

impl Client {
    /// Open a probe window (`xdg_toplevel` with this app_id and title).
    ///
    /// Sway maps and focuses it like any new app window. Its pointer and
    /// keyboard input is recorded; read it with
    /// [`take_input_events`](Self::take_input_events).
    pub fn open_window(&self, app_id: &str) -> Result<()> {
        self.open_window_coloured(app_id, PROBE_GREY)
    }

    /// [`open_window`](Self::open_window), filled with `colour` instead of
    /// the default grey (`#808080`), so windows can be told apart on screen.
    pub fn open_window_coloured(&self, app_id: &str, colour: Rgb8) -> Result<()> {
        let app_id = app_id.to_owned();
        self.call(move |w| w.open_window(&app_id, colour))
    }

    /// All input events this client's windows received since the last call.
    pub fn take_input_events(&self) -> Result<Vec<InputEvent>> {
        self.call(|w| {
            w.roundtrip()?;
            Ok(std::mem::take(&mut w.state.input.events))
        })
    }
}

impl Worker {
    fn devices(&self) -> &Devices {
        self.devices
            .as_ref()
            .expect("virtual devices are created with the Input")
    }

    fn now_ms(&self) -> u32 {
        self.devices().epoch.elapsed().as_millis() as u32
    }

    fn create_virtual_devices(&mut self) -> Result<()> {
        let pointer_manager: ZwlrVirtualPointerManagerV1 = self.bind(1)?;
        let keyboard_manager: ZwpVirtualKeyboardManagerV1 = self.bind(1)?;
        let pointer = pointer_manager.create_virtual_pointer(Some(&self.seat), &self.qh, ());
        let keyboard = keyboard_manager.create_virtual_keyboard(&self.seat, &self.qh, ());

        let mut keymap = KEYMAP.as_bytes().to_vec();
        keymap.push(0);
        let fd = rustix::fs::memfd_create("hoarder-e2e-keymap", rustix::fs::MemfdFlags::CLOEXEC)
            .map_err(std::io::Error::from)?;
        std::io::Write::write_all(&mut std::fs::File::from(fd.try_clone()?), &keymap)?;
        keyboard.keymap(
            wl_keyboard::KeymapFormat::XkbV1.into(),
            fd.as_fd(),
            keymap.len() as u32,
        );

        self.devices = Some(Devices {
            pointer,
            keyboard,
            epoch: Instant::now(),
            depressed: 0,
        });
        self.roundtrip()
    }

    fn open_window(&mut self, app_id: &str, colour: Rgb8) -> Result<()> {
        let compositor: wayland_client::protocol::wl_compositor::WlCompositor = self.bind(4)?;
        let wm_base: XdgWmBase = self.bind(1)?;
        let viewporter: WpViewporter = self.bind(1)?;

        let surface = compositor.create_surface(&self.qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &self.qh, ());
        let toplevel = xdg_surface.get_toplevel(&self.qh, ());
        toplevel.set_app_id(app_id.to_owned());
        toplevel.set_title(app_id.to_owned());
        surface.commit();
        let id = xdg_surface.id();
        self.wait_until("xdg_surface configure", |s| {
            s.input.configured.iter().any(|(c, _)| *c == id)
        })?;

        let (w, h) = self
            .state
            .input
            .configured
            .iter()
            .find(|(c, _)| *c == id)
            .map(|(_, size)| *size)
            .expect("waited for it");
        let (w, h) = if w > 0 && h > 0 {
            (w, h)
        } else {
            DEFAULT_WINDOW_SIZE
        };
        // One pixel, stretched over the window: scale-agnostic and cheap.
        let Rgb8(r, g, b) = colour;
        let pixel = u32::from_be_bytes([0, r, g, b]);
        let buffer = self.shm_buffer(&pixel.to_le_bytes(), 1, 1, wl_shm::Format::Xrgb8888)?;
        let viewport = viewporter.get_viewport(&surface, &self.qh, ());
        viewport.set_destination(w, h);
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, 1, 1);
        self.state.input.live.push((id, surface.clone(), viewport));
        self.commit_and_wait_for_frame(&surface)?;

        self.windows.push(Window {
            _xdg_surface: xdg_surface,
            _toplevel: toplevel,
        });
        self.roundtrip()
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            let input = &mut state.input;
            if caps.contains(wl_seat::Capability::Pointer) && input.pointer.is_none() {
                input.pointer = Some(seat.get_pointer(qh, ()));
            }
            if caps.contains(wl_seat::Capability::Keyboard) && input.keyboard.is_none() {
                input.keyboard = Some(seat.get_keyboard(qh, ()));
            }
        }
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let e = match event {
            wl_pointer::Event::Enter {
                surface_x,
                surface_y,
                ..
            } => InputEvent::PointerEnter {
                x: surface_x,
                y: surface_y,
            },
            wl_pointer::Event::Leave { .. } => InputEvent::PointerLeave,
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => InputEvent::PointerMotion {
                x: surface_x,
                y: surface_y,
            },
            wl_pointer::Event::Button {
                button, state: s, ..
            } => InputEvent::PointerButton {
                button,
                pressed: s == WEnum::Value(wl_pointer::ButtonState::Pressed),
            },
            _ => return,
        };
        state.input.events.push(e);
    }
}

impl Dispatch<WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let e = match event {
            wl_keyboard::Event::Enter { .. } => InputEvent::KeyboardEnter,
            wl_keyboard::Event::Leave { .. } => InputEvent::KeyboardLeave,
            wl_keyboard::Event::Key { key, state: s, .. } => InputEvent::Key {
                key,
                pressed: s == WEnum::Value(wl_keyboard::KeyState::Pressed),
            },
            wl_keyboard::Event::Modifiers { mods_depressed, .. } => InputEvent::Modifiers {
                depressed: mods_depressed,
            },
            _ => return,
        };
        state.input.events.push(e);
    }
}

impl Dispatch<XdgWmBase, ()> for State {
    fn event(
        _: &mut Self,
        wm_base: &XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, ()> for State {
    fn event(
        state: &mut Self,
        xdg_surface: &XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg_surface.ack_configure(serial);
            let size = state.input.pending_size;
            let id = xdg_surface.id();
            // Sway holds a tiling transaction until the window commits the
            // configure it acked, so answer every one straight away.
            if let Some((_, surface, viewport)) = state.input.live.iter().find(|(c, ..)| *c == id) {
                let (w, h) = if size.0 > 0 && size.1 > 0 {
                    size
                } else {
                    DEFAULT_WINDOW_SIZE
                };
                viewport.set_destination(w, h);
                surface.commit();
            }
            state.input.configured.retain(|(c, _)| *c != id);
            state.input.configured.push((id, size));
        }
    }
}

impl Dispatch<XdgToplevel, ()> for State {
    fn event(
        state: &mut Self,
        _: &XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Configure { width, height, .. } = event {
            state.input.pending_size = (width, height);
        }
    }
}

wayland_client::delegate_noop!(State: ignore ZwlrVirtualPointerManagerV1);
wayland_client::delegate_noop!(State: ignore ZwlrVirtualPointerV1);
wayland_client::delegate_noop!(State: ignore ZwpVirtualKeyboardManagerV1);
wayland_client::delegate_noop!(State: ignore ZwpVirtualKeyboardV1);
