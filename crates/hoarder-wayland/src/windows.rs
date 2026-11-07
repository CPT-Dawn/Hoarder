//! Window geometry for the Overlay's window snapping (PLAN.md §4.2): where
//! the windows are, in global logical coordinates, asked of the compositor
//! over its own IPC.
//!
//! [`detect`] picks the [`WindowGeometry`] provider for the running
//! compositor: [`HyprlandIpc`], [`SwayIpc`], or [`NoGeometry`] everywhere
//! else, where there simply is no hover highlighting.

use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use crate::LogicalRect;

/// How long a compositor gets to answer one IPC request.
const IPC_TIMEOUT: Duration = Duration::from_secs(1);

/// A window as the compositor lays it out.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    /// The window's content (no borders or title bar), in global logical
    /// coordinates.
    pub rect: LogicalRect,
    pub title: String,
    /// The Wayland app_id, or the X11 class for Xwayland windows.
    pub app_id: String,
    /// Its `ext-foreign-toplevel-list-v1` identifier, when the provider
    /// knows it (Sway), for capturing its own buffer.
    pub toplevel: Option<String>,
    /// Its Hyprland address, which `hyprland_toplevel_mapping_v1` ties to
    /// a toplevel.
    pub address: Option<u64>,
    /// The scale of the output it is on, if known.
    pub scale: Option<f64>,
    /// Its own buffer may be old: on Hyprland, a window on a workspace no
    /// monitor shows is not redrawn.
    pub may_be_stale: bool,
}

impl Window {
    /// Whether the global logical point `(x, y)` lies on the window.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        let r = &self.rect;
        x >= r.x as f64
            && y >= r.y as f64
            && x < r.x as f64 + r.width as f64
            && y < r.y as f64 + r.height as f64
    }
}

/// Why the window geometry could not be read.
#[derive(Debug)]
pub enum GeometryError {
    Io(std::io::Error),
    /// The compositor answered with something we cannot read.
    Parse(String),
}

impl std::fmt::Display for GeometryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GeometryError::Io(e) => write!(f, "cannot ask the compositor for its windows: {e}"),
            GeometryError::Parse(m) => write!(f, "cannot read the compositor's windows: {m}"),
        }
    }
}

impl std::error::Error for GeometryError {}

impl From<std::io::Error> for GeometryError {
    fn from(e: std::io::Error) -> Self {
        GeometryError::Io(e)
    }
}

impl From<serde_json::Error> for GeometryError {
    fn from(e: serde_json::Error) -> Self {
        GeometryError::Parse(e.to_string())
    }
}

/// Something that knows where the windows are.
pub trait WindowGeometry: std::fmt::Debug + Send {
    /// The windows on screen right now, topmost first: of the windows
    /// containing a point, the first one is the one the user sees there.
    fn windows(&self) -> Result<Vec<Window>, GeometryError>;

    /// Where the geometry comes from, for `hoarder doctor`.
    fn name(&self) -> &'static str {
        "none"
    }

    /// Every window, also those on workspaces no output shows; the ones
    /// on screen first.
    fn all_windows(&self) -> Result<Vec<Window>, GeometryError> {
        self.windows()
    }

    /// The name of the output the user is on (`shot output`); `None` if
    /// the provider cannot tell.
    fn focused_output(&self) -> Result<Option<String>, GeometryError> {
        Ok(None)
    }
}

/// The provider for the compositor this process runs under, picked from
/// the environment: `HYPRLAND_INSTANCE_SIGNATURE`, then `SWAYSOCK`.
pub fn detect() -> Box<dyn WindowGeometry> {
    if let Some(p) = HyprlandIpc::from_env() {
        Box::new(p)
    } else if let Some(p) = SwayIpc::from_env() {
        Box::new(p)
    } else {
        Box::new(NoGeometry)
    }
}

/// No window geometry: the Overlay shows no hover highlighting.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoGeometry;

impl WindowGeometry for NoGeometry {
    fn windows(&self) -> Result<Vec<Window>, GeometryError> {
        Ok(Vec::new())
    }
}

fn connect(path: &std::path::Path) -> std::io::Result<UnixStream> {
    let stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(IPC_TIMEOUT))?;
    stream.set_write_timeout(Some(IPC_TIMEOUT))?;
    Ok(stream)
}

// ---------------------------------------------------------------- Sway ----

/// Sway's i3-compatible IPC: `get_tree`.
#[derive(Debug, Clone)]
pub struct SwayIpc {
    socket: PathBuf,
}

/// i3-ipc message types `GET_OUTPUTS` and `GET_TREE`.
const SWAY_GET_OUTPUTS: u32 = 3;
const SWAY_GET_TREE: u32 = 4;
const SWAY_MAGIC: &[u8; 6] = b"i3-ipc";

