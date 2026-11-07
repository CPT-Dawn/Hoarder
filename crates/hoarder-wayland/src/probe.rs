//! What the compositor offers, without binding anything: the global
//! interfaces it advertises, less those hidden by `HOARDER_HIDE_GLOBALS`
//! (so `hoarder doctor` agrees with the paths the other commands choose).

use std::collections::HashSet;

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::{Connection, Dispatch, QueueHandle};

use crate::clipboard::hidden_globals;

/// The interfaces a compositor advertises.
#[derive(Debug, Clone, Default)]
pub struct Globals {
    interfaces: HashSet<String>,
}

impl Globals {
    /// Whether `interface` (e.g. `ext_data_control_manager_v1`) is offered.
    pub fn has(&self, interface: &str) -> bool {
        self.interfaces.contains(interface)
    }
}

/// Connect to `$WAYLAND_DISPLAY` and list its globals.
pub fn globals() -> Result<Globals, String> {
    let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
    let (globals, _queue) = registry_queue_init::<Probe>(&conn).map_err(|e| e.to_string())?;
    let hidden = hidden_globals();
    let interfaces = globals.contents().with_list(|list| {
        list.iter()
            .map(|g| g.interface.clone())
            .filter(|i| !hidden.contains(i))
            .collect()
    });
    Ok(Globals { interfaces })
}

struct Probe;

impl Dispatch<WlRegistry, GlobalListContents> for Probe {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as wayland_client::Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
