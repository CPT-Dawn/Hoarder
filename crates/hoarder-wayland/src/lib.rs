//! Wayland protocol bindings, capture, data-control and window geometry for Hoarder.
//!
//! [`capture::freeze`] takes the Frozen Frames a Capture is cut from
//! (PLAN.md §4.1, ADR-0003).

pub mod capture;

pub use capture::{
    Backend, CaptureError, Frozen, FrozenFrame, LogicalRect, ShmFormat, Transform, freeze,
};
