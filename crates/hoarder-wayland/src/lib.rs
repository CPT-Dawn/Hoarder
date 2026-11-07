//! Wayland protocol bindings, capture, data-control and window geometry for Hoarder.
//!
//! [`capture::freeze`] takes the Frozen Frames a Capture is cut from
//! (PLAN.md §4.1, ADR-0003). [`clipboard::Clipboard`] reads, serves and
//! clears Selections over data control (PLAN.md §5). [`watch::Watcher`] is
//! the Daemon's non-blocking view of the Selections (PLAN.md §5.1).
//! [`focus::FocusWatch`] follows the focused app over compositor IPC.
//! [`windows::detect`] finds the window-geometry provider the Overlay snaps
//! to (PLAN.md §4.2).
//! [`paste::paste`] types the paste chord after a Restore (PLAN.md §5.4).
//! [`probe::globals`] lists what the compositor offers, for `hoarder doctor`.

pub mod capture;
pub mod clipboard;
mod data_control;
pub mod focus;
mod hyprland;
pub mod paste;
pub mod probe;
pub mod toplevel;
pub mod wait;
pub mod watch;
pub mod windows;

pub use capture::{
    Backend, CaptureError, Frozen, FrozenCursor, FrozenFrame, LogicalRect, ShmFormat, Transform,
    freeze,
};
pub use windows::{GeometryError, Window, WindowGeometry};
