//! Everything that follows a delivered Capture: hook, notification, sound
//! (PLAN.md §4.1 step 7). None of it can fail the Capture; problems are logged.

use std::path::PathBuf;

use hoarder_config::Screenshot;

/// A Capture that has reached the clipboard and/or disk.
#[derive(Debug)]
pub struct Delivered {
    /// The file it was written to, if any (not for `-o -` or `save = false`).
    pub path: Option<PathBuf>,
    pub width: u32,
    pub height: u32,
    /// `all`, `region`, `window` or `output`.
    pub target: &'static str,
}

pub fn run(capture: &Delivered, settings: &Screenshot) {
    crate::hook::spawn(&settings.on_capture, capture);
    if settings.notify {
        crate::notify::spawn(capture);
    }
}
