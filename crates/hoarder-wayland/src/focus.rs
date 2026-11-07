//! The focused app, as the compositor's own IPC reports it (PLAN.md §5.1):
//! Hyprland's `activewindowv2` events, or Sway's `window` `focus` events.
//! The Classifier uses it for `privacy.exclude_apps`; where neither IPC is
//! there, `wlr-foreign-toplevel` activation (in [`crate::watch`]) still is.
//!
//! [`FocusWatch`] is non-blocking: poll its fd in an event loop and call
//! [`FocusWatch::read`] when it is readable.

use std::io::{ErrorKind, Read as _, Write as _};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use crate::windows::{HyprlandIpc, SwayIpc};

/// i3-ipc message type `SUBSCRIBE`, and the `window` event's type.
const SWAY_SUBSCRIBE: u32 = 2;
const SWAY_WINDOW_EVENT: u32 = 0x8000_0003;
const SWAY_MAGIC: &[u8; 6] = b"i3-ipc";
/// The most event bytes held while a frame or line is incomplete.
const MAX_PENDING: usize = 4 * 1024 * 1024;

/// Which IPC the events come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Hyprland,
    Sway,
}

/// Follows focus changes from a stream of event bytes.
#[derive(Debug)]
pub struct FocusTracker {
    source: Source,
    buf: Vec<u8>,
    focused: Option<String>,
    /// Hyprland: the class from the latest `activewindow`, which
    /// `activewindowv2` (sent right after it) commits.
    pending: Option<String>,
    /// Hyprland: the focused window's address, to notice it closing.
    address: Option<String>,
}

impl FocusTracker {
    /// A tracker for `source`, starting with `focused` as the focused app.
    pub fn new(source: Source, focused: Option<String>) -> FocusTracker {
        FocusTracker {
            source,
            buf: Vec::new(),
            focused,
            pending: None,
            address: None,
        }
    }

    /// The app_id (Wayland) or class (X11) of the focused window, if any.
    pub fn focused_app(&self) -> Option<&str> {
        self.focused.as_deref()
    }

    /// Take in more of the event stream.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
        match self.source {
            Source::Hyprland => self.hyprland_lines(),
            Source::Sway => self.sway_frames(),
        }
        if self.buf.len() > MAX_PENDING {
            // Not an event stream we understand: start over.
            self.buf.clear();
        }
    }

    /// `EVENT>>DATA` lines.
    fn hyprland_lines(&mut self) {
        while let Some(end) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            let Some((event, data)) = line.trim_end().split_once(">>") else {
                continue;
            };
            match event {
                // `CLASS,TITLE`; the title may hold commas, the class not.
                "activewindow" => {
                    let class = data.split(',').next().unwrap_or_default();
                    self.pending = (!class.is_empty()).then(|| class.to_owned());
                }
                // `ADDRESS`, empty (`,`) when nothing has focus.
                "activewindowv2" => {
                    let address = data.trim_matches(',');
                    self.focused = if address.is_empty() {
                        None
                    } else {
                        self.pending.take()
                    };
                    self.address = (!address.is_empty()).then(|| address.to_owned());
                }
                // The focused window went away with nothing focused after it.
                "closewindow" if self.address.as_deref() == Some(data) => {
                    self.focused = None;
                    self.address = None;
                }
                _ => {}
            }
        }
    }

    /// i3-ipc frames: magic, length, type, JSON.
    fn sway_frames(&mut self) {
        while self.buf.len() >= 14 {
            if &self.buf[..6] != SWAY_MAGIC {
                self.buf.clear();
                return;
            }
            let len = u32::from_ne_bytes(self.buf[6..10].try_into().expect("4 bytes")) as usize;
            let kind = u32::from_ne_bytes(self.buf[10..14].try_into().expect("4 bytes"));
            if self.buf.len() < 14 + len {
                return;
            }
            let payload: Vec<u8> = self.buf.drain(..14 + len).skip(14).collect();
            if kind != SWAY_WINDOW_EVENT {
                continue;
            }
            let Ok(event) = serde_json::from_slice::<serde_json::Value>(&payload) else {
                continue;
            };
            let app = sway_app(&event["container"]);
            match event["change"].as_str() {
                Some("focus") => self.focused = app,
                // Closing the focused window focuses nothing, unless a focus
                // event for another one follows.
                Some("close") if app.is_some() && app == self.focused => self.focused = None,
                _ => {}
            }
        }
    }
}

/// A Sway container's app_id, or its X11 class.
fn sway_app(container: &serde_json::Value) -> Option<String> {
    container["app_id"]
        .as_str()
        .or_else(|| container["window_properties"]["class"].as_str())
        .map(str::to_owned)
}

/// The focused window's app in a Sway `get_tree` reply.
pub fn sway_focused_in_tree(tree: &serde_json::Value) -> Option<String> {
    if tree["focused"] == true && matches!(tree["type"].as_str(), Some("con" | "floating_con")) {
        return sway_app(tree);
    }
    ["nodes", "floating_nodes"]
        .iter()
        .filter_map(|k| tree[*k].as_array())
        .flatten()
        .find_map(sway_focused_in_tree)
}

/// A live subscription to focus changes.
#[derive(Debug)]
pub struct FocusWatch {
    stream: UnixStream,
    tracker: FocusTracker,
}

