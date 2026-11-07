//! Text fields: probe windows that paste the clipboard on their paste chord,
//! the way a real app does, through `wl_data_device` and keyboard focus.

use std::io::Read as _;
use std::sync::{Arc, Mutex};

use wayland_client::backend::ObjectId;
use wayland_client::protocol::{
    wl_data_device::{self, WlDataDevice},
    wl_data_device_manager::WlDataDeviceManager,
    wl_data_offer::{self, WlDataOffer},
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

use super::{Client, State};
use crate::{Result, Rgb8};

/// Which paste chord a text field answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextField {
    /// An ordinary app: Ctrl+V pastes.
    Editor,
    /// A terminal: Ctrl+Shift+V pastes, and Ctrl+V is a literal `^V` that
    /// pastes nothing.
    Terminal,
}

/// xkb modifier bits (US keymap).
const SHIFT: u32 = 1;
const CTRL: u32 = 4;
const KEY_V: u32 = 47;

/// Text-field state of one client.
#[derive(Default)]
pub(super) struct FieldState {
    /// Each text field's surface and kind.
    fields: Vec<(ObjectId, TextField)>,
    /// The surface with keyboard focus, if it is ours.
    focus: Option<ObjectId>,
    /// Depressed xkb modifiers.
    mods: u32,
    /// MIME types of each live `wl_data_offer`.
    offers: Vec<(WlDataOffer, Vec<String>)>,
    /// The clipboard offer, as the compositor last announced it to us.
    selection: Option<WlDataOffer>,
    /// Everything pasted into this client's text fields, in order.
    text: Arc<Mutex<String>>,
}

impl FieldState {
    pub(super) fn keyboard_enter(&mut self, surface: ObjectId) {
        self.focus = Some(surface);
    }

    pub(super) fn keyboard_leave(&mut self) {
        self.focus = None;
        self.mods = 0;
    }

    pub(super) fn modifiers(&mut self, depressed: u32) {
        self.mods = depressed;
    }

    /// A key went down in one of our windows: paste if it is the focused
    /// field's chord.
    pub(super) fn key_pressed(&mut self, key: u32, conn: &Connection) {
        let Some(focus) = &self.focus else { return };
        let Some((_, kind)) = self.fields.iter().find(|(s, _)| s == focus) else {
            return;
        };
        let chord = match kind {
            TextField::Editor => CTRL,
            TextField::Terminal => CTRL | SHIFT,
        };
        if key != KEY_V || self.mods & (CTRL | SHIFT) != chord {
            return;
        }
        let Some(offer) = &self.selection else { return };
        let Some((_, mimes)) = self.offers.iter().find(|(o, _)| o == offer) else {
            return;
        };
        let Some(mime) = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"]
            .into_iter()
            .find(|m| mimes.iter().any(|t| t == m))
        else {
            return;
        };
        let Ok((read, write)) = rustix::pipe::pipe() else {
            return;
        };
        offer.receive(mime.to_owned(), std::os::fd::AsFd::as_fd(&write));
        drop(write);
        let _ = conn.flush();
        let text = self.text.clone();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = std::fs::File::from(read).read_to_end(&mut bytes);
            text.lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&bytes));
        });
    }
}

impl Client {
    /// Open a text field: a probe window (see [`open_window`](Self::open_window))
    /// that, while focused, pastes the clipboard's text when it gets its
    /// [`TextField`] chord. Read what it received with [`text`](Self::text).
    pub fn open_text_field(&self, app_id: &str, kind: TextField) -> Result<()> {
        let app_id = app_id.to_owned();
        self.call(move |w| {
            if w.text_device.is_none() {
                let manager: WlDataDeviceManager = w.bind(3)?;
                w.text_device = Some(manager.get_data_device(&w.seat, &w.qh, ()));
            }
            let surface = w.open_window(&app_id, Rgb8(0xf0, 0xf0, 0xe0))?;
            w.state.fields.fields.push((surface, kind));
            w.roundtrip()
        })
    }

    /// Everything pasted into this client's text fields so far.
    pub fn text(&self) -> Result<String> {
        self.call(|w| {
            w.roundtrip()?;
            Ok(w.state.fields.text.lock().unwrap().clone())
        })
    }
}

impl Dispatch<WlDataDevice, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let fields = &mut state.fields;
        match event {
            wl_data_device::Event::DataOffer { id } => fields.offers.push((id, Vec::new())),
            wl_data_device::Event::Selection { id } => {
                if let Some(old) = std::mem::replace(&mut fields.selection, id) {
                    fields.offers.retain(|(o, _)| *o != old);
                    old.destroy();
                }
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(State, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ()),
    ]);
}

impl Dispatch<WlDataOffer, ()> for State {
    fn event(
        state: &mut Self,
        offer: &WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event
            && let Some((_, mimes)) = state
                .fields
                .offers
                .iter_mut()
                .find(|(o, _)| o.id() == offer.id())
        {
            mimes.push(mime_type);
        }
    }
}

wayland_client::delegate_noop!(State: ignore WlDataDeviceManager);
