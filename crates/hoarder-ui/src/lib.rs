//! Layer-shell UI plumbing, drawing and widgets for Hoarder's Overlay and Picker
//! (ADR-0007: smithay-client-toolkit, tiny-skia and cosmic-text, no toolkit).
//!
//! [`overlay::run`] shows the Overlay over the Frozen Frames and returns the
//! Target the user picked (PLAN.md §4.2).

mod chrome;
pub mod countdown;
mod geom;
pub mod overlay;
mod select;
mod wait;

pub use overlay::{Options, Outcome, OverlayError, Region};