impl SwayIpc {
    /// The session's Sway, from `$SWAYSOCK`.
    pub fn from_env() -> Option<SwayIpc> {
        std::env::var_os("SWAYSOCK")
            .filter(|s| !s.is_empty())
            .map(|s| SwayIpc { socket: s.into() })
    }

    /// The windows in a `get_tree` reply, topmost first.
    pub fn parse_tree(json: &[u8]) -> Result<Vec<Window>, GeometryError> {
        Ok(SwayIpc::parse(json)?
            .into_iter()
            .filter(|(_, visible)| *visible)
            .map(|(w, _)| w)
            .collect())
    }

    /// Every window in a `get_tree` reply, on screen or not (other
    /// workspaces, the scratchpad); those on screen come first, topmost
    /// first.
    pub fn parse_tree_all(json: &[u8]) -> Result<Vec<Window>, GeometryError> {
        let mut all = SwayIpc::parse(json)?;
        all.sort_by_key(|(_, visible)| !visible);
        Ok(all.into_iter().map(|(w, _)| w).collect())
    }

    /// The focused output's name in a `get_outputs` reply.
    pub fn parse_focused_output(json: &[u8]) -> Result<Option<String>, GeometryError> {
        #[derive(Deserialize)]
        struct Output {
            name: String,
            #[serde(default)]
            focused: bool,
        }
        let outputs: Vec<Output> = serde_json::from_slice(json)?;
        Ok(outputs.into_iter().find(|o| o.focused).map(|o| o.name))
    }

    /// Every window, topmost first, and whether it is on screen.
    fn parse(json: &[u8]) -> Result<Vec<(Window, bool)>, GeometryError> {
        let root: SwayNode = serde_json::from_slice(json)?;
        // Paint order, bottom first; reversed at the end.
        let mut normal = Vec::new();
        let mut fullscreen = Vec::new();
        let mut global = Vec::new();
        for output in &root.nodes {
            // The scratchpad (`__i3`) has no scale: its windows show nowhere.
            let scale = output.scale.filter(|s| *s > 0.0);
            for workspace in &output.nodes {
                let mut tiled = Vec::new();
                for n in &workspace.nodes {
                    n.leaves(&mut tiled);
                }
                let mut floating = Vec::new();
                for n in &workspace.floating_nodes {
                    n.leaves(&mut floating);
                }
                for leaf in tiled.into_iter().chain(floating) {
                    let shown =
                        output.name.as_deref() != Some("__i3") && leaf.visible == Some(true);
                    let at = (leaf, scale, shown);
                    match leaf.fullscreen_mode {
                        Some(2) => global.push(at),
                        Some(1) => fullscreen.push(at),
                        _ => normal.push(at),
                    }
                }
            }
        }
        Ok(normal
            .into_iter()
            .chain(fullscreen)
            .chain(global)
            .rev()
            .filter_map(|(leaf, scale, shown)| Some((leaf.window(scale)?, shown)))
            .collect())
    }

    /// Sway's IPC socket.
    pub(crate) fn socket(&self) -> &std::path::Path {
        &self.socket
    }

    /// A `get_tree` reply.
    pub(crate) fn tree(&self) -> Result<Vec<u8>, GeometryError> {
        self.request(SWAY_GET_TREE)
    }

    fn request(&self, kind: u32) -> Result<Vec<u8>, GeometryError> {
        let mut stream = connect(&self.socket)?;
        let mut msg = Vec::with_capacity(14);
        msg.extend_from_slice(SWAY_MAGIC);
        msg.extend_from_slice(&0u32.to_ne_bytes());
        msg.extend_from_slice(&kind.to_ne_bytes());
        stream.write_all(&msg)?;
        let mut header = [0u8; 14];
        stream.read_exact(&mut header)?;
        if &header[..6] != SWAY_MAGIC {
            return Err(GeometryError::Parse("not an i3-ipc reply".into()));
        }
        let len = u32::from_ne_bytes(header[6..10].try_into().expect("4 bytes"));
        let mut payload = vec![0; len as usize];
        stream.read_exact(&mut payload)?;
        Ok(payload)
    }
}

impl WindowGeometry for SwayIpc {
    fn windows(&self) -> Result<Vec<Window>, GeometryError> {
        SwayIpc::parse_tree(&self.request(SWAY_GET_TREE)?)
    }

    fn name(&self) -> &'static str {
        "Sway IPC"
    }

    fn all_windows(&self) -> Result<Vec<Window>, GeometryError> {
        SwayIpc::parse_tree_all(&self.request(SWAY_GET_TREE)?)
    }

    fn focused_output(&self) -> Result<Option<String>, GeometryError> {
        SwayIpc::parse_focused_output(&self.request(SWAY_GET_OUTPUTS)?)
    }
}

