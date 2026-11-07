//! Layer-shell UI plumbing, drawing and widgets for Hoarder's Overlay and Picker
//! (ADR-0007: smithay-client-toolkit, tiny-skia and cosmic-text, no toolkit).
//!
//! [`overlay::run`] shows the Overlay over the Frozen Frames and returns the
//! Region the user dragged out (PLAN.md §4.2).

mod chrome;
mod geom;
pub mod overlay;
mod wait;

pub use overlay::{Outcome, OverlayError, Region};
