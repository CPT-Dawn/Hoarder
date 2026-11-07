//! Auto-paste after a Picker Restore (PLAN.md §5.4).

use hoarder_config::Picker;
use hoarder_wayland::paste::{self, Pasted};

/// The Restored Clip is on the clipboard and the Picker is off the screen:
/// paste it into the window that had focus, if `picker.auto_paste` says so.
/// A paste that cannot happen only costs a warning; the Restore stands.
pub fn after_restore(picker: &Picker) {
    if !picker.auto_paste {
        return;
    }
    match paste::paste(&picker.terminal_apps) {
        Ok(Pasted::Into { .. } | Pasted::NoWindow) => {}
        Err(e) => eprintln!("hoarder: restored, but not pasted: {e}"),
    }
}