#[derive(Debug, Deserialize)]
struct SwayRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Debug, Deserialize)]
struct SwayWindowProperties {
    class: Option<String>,
}

/// What a node of Sway's tree is; only windows matter here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SwayNodeType {
    Con,
    FloatingCon,
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct SwayNode {
    #[serde(rename = "type")]
    kind: SwayNodeType,
    name: Option<String>,
    app_id: Option<String>,
    window_properties: Option<SwayWindowProperties>,
    rect: SwayRect,
    window_rect: Option<SwayRect>,
    visible: Option<bool>,
    fullscreen_mode: Option<u8>,
    #[serde(default)]
    nodes: Vec<SwayNode>,
    #[serde(default)]
    floating_nodes: Vec<SwayNode>,
    foreign_toplevel_identifier: Option<String>,
    /// Outputs only.
    scale: Option<f64>,
}

impl SwayNode {
    /// The windows in this subtree, in paint order.
    fn leaves<'a>(&'a self, out: &mut Vec<&'a SwayNode>) {
        if self.nodes.is_empty() && self.floating_nodes.is_empty() {
            if matches!(self.kind, SwayNodeType::Con | SwayNodeType::FloatingCon) {
                out.push(self);
            }
            return;
        }
        for n in self.nodes.iter().chain(&self.floating_nodes) {
            n.leaves(out);
        }
    }

    fn window(&self, scale: Option<f64>) -> Option<Window> {
        // `window_rect` is the content, relative to `rect` (which adds the
        // borders; the title bar sits above it in `deco_rect`).
        let content = self.window_rect.as_ref()?;
        if content.width <= 0 || content.height <= 0 {
            return None;
        }
        Some(Window {
            rect: LogicalRect {
                x: self.rect.x + content.x,
                y: self.rect.y + content.y,
                width: content.width as u32,
                height: content.height as u32,
            },
            title: self.name.clone().unwrap_or_default(),
            app_id: self
                .app_id
                .clone()
                .or_else(|| self.window_properties.as_ref()?.class.clone())
                .unwrap_or_default(),
            toplevel: self.foreign_toplevel_identifier.clone(),
            address: None,
            scale,
            may_be_stale: false,
        })
    }
}

// ------------------------------------------------------------ Hyprland ----

/// Hyprland's request socket: `j/clients` and `j/monitors`.
#[derive(Debug, Clone)]
pub struct HyprlandIpc {
    socket: PathBuf,
}

