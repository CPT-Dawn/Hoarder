//! Wayland protocol bindings, capture, data-control and window geometry for Hoarder.
//!
//! [`capture::freeze`] takes the Frozen Frames a Capture is cut from
//! (PLAN.md §4.1, ADR-0003). [`clipboard::Clipboard`] reads, serves and
//! clears Selections over data control (PLAN.md §5). [`watch::Watcher`] is
//! the Daemon's non-blocking view of the Selections (PLAN.md §5.1).
//! [`flash::flash`] is the optional camera flash. [`windows::detect`] finds
//! the window-geometry provider the Overlay snaps to (PLAN.md §4.2).

pub mod capture;
pub mod clipboard;
pub mod flash;
pub mod watch;
pub mod windows;

pub use capture::{
    Backend, CaptureError, Frozen, FrozenFrame, LogicalRect, ShmFormat, Transform, freeze,
};
pub use windows::{GeometryError, Window, WindowGeometry};
