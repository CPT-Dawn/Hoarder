//! Wayland protocol bindings, capture, data-control and window geometry for Hoarder.
//!
//! [`capture::freeze`] takes the Frozen Frames a Capture is cut from
//! (PLAN.md §4.1, ADR-0003). [`clipboard::Clipboard`] reads, serves and
//! clears Selections over data control (PLAN.md §5). [`watch::Watcher`] is
//! the Daemon's non-blocking view of the Selections (PLAN.md §5.1).

pub mod capture;
pub mod clipboard;
pub mod watch;

pub use capture::{
    Backend, CaptureError, Frozen, FrozenFrame, LogicalRect, ShmFormat, Transform, freeze,
};
