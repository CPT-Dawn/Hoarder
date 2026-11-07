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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    /// The window's content (no borders or title bar), in global logical
    /// coordinates.
    pub rect: LogicalRect,
    pub title: String,
    /// The Wayland app_id, or the X11 class for Xwayland windows.
    pub app_id: String,
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

/// i3-ipc message type `GET_TREE`.
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
        let root: SwayNode = serde_json::from_slice(json)?;
        // Paint order, bottom first; reversed at the end.
        let mut normal = Vec::new();
        let mut fullscreen = Vec::new();
        let mut global = Vec::new();
        for output in root
            .nodes
            .iter()
            .filter(|o| o.name.as_deref() != Some("__i3"))
        {
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
                    match leaf.fullscreen_mode {
                        Some(2) => global.push(leaf),
                        Some(1) => fullscreen.push(leaf),
                        _ => normal.push(leaf),
                    }
                }
            }
        }
        Ok(normal
            .into_iter()
            .chain(fullscreen)
            .chain(global)
            .rev()
            .filter(|n| n.visible == Some(true))
            .filter_map(SwayNode::window)
            .collect())
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

#[derive(Debug, Deserialize)]
struct SwayNode {
    #[serde(rename = "type")]
    kind: String,
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
}

impl SwayNode {
    /// The windows in this subtree, in paint order.
    fn leaves<'a>(&'a self, out: &mut Vec<&'a SwayNode>) {
        if self.nodes.is_empty() && self.floating_nodes.is_empty() {
            if matches!(self.kind.as_str(), "con" | "floating_con") {
                out.push(self);
            }
            return;
        }
        for n in self.nodes.iter().chain(&self.floating_nodes) {
            n.leaves(out);
        }
    }

    fn window(&self) -> Option<Window> {
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
        let mut on_screen: Vec<HyprClient> = clients
            .into_iter()
            .filter(|c| c.mapped && !c.hidden && c.visible.unwrap_or(true))
            .filter(|c| c.workspace.id.is_some_and(|id| shown.contains(&id)))
            .filter(|c| c.size[0] > 0 && c.size[1] > 0)
            .collect();
        on_screen.sort_by_key(|c| {
            (
                std::cmp::Reverse(c.workspace.id.is_some_and(|id| id < 0)),
                std::cmp::Reverse(c.pinned),
                std::cmp::Reverse(c.fullscreen.is_real()),
                std::cmp::Reverse(c.floating),
                c.focus_history_id.unwrap_or(i64::MAX),
            )
        });
        Ok(on_screen
            .into_iter()
            .map(|c| Window {
                rect: LogicalRect {
                    x: c.at[0],
                    y: c.at[1],
                    width: c.size[0] as u32,
                    height: c.size[1] as u32,
                },
                title: c.title,
                app_id: c.class,
            })
            .collect())
    }

    /// One request, answered in full. Hyprland serves this socket
    /// synchronously, so the connection is closed straight away.
    fn request(&self, command: &str) -> Result<Vec<u8>, GeometryError> {
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
}

#[derive(Debug, Deserialize)]
struct HyprWorkspace {
    id: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HyprClient {
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
    active_workspace: Option<HyprWorkspace>,
    special_workspace: Option<HyprWorkspace>,
    #[serde(default)]
    disabled: bool,
}
