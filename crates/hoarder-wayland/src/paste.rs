//! Auto-paste (PLAN.md §5.4): type the paste chord into the focused window
//! through `zwp_virtual_keyboard_v1`.
//!
//! The focused window and its app_id come from
//! `wlr-foreign-toplevel-management`; its app_id decides between Ctrl+V and
//! a terminal's Ctrl+Shift+V. The virtual keyboard brings its own US
//! keymap, so the chord means the same under any user layout.

use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use wayland_client::backend::ObjectId;
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_keyboard, wl_registry::WlRegistry, wl_seat::WlSeat};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop, event_created_child,
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

use crate::clipboard::{ClipboardError, hidden_globals};

const VIRTUAL_KEYBOARD: &str = "zwp_virtual_keyboard_manager_v1";
const WLR_FOREIGN_TOPLEVEL: &str = "zwlr_foreign_toplevel_manager_v1";

/// How long to wait for focus to come back to a window after the Picker
/// left. Sway hands it back as the layer surface goes; other compositors
/// may take a frame or two.
const FOCUS_WAIT: Duration = Duration::from_millis(250);

/// Plain US keymap; the compositor resolves the includes.
const KEYMAP: &str = "xkb_keymap {
    xkb_keycodes { include \"evdev+aliases(qwerty)\" };
    xkb_types { include \"complete\" };
    xkb_compat { include \"complete\" };
    xkb_symbols { include \"pc+us+inet(evdev)\" };
};
";

// Linux input event codes, and their xkb modifier bits in the keymap above.
const KEY_LEFTCTRL: u32 = 29;
const KEY_LEFTSHIFT: u32 = 42;
const KEY_V: u32 = 47;
const MOD_SHIFT: u32 = 1;
const MOD_CTRL: u32 = 4;

/// The keys that paste in a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chord {
    CtrlV,
    /// What terminals paste with, since Ctrl+V is theirs to pass on.
    CtrlShiftV,
}

impl Chord {
    /// The chord for a window with `app_id`: Ctrl+Shift+V if it is one of
    /// `terminal_apps` (compared ignoring ASCII case), else Ctrl+V.
    pub fn for_app(app_id: Option<&str>, terminal_apps: &[String]) -> Chord {
        match app_id {
            Some(id) if terminal_apps.iter().any(|t| t.eq_ignore_ascii_case(id)) => {
                Chord::CtrlShiftV
            }
            _ => Chord::CtrlV,
        }
    }

    /// `(key, xkb modifier bit)` in press order.
    fn keys(self) -> &'static [(u32, u32)] {
        match self {
            Chord::CtrlV => &[(KEY_LEFTCTRL, MOD_CTRL), (KEY_V, 0)],
            Chord::CtrlShiftV => &[
                (KEY_LEFTCTRL, MOD_CTRL),
                (KEY_LEFTSHIFT, MOD_SHIFT),
                (KEY_V, 0),
            ],
        }
    }
}

/// What [`paste`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pasted {
    /// The chord went to the focused window, whose app_id is given when the
    /// compositor tells.
    Into {
        app_id: Option<String>,
        chord: Chord,
    },
    /// No window took focus, so there was nowhere to paste.
    NoWindow,
}