impl FocusWatch {
    /// Subscribe to the session's compositor: Hyprland when
    /// `HYPRLAND_INSTANCE_SIGNATURE` is set, Sway when `SWAYSOCK` is. `None`
    /// under any other compositor.
    pub fn connect() -> Option<std::io::Result<FocusWatch>> {
        if let Some(hypr) = HyprlandIpc::from_env() {
            return Some(FocusWatch::hyprland(&hypr));
        }
        SwayIpc::from_env().map(|sway| FocusWatch::sway(&sway))
    }

    fn hyprland(hypr: &HyprlandIpc) -> std::io::Result<FocusWatch> {
        let events: PathBuf = hypr.socket().with_file_name(".socket2.sock");
        let stream = UnixStream::connect(events)?;
        let focused = hypr
            .request("j/activewindow")
            .ok()
            .and_then(|r| serde_json::from_slice::<serde_json::Value>(&r).ok())
            .and_then(|w| w["class"].as_str().map(str::to_owned))
            .filter(|c| !c.is_empty());
        FocusWatch::start(stream, FocusTracker::new(Source::Hyprland, focused))
    }

    fn sway(sway: &SwayIpc) -> std::io::Result<FocusWatch> {
        let mut stream = UnixStream::connect(sway.socket())?;
        let payload = br#"["window"]"#;
        let mut msg = Vec::with_capacity(14 + payload.len());
        msg.extend_from_slice(SWAY_MAGIC);
        msg.extend_from_slice(&(payload.len() as u32).to_ne_bytes());
        msg.extend_from_slice(&SWAY_SUBSCRIBE.to_ne_bytes());
        msg.extend_from_slice(payload);
        stream.write_all(&msg)?;
        // The tree after subscribing, so no focus change falls between.
        let focused = sway
            .tree()
            .ok()
            .and_then(|t| serde_json::from_slice::<serde_json::Value>(&t).ok())
            .and_then(|t| sway_focused_in_tree(&t));
        FocusWatch::start(stream, FocusTracker::new(Source::Sway, focused))
    }

    fn start(stream: UnixStream, tracker: FocusTracker) -> std::io::Result<FocusWatch> {
        stream.set_nonblocking(true)?;
        Ok(FocusWatch { stream, tracker })
    }

    /// Read every event waiting. `Ok(false)` once the compositor has closed
    /// the stream.
    pub fn read(&mut self) -> std::io::Result<bool> {
        let mut chunk = [0u8; 8192];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => return Ok(false),
                Ok(n) => self.tracker.feed(&chunk[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(true),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }

    /// The focused app as of the events read so far.
    pub fn focused_app(&self) -> Option<&str> {
        self.tracker.focused_app()
    }
}

impl AsFd for FocusWatch {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.stream.as_fd()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyprland_commits_the_class_on_activewindowv2() {
        let mut t = FocusTracker::new(Source::Hyprland, None);
        t.feed(b"workspace>>2\nactivewindow>>org.keepassxc.KeePassXC,Pass, words\nactivewin");
        assert_eq!(t.focused_app(), None, "not committed before activewindowv2");
        t.feed(b"dowv2>>55764847fa40\n");
        assert_eq!(t.focused_app(), Some("org.keepassxc.KeePassXC"));
        t.feed(b"activewindow>>,\nactivewindowv2>>,\n");
        assert_eq!(t.focused_app(), None);
        t.feed(b"activewindow>>kitty,~\nactivewindowv2>>abc\ncloseindow>>abc\n");
        assert_eq!(t.focused_app(), Some("kitty"));
        t.feed(b"closewindow>>abc\n");
        assert_eq!(t.focused_app(), None, "its window closed");
    }

    fn sway_event(kind: u32, json: &str) -> Vec<u8> {
        let mut frame = SWAY_MAGIC.to_vec();
        frame.extend_from_slice(&(json.len() as u32).to_ne_bytes());
        frame.extend_from_slice(&kind.to_ne_bytes());
        frame.extend_from_slice(json.as_bytes());
        frame
    }

    #[test]
    fn sway_follows_focus_events_split_anywhere() {
        let mut t = FocusTracker::new(Source::Sway, Some("foot".into()));
        let mut stream = sway_event(SWAY_SUBSCRIBE, r#"{"success":true}"#);
        stream.extend(sway_event(
            SWAY_WINDOW_EVENT,
            r#"{"change":"title","container":{"app_id":"nope"}}"#,
        ));
        stream.extend(sway_event(
            SWAY_WINDOW_EVENT,
            r#"{"change":"focus","container":{"app_id":null,"window_properties":{"class":"KeePassXC"}}}"#,
        ));
        let (a, b) = stream.split_at(20);
        t.feed(a);
        assert_eq!(t.focused_app(), Some("foot"));
        t.feed(b);
        assert_eq!(t.focused_app(), Some("KeePassXC"));
        t.feed(&sway_event(
            SWAY_WINDOW_EVENT,
            r#"{"change":"close","container":{"window_properties":{"class":"KeePassXC"}}}"#,
        ));
        assert_eq!(t.focused_app(), None, "its window closed");
    }

    #[test]
    fn the_focused_leaf_of_a_sway_tree() {
        let tree: serde_json::Value = serde_json::from_str(
            r#"{"type":"root","nodes":[{"type":"output","nodes":[{"type":"workspace",
            "focused":false,"nodes":[{"type":"con","focused":false,"app_id":"a"}],
            "floating_nodes":[{"type":"floating_con","focused":true,"app_id":"b"}]}]}]}"#,
        )
        .unwrap();
        assert_eq!(sway_focused_in_tree(&tree).as_deref(), Some("b"));
    }
}
