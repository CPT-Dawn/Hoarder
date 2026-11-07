//! `$XDG_STATE_HOME/hoarder/state.toml`: what Hoarder remembers between runs
//! that is not a setting (PLAN.md §4.2), such as the last Region.
//!
//! Keys this version does not know are kept as they are, so features can add
//! their own without stepping on each other.

use std::path::PathBuf;

use toml_edit::{DocumentMut, Item, Table, value};

/// `$XDG_STATE_HOME/hoarder/state.toml`, or `~/.local/state/hoarder/state.toml`.
pub fn state_path() -> PathBuf {
    let dir = match std::env::var_os("XDG_STATE_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => crate::home().join(".local").join("state"),
    };
    dir.join("hoarder").join("state.toml")
}

/// After this many showings the Overlay's hint bar retires (PLAN.md §4.2).
pub const HINT_BAR_RETIRES_AFTER: u32 = 20;

const HINT_BAR_USES: &str = "hint_bar_uses";

/// A Region as stored: global logical coordinates of the compositor layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StoredRegion {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The contents of `state.toml`. A missing or unreadable file is empty state:
/// losing it costs a convenience, never a Capture.
#[derive(Debug, Clone, Default)]
pub struct State {
    doc: DocumentMut,
}

impl State {
    /// Read [`state_path`].
    pub fn load() -> State {
        let doc = std::fs::read_to_string(state_path())
            .ok()
            .and_then(|text| text.parse().ok())
            .unwrap_or_default();
        State { doc }
    }

    /// Write [`state_path`] (atomically, so a concurrent run never reads half a file).
    pub fn save(&self) -> std::io::Result<()> {
        crate::write_atomic(&state_path(), self.doc.to_string())
    }

    /// The Region of the last Region Capture.
    pub fn last_region(&self) -> Option<StoredRegion> {
        let t = self.doc.get("last_region")?.as_table_like()?;
        let num = |k: &str| {
            let v = t.get(k)?;
            v.as_float().or_else(|| v.as_integer().map(|i| i as f64))
        };
        let region = StoredRegion {
            x: num("x")?,
            y: num("y")?,
            width: num("width")?,
            height: num("height")?,
        };
        (region.width > 0.0 && region.height > 0.0).then_some(region)
    }

    /// How many times the Overlay has shown its hint bar.
    pub fn hint_bar_uses(&self) -> u32 {
        self.doc
            .get(HINT_BAR_USES)
            .and_then(Item::as_integer)
            .map_or(0, |n| n.clamp(0, u32::MAX as i64) as u32)
    }

    /// Whether the hint bar has uses left ([`HINT_BAR_RETIRES_AFTER`]).
    pub fn hint_bar_due(&self) -> bool {
        self.hint_bar_uses() < HINT_BAR_RETIRES_AFTER
    }

    /// Count one more showing of the hint bar.
    pub fn record_hint_bar_use(&mut self) {
        let uses = self.hint_bar_uses().saturating_add(1) as i64;
        match self.doc.get_mut(HINT_BAR_USES).and_then(Item::as_value_mut) {
            // Keep the comments around the old value.
            Some(old) => {
                let decor = old.decor().clone();
                *old = uses.into();
                *old.decor_mut() = decor;
            }
            None => self.doc[HINT_BAR_USES] = value(uses),
        }
    }

    pub fn set_last_region(&mut self, r: StoredRegion) {
        let mut t = Table::new();
        t["x"] = value(r.x);
        t["y"] = value(r.y);
        t["width"] = value(r.width);
        t["height"] = value(r.height);
        self.doc["last_region"] = Item::Table(t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_region_round_trips_and_other_keys_survive() {
        let mut state = State {
            doc: "hint_uses = 3\n".parse().unwrap(),
        };
        let r = StoredRegion {
            x: -10.5,
            y: 20.0,
            width: 300.0,
            height: 200.25,
        };
        state.set_last_region(r);
        let text = state.doc.to_string();
        let back = State {
            doc: text.parse().unwrap(),
        };
        assert_eq!(back.last_region(), Some(r));
        assert!(text.contains("hint_uses = 3"), "{text}");
    }

    #[test]
    fn the_hint_bar_counts_its_uses_and_keeps_comments() {
        let mut state = State::default();
        assert_eq!(state.hint_bar_uses(), 0);
        state.record_hint_bar_use();
        assert_eq!(state.doc.to_string(), "hint_bar_uses = 1\n");

        let mut state = State {
            doc: "# mine\nhint_bar_uses = 19 # nearly\n".parse().unwrap(),
        };
        assert!(state.hint_bar_due());
        state.record_hint_bar_use();
        assert_eq!(
            state.doc.to_string(),
            "# mine\nhint_bar_uses = 20 # nearly\n"
        );
        assert!(!state.hint_bar_due());
    }
}