impl HyprlandIpc {
    /// The session's Hyprland, from `$HYPRLAND_INSTANCE_SIGNATURE`.
    pub fn from_env() -> Option<HyprlandIpc> {
        let signature =
            std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").filter(|s| !s.is_empty())?;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").filter(|s| !s.is_empty())?;
        let socket = PathBuf::from(runtime)
            .join("hypr")
            .join(signature)
            .join(".socket.sock");
        Some(HyprlandIpc { socket })
    }

    /// The windows in `j/clients` that are on a workspace some monitor in
    /// `j/monitors` shows, topmost first.
    ///
    /// Hyprland does not report its stacking order, so it is approximated:
    /// special workspaces over normal ones, then pinned, fullscreen, and
    /// floating over tiled windows, and among equals the most recently
    /// focused first.
    pub fn parse(clients: &[u8], monitors: &[u8]) -> Result<Vec<Window>, GeometryError> {
        Ok(HyprlandIpc::parse_all(clients, monitors)?
            .into_iter()
            .filter(|w| !w.may_be_stale)
            .collect())
    }

    /// Every mapped window in `j/clients`: those on screen first, as by
    /// [`parse`](Self::parse), then the rest, which [may be
    /// stale](Window::may_be_stale).
    pub fn parse_all(clients: &[u8], monitors: &[u8]) -> Result<Vec<Window>, GeometryError> {
        let clients: Vec<HyprClient> = serde_json::from_slice(clients)?;
        let monitors: Vec<HyprMonitor> = serde_json::from_slice(monitors)?;
        let shown: Vec<i64> = monitors
            .iter()
            .filter(|m| !m.disabled)
            .flat_map(|m| {
                [
                    m.active_workspace.as_ref().and_then(|w| w.id),
                    m.special_workspace.as_ref().and_then(|w| w.id),
                ]
            })
            .flatten()
            .collect();
        let on_screen = |c: &HyprClient| {
            !c.hidden
                && c.visible.unwrap_or(true)
                && c.workspace.id.is_some_and(|id| shown.contains(&id))
        };
        let mut mapped: Vec<HyprClient> = clients
            .into_iter()
            .filter(|c| c.mapped)
            .filter(|c| c.size[0] > 0 && c.size[1] > 0)
            .collect();
        mapped.sort_by_key(|c| {
            (
                !on_screen(c),
                std::cmp::Reverse(c.workspace.id.is_some_and(|id| id < 0)),
                std::cmp::Reverse(c.pinned),
                std::cmp::Reverse(c.fullscreen.is_real()),
                std::cmp::Reverse(c.floating),
                c.focus_history_id.unwrap_or(i64::MAX),
            )
        });
        Ok(mapped
            .into_iter()
            .map(|c| Window {
                rect: LogicalRect {
                    x: c.at[0],
                    y: c.at[1],
                    width: c.size[0] as u32,
                    height: c.size[1] as u32,
                },
                may_be_stale: !on_screen(&c),
                address: c
                    .address
                    .as_deref()
                    .and_then(|a| u64::from_str_radix(a.trim_start_matches("0x"), 16).ok()),
                scale: monitors
                    .iter()
                    .find(|m| m.id.is_some() && m.id == c.monitor)
                    .and_then(|m| m.scale)
                    .filter(|s| *s > 0.0),
                toplevel: None,
                title: c.title,
                app_id: c.class,
            })
            .collect())
    }

    /// The focused monitor's name in `j/monitors`.
    pub fn parse_focused_output(monitors: &[u8]) -> Result<Option<String>, GeometryError> {
        let monitors: Vec<HyprMonitor> = serde_json::from_slice(monitors)?;
        Ok(monitors
            .into_iter()
            .find(|m| m.focused && !m.disabled)
            .and_then(|m| m.name))
    }

    /// Hyprland's request socket.
    pub(crate) fn socket(&self) -> &std::path::Path {
        &self.socket
    }

    /// One request, answered in full. Hyprland serves this socket
    /// synchronously, so the connection is closed straight away.
    pub(crate) fn request(&self, command: &str) -> Result<Vec<u8>, GeometryError> {
        let mut stream = connect(&self.socket)?;
        stream.write_all(command.as_bytes())?;
        let mut reply = Vec::new();
        stream.read_to_end(&mut reply)?;
        Ok(reply)
    }
}

impl WindowGeometry for HyprlandIpc {
    fn name(&self) -> &'static str {
        "Hyprland IPC"
    }

    fn windows(&self) -> Result<Vec<Window>, GeometryError> {
        let monitors = self.request("j/monitors")?;
        let clients = self.request("j/clients")?;
        HyprlandIpc::parse(&clients, &monitors)
    }

    fn all_windows(&self) -> Result<Vec<Window>, GeometryError> {
        let monitors = self.request("j/monitors")?;
        let clients = self.request("j/clients")?;
        HyprlandIpc::parse_all(&clients, &monitors)
    }

    fn focused_output(&self) -> Result<Option<String>, GeometryError> {
        HyprlandIpc::parse_focused_output(&self.request("j/monitors")?)
    }
}

#[derive(Debug, Deserialize)]
struct HyprWorkspace {
    id: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyprClient {
    /// `0x…`, in hex.
    address: Option<String>,
    monitor: Option<i64>,
    #[serde(default)]
    mapped: bool,
    #[serde(default)]
    hidden: bool,
    visible: Option<bool>,
    at: [i32; 2],
    size: [i32; 2],
    workspace: HyprWorkspace,
    #[serde(default)]
    floating: bool,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    fullscreen: HyprFullscreen,
    #[serde(rename = "focusHistoryID")]
    focus_history_id: Option<i64>,
    #[serde(default)]
    class: String,
    #[serde(default)]
    title: String,
}

/// `fullscreen`: a mode since Hyprland 0.42 (0 none, 1 maximised, 2
/// fullscreen, 3 both), a plain bool before.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum HyprFullscreen {
    Mode(i64),
    Flag(bool),
}

impl Default for HyprFullscreen {
    fn default() -> Self {
        HyprFullscreen::Mode(0)
    }
}

impl HyprFullscreen {
    /// Covering the monitor, over the workspace's other windows.
    fn is_real(&self) -> bool {
        match *self {
            HyprFullscreen::Mode(m) => m >= 2,
            HyprFullscreen::Flag(f) => f,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyprMonitor {
    id: Option<i64>,
    name: Option<String>,
    #[serde(default)]
    focused: bool,
    scale: Option<f64>,
    active_workspace: Option<HyprWorkspace>,
    special_workspace: Option<HyprWorkspace>,
    #[serde(default)]
    disabled: bool,
}
