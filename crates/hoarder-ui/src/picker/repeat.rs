//! Key repeat for the Picker: a held key fires again after the
//! compositor's delay, then at its rate, until it is let go.
//!
//! The compositor only says how fast keys repeat (`wl_keyboard.repeat_info`);
//! repeating them is the client's job, and SCTK does it only on a calloop
//! loop, which the Picker does not run.

use std::time::{Duration, Instant};

/// What a compositor that never says uses: 25 per second after 600 ms
/// (the usual xkb defaults).
const DELAY: Duration = Duration::from_millis(600);
const RATE: u32 = 25;

#[derive(Debug)]
pub(crate) struct Repeat<K> {
    /// The delay before the first repeat and the gap between repeats;
    /// `None` when the compositor turned repeat off.
    timing: Option<(Duration, Duration)>,
    /// The key held, its evdev code, and when it fires next.
    held: Option<(K, u32, Instant)>,
}

impl<K: Clone> Repeat<K> {
    pub(crate) fn new() -> Repeat<K> {
        Repeat {
            timing: Some((DELAY, gap(RATE))),
            held: None,
        }
    }

    /// The compositor's repeat rate (per second, 0 = off) and delay (ms).
    pub(crate) fn set_info(&mut self, rate: u32, delay_ms: u32) {
        self.timing = (rate > 0).then(|| (Duration::from_millis(u64::from(delay_ms)), gap(rate)));
        if self.timing.is_none() {
            self.held = None;
        }
    }

    /// Key `code` went down at `now`; `key` is what it repeats as, or
    /// `None` for a key that does not repeat (it still ends any other
    /// key's repeat, as on any keyboard).
    pub(crate) fn press(&mut self, code: u32, key: Option<K>, now: Instant) {
        self.held = match (key, self.timing) {
            (Some(key), Some((delay, _))) => Some((key, code, now + delay)),
            _ => None,
        };
    }

    /// Key `code` went up.
    pub(crate) fn release(&mut self, code: u32) {
        if self.held.as_ref().is_some_and(|(_, held, _)| *held == code) {
            self.held = None;
        }
    }

    /// Stop repeating (the keyboard left the panel).
    pub(crate) fn stop(&mut self) {
        self.held = None;
    }

    /// When the held key fires next, if one is held.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.held.as_ref().map(|(_, _, at)| *at)
    }

    /// The held key if it is due at `now`. A late check fires once, not
    /// in a burst: the next repeat is a full gap after `now`.
    pub(crate) fn due(&mut self, now: Instant) -> Option<K> {
        let (_, gap) = self.timing?;
        let (key, _, at) = self.held.as_mut()?;
        if now < *at {
            return None;
        }
        *at = (*at + gap).max(now + gap / 2);
        Some(key.clone())
    }
}

fn gap(rate: u32) -> Duration {
    Duration::from_micros(1_000_000 / u64::from(rate.max(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn a_held_key_fires_after_the_delay_then_at_the_rate() {
        let t = Instant::now();
        let mut r = Repeat::new();
        r.set_info(10, 300);
        r.press(103, Some("up"), t);

        assert_eq!(r.due(t + 299 * MS), None);
        assert_eq!(r.due(t + 300 * MS), Some("up"));
        assert_eq!(r.due(t + 350 * MS), None);
        assert_eq!(r.deadline(), Some(t + 400 * MS));
        assert_eq!(r.due(t + 400 * MS), Some("up"));
    }

    #[test]
    fn letting_go_stops_it_but_only_for_the_held_key() {
        let t = Instant::now();
        let mut r = Repeat::new();
        r.press(103, Some("up"), t);

        r.release(108);
        assert!(r.deadline().is_some());
        r.release(103);
        assert_eq!(r.deadline(), None);
        assert_eq!(r.due(t + 10_000 * MS), None);
    }

    #[test]
    fn a_new_key_takes_over_and_a_non_repeating_one_ends_it() {
        let t = Instant::now();
        let mut r = Repeat::new();
        r.press(103, Some("up"), t);
        r.press(108, Some("down"), t);
        assert_eq!(r.due(t + DELAY), Some("down"));

        r.press(1, None, t);
        assert_eq!(r.deadline(), None);
    }

    #[test]
    fn repeat_off_means_no_repeats() {
        let t = Instant::now();
        let mut r = Repeat::new();
        r.press(103, Some("up"), t);
        r.set_info(0, 600);
        assert_eq!(r.deadline(), None);

        r.press(103, Some("up"), t);
        assert_eq!(r.deadline(), None);
    }

    #[test]
    fn a_late_check_fires_once_instead_of_a_burst() {
        let t = Instant::now();
        let mut r = Repeat::new();
        r.set_info(10, 100);
        r.press(103, Some("up"), t);

        assert_eq!(r.due(t + 1000 * MS), Some("up"));
        assert_eq!(r.due(t + 1000 * MS), None);
    }
}
