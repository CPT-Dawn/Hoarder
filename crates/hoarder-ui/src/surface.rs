//! What every layer-shell UI of Hoarder's shares: connecting to the
//! compositor, binding the globals a layer surface needs, the error they all
//! fail with, and the stage timings printed with [`TIMINGS_ENV`].

use std::time::Instant;

use smithay_client_toolkit::{
    compositor::CompositorState,
    globals::GlobalData,
    shell::wlr_layer::{LayerShell, LayerShellHandler},
    shm::{Shm, ShmHandler},
};
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle,
    globals::{GlobalList, GlobalListContents, registry_queue_init},
    protocol::{wl_compositor::WlCompositor, wl_registry::WlRegistry, wl_shm::WlShm},
};
use smithay_client_toolkit::reexports::protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;

/// Set to print how long each stage of the Overlay or Picker took.
pub const TIMINGS_ENV: &str = "HOARDER_TIMINGS";

/// Everything that can stop the Overlay, the Picker or the countdown.
#[derive(Debug)]
pub enum Error {
    /// The compositor lacks a protocol the UI needs.
    Unsupported(String),
    /// The Wayland connection failed.
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unsupported(m) => f.write_str(m),
            Error::Failed(m) => write!(f, "overlay failed: {m}"),
        }
    }
}

impl std::error::Error for Error {}

/// A [`Error::Failed`] saying why.
pub(crate) fn failed(e: impl std::fmt::Display) -> Error {
    Error::Failed(e.to_string())
}

/// A [`Error::Unsupported`] for a missing `what` that `who` (the Overlay,
/// the Picker…) needs.
pub(crate) fn missing(who: &str, what: &str, e: impl std::fmt::Display) -> Error {
    Error::Unsupported(format!(
        "the compositor does not offer {what}, which the {who} needs: {e}"
    ))
}

/// With [`TIMINGS_ENV`] set, print how long `stage` took to reach since `started`.
pub(crate) fn timing(started: Instant, stage: &str) {
    if std::env::var_os(TIMINGS_ENV).is_some() {
        eprintln!(
            "hoarder: timing {stage}_ms={:.1}",
            started.elapsed().as_secs_f64() * 1000.0
        );
    }
}

/// A connection with what any layer surface needs bound.
pub(crate) struct Layered<D> {
    pub conn: Connection,
    pub globals: GlobalList,
    pub queue: EventQueue<D>,
    pub qh: QueueHandle<D>,
    pub compositor: CompositorState,
    pub layer_shell: LayerShell,
    pub shm: Shm,
}

/// Connect to the compositor and bind `wl_compositor`, `wlr-layer-shell` and
/// `wl_shm`; a missing one is [`Error::Unsupported`], naming `who` needs it.
pub(crate) fn connect<D>(who: &str) -> Result<Layered<D>, Error>
where
    D: Dispatch<WlRegistry, GlobalListContents>
        + Dispatch<WlCompositor, GlobalData>
        + Dispatch<ZwlrLayerShellV1, GlobalData>
        + Dispatch<WlShm, GlobalData>
        + LayerShellHandler
        + ShmHandler
        + 'static,
{
    let conn = Connection::connect_to_env().map_err(failed)?;
    let (globals, queue) = registry_queue_init::<D>(&conn).map_err(failed)?;
    let qh = queue.handle();
    let compositor =
        CompositorState::bind(&globals, &qh).map_err(|e| missing(who, "wl_compositor", e))?;
    let layer_shell =
        LayerShell::bind(&globals, &qh).map_err(|e| missing(who, "wlr-layer-shell", e))?;
    let shm = Shm::bind(&globals, &qh).map_err(|e| missing(who, "wl_shm", e))?;
    Ok(Layered {
        conn,
        globals,
        queue,
        qh,
        compositor,
        layer_shell,
        shm,
    })
}