/// Type the paste chord into the focused window.
///
/// Call it once the caller's own surfaces are gone; it waits briefly for
/// focus to settle on a window first. Fails with
/// [`ClipboardError::Unsupported`] when the compositor has no virtual
/// keyboard.
pub fn paste(terminal_apps: &[String]) -> Result<Pasted, ClipboardError> {
    let conn =
        Connection::connect_to_env().map_err(|e| ClipboardError::NoWayland(e.to_string()))?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn)
        .map_err(|e| ClipboardError::NoWayland(e.to_string()))?;
    let qh = queue.handle();
    let hidden = hidden_globals();
    let available = |name: &str| !hidden.contains(name) && advertises(&globals, name);
    if !available(VIRTUAL_KEYBOARD) {
        return Err(ClipboardError::Unsupported(
            "this compositor offers no zwp_virtual_keyboard_manager_v1, so the Picker \
             cannot paste for you; paste with Ctrl+V yourself"
                .into(),
        ));
    }
    let seat: WlSeat = bind(&globals, &qh, 1)?;
    let mut state = State::default();

    // Without the toplevel list, assume focus is back and paste plainly.
    let app_id = if available(WLR_FOREIGN_TOPLEVEL) {
        let _: ZwlrForeignToplevelManagerV1 = bind(&globals, &qh, 3)?;
        match focused(&mut queue, &mut state)? {
            Some(app_id) => app_id,
            None => return Ok(Pasted::NoWindow),
        }
    } else {
        None
    };
    let chord = Chord::for_app(app_id.as_deref(), terminal_apps);

    let manager: ZwpVirtualKeyboardManagerV1 = bind(&globals, &qh, 1)?;
    let keyboard = manager.create_virtual_keyboard(&seat, &qh, ());
    let mut keymap = KEYMAP.as_bytes().to_vec();
    keymap.push(0);
    let fd = rustix::fs::memfd_create("hoarder-keymap", rustix::fs::MemfdFlags::CLOEXEC)
        .map_err(std::io::Error::from)?;
    std::io::Write::write_all(&mut std::fs::File::from(fd.try_clone()?), &keymap)?;
    keyboard.keymap(
        wl_keyboard::KeymapFormat::XkbV1.into(),
        fd.as_fd(),
        keymap.len() as u32,
    );

    let started = Instant::now();
    let time = || started.elapsed().as_millis() as u32;
    let mut mods = 0;
    for &(key, bit) in chord.keys() {
        keyboard.key(time(), key, wl_keyboard::KeyState::Pressed.into());
        if bit != 0 {
            mods |= bit;
            keyboard.modifiers(mods, 0, 0, 0);
        }
    }
    for &(key, bit) in chord.keys().iter().rev() {
        keyboard.key(time(), key, wl_keyboard::KeyState::Released.into());
        if bit != 0 {
            mods &= !bit;
            keyboard.modifiers(mods, 0, 0, 0);
        }
    }
    // Everything is delivered before the keyboard goes away.
    queue.roundtrip(&mut state).map_err(failed)?;
    keyboard.destroy();
    queue.roundtrip(&mut state).map_err(failed)?;
    Ok(Pasted::Into { app_id, chord })
}

/// The focused toplevel's app_id (`Some(None)` when it has none), waiting
/// up to [`FOCUS_WAIT`] for one to be focused.
fn focused(
    queue: &mut EventQueue<State>,
    state: &mut State,
) -> Result<Option<Option<String>>, ClipboardError> {
    let deadline = Instant::now() + FOCUS_WAIT;
    // The first roundtrip lists the toplevels, the second their details.
    queue.roundtrip(state).map_err(failed)?;
    loop {
        queue.roundtrip(state).map_err(failed)?;
        if let Some(id) = &state.focused {
            return Ok(Some(state.toplevels.get(id).and_then(|t| t.app_id.clone())));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[derive(Debug, Default)]
struct Toplevel {
    app_id: Option<String>,
    pending_active: bool,
}

#[derive(Debug, Default)]
struct State {
    toplevels: std::collections::HashMap<ObjectId, Toplevel>,
    focused: Option<ObjectId>,
}

fn bind<I>(globals: &GlobalList, qh: &QueueHandle<State>, version: u32) -> Result<I, ClipboardError>
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
            }
            _ => {}
        }
    }
}

delegate_noop!(State: ignore WlSeat);
delegate_noop!(State: ignore ZwpVirtualKeyboardManagerV1);
delegate_noop!(State: ignore ZwpVirtualKeyboardV1);

#[cfg(test)]
mod tests {
    use super::*;

    fn terminals() -> Vec<String> {
        vec!["kitty".into(), "Alacritty".into()]
    }

    #[test]
    fn terminals_paste_with_shift_and_everything_else_without() {
        assert_eq!(
            Chord::for_app(Some("kitty"), &terminals()),
            Chord::CtrlShiftV
        );
        assert_eq!(
            Chord::for_app(Some("alacritty"), &terminals()),
            Chord::CtrlShiftV
        );
        assert_eq!(Chord::for_app(Some("firefox"), &terminals()), Chord::CtrlV);
        assert_eq!(Chord::for_app(None, &terminals()), Chord::CtrlV);
    }
}
