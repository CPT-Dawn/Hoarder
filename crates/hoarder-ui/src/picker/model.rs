//! What the Picker shows, apart from how it is drawn: the query, which
//! Clips match it and in what order, and which one is selected.
//!
//! Matching (PLAN.md §6) is a fuzzy match (nucleo) over every preview,
//! best first and newest first among equals, followed by the Clips only
//! the full-text search found (long text whose match lies past its
//! preview), newest first.

use std::collections::HashMap;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use super::Entry;

/// A query split into its filter prefixes (`img:`, `txt:`, `file:`,
/// `pin:`, `app:<id>`) and the text to search for.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Filter {
    /// Kinds to keep (any of them); every kind if empty.
    kinds: Vec<&'static str>,
    pinned: bool,
    /// Lowercased parts of the source app id to keep (all of them).
    apps: Vec<String>,
    /// What is left once the filters are taken out, words joined by one
    /// space.
    pub(crate) text: String,
}

impl Filter {
    pub(crate) fn parse(query: &str) -> Filter {
        let mut filter = Filter::default();
        let mut words = Vec::new();
        for word in query.split_whitespace() {
            match word {
                "img:" => filter.kinds.push("image"),
                "txt:" => filter.kinds.push("text"),
                "file:" => filter.kinds.push("files"),
                "pin:" => filter.pinned = true,
                _ => match word.strip_prefix("app:") {
                    // `app:` alone is a filter still being typed.
                    Some("") => {}
                    Some(app) => filter.apps.push(app.to_lowercase()),
                    None => words.push(word),
                },
            }
        }
        filter.text = words.join(" ");
        filter
    }

    fn keeps(&self, entry: &Entry) -> bool {
        (self.kinds.is_empty() || self.kinds.contains(&entry.kind.as_str()))
            && (!self.pinned || entry.pinned)
            && self.apps.iter().all(|want| {
                entry
                    .app
                    .as_ref()
                    .is_some_and(|app| app.to_lowercase().contains(want))
            })
    }
}

#[derive(Debug)]
pub(crate) struct Model {
    /// Every Clip, most recently used first.
    entries: Vec<Entry>,
    by_id: HashMap<i64, usize>,
    matcher: Matcher,
    query: String,
    /// What matches the query, best first: indices into `entries`.
    shown: Vec<usize>,
    /// An index into `shown`.
    selected: usize,
    /// The first of `shown` on screen.
    top: usize,
}

impl Model {
    pub(crate) fn new(entries: Vec<Entry>) -> Model {
        let by_id = entries.iter().enumerate().map(|(i, e)| (e.id, i)).collect();
        Model {
            shown: (0..entries.len()).collect(),
            entries,
            by_id,
            matcher: Matcher::new(Config::DEFAULT),
            query: String::new(),
            selected: 0,
            top: 0,
        }
    }

