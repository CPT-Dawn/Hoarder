//! Layer-shell UI plumbing, drawing and widgets for Hoarder's Overlay and Picker
//! (ADR-0007: smithay-client-toolkit, tiny-skia and cosmic-text, no toolkit).
//!
//! [`overlay::run`] shows the Overlay over the Frozen Frames and returns the
//! Target the user picked, a clicked Window included (PLAN.md §4.2).
//! [`picker::run`] shows the Picker over History and returns the Clip the
//! user picked (PLAN.md §6).

mod chrome;
pub mod countdown;
pub mod flash;
mod geom;
mod icon;
mod layers;
mod loupe;
pub mod overlay;
pub mod picker;
mod select;
mod snap;
mod sprite;
mod surface;
pub mod theme;

pub use overlay::{Options, Outcome, Region};
pub use surface::{Error, TIMINGS_ENV};
pub use theme::Theme;