    pub(crate) fn query(&self) -> &str {
        &self.query
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Match `query`; `found` are the ids the full-text search found for
    /// its text ([`Filter::text`]). The selection goes back to the best
    /// match.
    pub(crate) fn set_query(&mut self, query: &str, found: &[i64]) {
        self.query = query.to_owned();
        self.selected = 0;
        self.top = 0;
        let filter = Filter::parse(query);
        let kept = |e: &Entry| filter.keeps(e);
        if filter.text.is_empty() {
            self.shown = (0..self.entries.len())
                .filter(|&i| kept(&self.entries[i]))
                .collect();
            return;
        }
        let pattern = Pattern::parse(&filter.text, CaseMatching::Smart, Normalization::Smart);
        let mut buf = Vec::new();
        let mut scored: Vec<(u32, usize)> = self
            .entries
            .iter()
            .enumerate()
            // A Concealed Clip's text is never searched.
            .filter(|(_, e)| kept(e) && !e.concealed)
            .filter_map(|(i, e)| {
                let score = pattern.score(Utf32Str::new(&e.preview, &mut buf), &mut self.matcher);
                score.map(|s| (s, i))
            })
            .collect();
        // Best first; among equals the newer (smaller index) first.
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut seen = vec![false; self.entries.len()];
        self.shown = scored.into_iter().map(|(_, i)| i).collect();
        for &i in &self.shown {
            seen[i] = true;
        }
        let mut extra: Vec<usize> = found
            .iter()
            .filter_map(|id| self.by_id.get(id).copied())
            .filter(|&i| kept(&self.entries[i]) && !self.entries[i].concealed)
            .filter(|&i| !std::mem::replace(&mut seen[i], true))
            .collect();
        extra.sort_unstable();
        self.shown.extend(extra);
    }

    /// Move the selection by `delta` rows, stopping at either end.
    pub(crate) fn move_by(&mut self, delta: isize) {
        let last = self.shown.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    pub(crate) fn selected(&self) -> Option<&Entry> {
        self.shown.get(self.selected).map(|&i| &self.entries[i])
    }

    /// Scroll a list `rows` tall so the selection shows.
    fn scroll(&mut self, rows: usize) {
        let rows = rows.max(1);
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + rows {
            self.top = self.selected + 1 - rows;
        }
    }

    /// The Clip on the `n`th row (from 0) of a list `rows` tall, as it is
    /// on screen.
    pub(crate) fn row(&mut self, n: usize, rows: usize) -> Option<&Entry> {
        self.scroll(rows);
        if n >= rows {
            return None;
        }
        let i = *self.shown.get(self.top + n)?;
        Some(&self.entries[i])
    }

    /// Mark Clip `id` pinned or not.
    pub(crate) fn set_pinned(&mut self, id: i64, pinned: bool) {
        if let Some(&i) = self.by_id.get(&id) {
            self.entries[i].pinned = pinned;
        }
    }

    /// Drop Clip `id`; the selection moves to the Clip after it, or the
    /// one before at the end.
    pub(crate) fn remove(&mut self, id: i64) {
        let Some(gone) = self.by_id.remove(&id) else {
            return;
        };
        self.entries.remove(gone);
        self.by_id = (self.entries.iter().enumerate())
            .map(|(i, e)| (e.id, i))
            .collect();
        self.shown.retain(|&i| i != gone);
        for i in &mut self.shown {
            if *i > gone {
                *i -= 1;
            }
        }
        self.selected = self.selected.min(self.shown.len().saturating_sub(1));
        self.top = self.top.min(self.selected);
    }

    /// The Clips on screen in a list `rows` tall, scrolled so the selection
    /// shows, with whether each is the selected one.
    pub(crate) fn visible(&mut self, rows: usize) -> Vec<(&Entry, bool)> {
        let rows = rows.max(1);
        self.scroll(rows);
        self.shown
            .iter()
            .enumerate()
            .skip(self.top)
            .take(rows)
            .map(|(n, &i)| (&self.entries[i], n == self.selected))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(previews: &[&str]) -> Model {
        Model::new(
            previews
                .iter()
                .enumerate()
                .map(|(i, p)| Entry {
                    id: 100 + i as i64,
                    kind: "text".into(),
                    preview: (*p).into(),
                    app: None,
                    at_ms: None,
                    pinned: false,
                    concealed: false,
                })
                .collect(),
        )
    }

    fn shown(m: &mut Model) -> Vec<String> {
        m.visible(100)
            .into_iter()
            .map(|(e, _)| e.preview.clone())
            .collect()
    }

    #[test]
    fn with_no_query_everything_shows_newest_first_and_the_newest_is_selected() {
        let mut m = model(&["newest", "older", "oldest"]);

        assert_eq!(shown(&mut m), ["newest", "older", "oldest"]);
        assert_eq!(m.selected().unwrap().id, 100);
    }

    #[test]
    fn a_query_keeps_only_fuzzy_matches_best_first() {
        let mut m = model(&["s, r and c", "hoarder", "cargo", "src/main.rs"]);

        m.set_query("src", &[]);

        // The older but contiguous match outranks the scattered one.
        assert_eq!(shown(&mut m), ["src/main.rs", "s, r and c"]);
    }

    #[test]
    fn equal_matches_keep_newest_first() {
        let mut m = model(&["banana split", "apple", "banana bread"]);

        m.set_query("banana", &[]);

        assert_eq!(shown(&mut m), ["banana split", "banana bread"]);
    }

    #[test]
    fn full_text_hits_follow_fuzzy_hits_newest_first_without_repeats() {
        let mut m = model(&["new long text…", "needle", "x", "old long text…"]);

        m.set_query("needle", &[103, 101, 100, 999]);

        assert_eq!(
            shown(&mut m),
            ["needle", "new long text…", "old long text…"]
        );
    }

    #[test]
    fn the_selection_stops_at_both_ends_and_resets_on_a_new_query() {
        let mut m = model(&["a1", "a2", "a3"]);
        m.move_by(-1);
        assert_eq!(m.selected().unwrap().preview, "a1");
        m.move_by(10);
        assert_eq!(m.selected().unwrap().preview, "a3");

        m.set_query("a", &[]);

        assert_eq!(m.selected().unwrap().preview, "a1");
    }

    #[test]
    fn rows_count_from_the_top_of_the_scrolled_list() {
        let mut m = model(&["r0", "r1", "r2", "r3", "r4"]);
        assert_eq!(m.row(1, 3).unwrap().preview, "r1");
        m.move_by(4);

        assert_eq!(m.row(0, 3).unwrap().preview, "r2");
        assert!(m.row(3, 3).is_none());
    }

    #[test]
    fn removing_a_clip_selects_the_next_one_and_keeps_the_order() {
        let mut m = model(&["a", "b", "c"]);
        m.move_by(1);

        m.remove(101);

        assert_eq!(m.selected().unwrap().preview, "c");
        assert_eq!(shown(&mut m), ["a", "c"]);
        m.remove(102);
        assert_eq!(m.selected().unwrap().preview, "a");
        m.set_query("a", &[]);
        assert_eq!(shown(&mut m), ["a"]);
    }

    #[test]
    fn nothing_is_selected_when_nothing_matches() {
        let mut m = model(&["alpha"]);
        m.set_query("zzzz", &[]);
        m.move_by(1);

        assert!(m.selected().is_none());
        assert!(shown(&mut m).is_empty());
    }

    #[test]
    fn the_list_scrolls_to_keep_the_selection_on_screen() {
        let mut m = model(&["r0", "r1", "r2", "r3", "r4"]);
        m.move_by(3);

        let rows = m.visible(2);

        let got: Vec<_> = rows.iter().map(|(e, s)| (e.preview.as_str(), *s)).collect();
        assert_eq!(got, [("r2", false), ("r3", true)]);
        m.move_by(-3);
        assert_eq!(m.visible(2)[0].0.preview, "r0");
    }
}
