//! The selection set.
//!
//! Selection is held as entry **names**, never row indices (DESIGN.md §4.1).
//! Rows renumber whenever the sort changes, the filter narrows, or a
//! background job adds a file; names survive all three. A name that leaves
//! the listing leaves the selection with it.

use crate::entry::Entry;
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};

/// What a range operation should do with entries selected before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeMode {
    /// Shift+move: the new range replaces the previous range.
    Replace,
    /// Ctrl+Shift+click: the new range is added to what is already there.
    Add,
}

/// A selection over one directory listing.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// Selected entry names.
    names: HashSet<OsString>,
    /// Where a range extends from. `None` means "use the cursor".
    anchor: Option<OsString>,
    /// Names selected by an explicit toggle, preserved when a range is
    /// replaced so Ctrl-picks are not lost by a later Shift-drag.
    pinned: HashSet<OsString>,
}

impl Selection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn contains(&self, name: &OsStr) -> bool {
        self.names.contains(name)
    }

    /// Everything selected, in the order it appears in `visible`.
    ///
    /// Order matters: a user who selects three files and copies them expects
    /// the job to report them in the order they saw, not hash order.
    pub fn resolve<'a>(&self, visible: &'a [&'a Entry]) -> Vec<&'a Entry> {
        visible
            .iter()
            .filter(|e| self.names.contains(e.name.as_os_str()))
            .copied()
            .collect()
    }

    /// Clear everything, including the anchor.
    pub fn clear(&mut self) {
        self.names.clear();
        self.pinned.clear();
        self.anchor = None;
    }

    /// Toggle one entry and move the anchor to it (Space, Ctrl+click).
    pub fn toggle(&mut self, name: &OsStr) {
        if self.names.remove(name) {
            self.pinned.remove(name);
        } else {
            self.names.insert(name.to_os_string());
            self.pinned.insert(name.to_os_string());
        }
        self.anchor = Some(name.to_os_string());
    }

    /// Select exactly one entry, discarding everything else (plain click).
    pub fn select_only(&mut self, name: &OsStr) {
        self.names.clear();
        self.pinned.clear();
        self.names.insert(name.to_os_string());
        self.anchor = Some(name.to_os_string());
    }

    /// Select every visible entry (Ctrl+A).
    ///
    /// `visible` is what the user can actually see — filtered and with hidden
    /// files already excluded. Nothing off-screen is ever selected.
    pub fn select_all_visible(&mut self, visible: &[&Entry]) {
        self.names = visible.iter().map(|e| e.name.clone()).collect();
        self.pinned.clear();
    }

    /// `select_all_visible` for a caller that already has the names.
    ///
    /// The bridge holds the model's entries behind a pin, so it passes names
    /// rather than borrows; the behaviour is identical.
    pub fn select_all_by_name(&mut self, names: &[OsString]) {
        self.names = names.iter().cloned().collect();
        self.pinned.clear();
    }

    /// Invert the selection over what is visible (Nautilus, cosmic-files,
    /// Flea #164). Only `visible` is considered: a name the filter hides
    /// never becomes selected, and a hidden selected name is dropped rather
    /// than kept out of sight where the next command would act on it.
    pub fn invert_by_name(&mut self, visible: &[OsString]) {
        let now: HashSet<OsString> = visible
            .iter()
            .filter(|n| !self.names.contains(n.as_os_str()))
            .cloned()
            .collect();
        self.names = now;
        self.pinned.clear();
    }

    /// `select_range` for a caller that already has the names in order.
    pub fn select_range_by_name(&mut self, names: &[OsString], cursor: usize, mode: RangeMode) {
        if names.is_empty() {
            return;
        }
        let cursor = cursor.min(names.len() - 1);
        let anchor = match &self.anchor {
            Some(a) => names.iter().position(|n| n == a).unwrap_or(cursor),
            None => cursor,
        };
        let (lo, hi) = if anchor <= cursor {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        };
        if mode == RangeMode::Replace {
            self.names = self.pinned.clone();
        }
        for n in &names[lo..=hi] {
            self.names.insert(n.clone());
        }
    }

    /// The anchor row within `visible`, or `fallback` when there is none.
    pub fn anchor_row(&self, visible: &[&Entry], fallback: usize) -> usize {
        let Some(a) = &self.anchor else {
            return fallback;
        };
        visible
            .iter()
            .position(|e| e.name == *a)
            .unwrap_or(fallback)
    }

    /// Set the anchor without changing what is selected.
    pub fn set_anchor(&mut self, name: &OsStr) {
        self.anchor = Some(name.to_os_string());
    }

    /// Select the inclusive range between the anchor and `cursor`.
    ///
    /// The anchor does NOT move: repeatedly extending a range must keep
    /// growing from the same origin, which is the whole point of an anchor.
    pub fn select_range(&mut self, visible: &[&Entry], cursor: usize, mode: RangeMode) {
        if visible.is_empty() {
            return;
        }
        let cursor = cursor.min(visible.len() - 1);
        let anchor = self.anchor_row(visible, cursor);
        let anchor = anchor.min(visible.len() - 1);

        let (lo, hi) = if anchor <= cursor {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        };

        if mode == RangeMode::Replace {
            // Keep explicit Ctrl-picks; drop the previous range.
            self.names = self.pinned.clone();
        }
        for e in &visible[lo..=hi] {
            self.names.insert(e.name.clone());
        }
    }

    /// Drop any selected name that is no longer in the listing.
    ///
    /// Called after every reload: a file deleted by another program, or
    /// filtered out of view, must not stay selected and become the silent
    /// target of the next command.
    pub fn retain_existing(&mut self, visible: &[&Entry]) {
        let present: HashSet<&OsStr> = visible.iter().map(|e| e.name.as_os_str()).collect();
        self.names.retain(|n| present.contains(n.as_os_str()));
        self.pinned.retain(|n| present.contains(n.as_os_str()));
        if let Some(a) = &self.anchor {
            if !present.contains(a.as_os_str()) {
                self.anchor = None;
            }
        }
    }

    /// A short human summary for the status line.
    pub fn summary(&self, total_bytes: u64) -> String {
        match self.names.len() {
            0 => String::new(),
            1 => "1 selected".to_string(),
            n if total_bytes > 0 => format!("{n} selected · {}", human_bytes(total_bytes)),
            n => format!("{n} selected"),
        }
    }
}

fn human_bytes(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else if v >= 10.0 {
        format!("{v:.0} {}", U[i])
    } else {
        format!("{v:.1} {}", U[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::EntryKind;

    fn entries(names: &[&str]) -> Vec<Entry> {
        names
            .iter()
            .map(|n| Entry::new(OsString::from(n), EntryKind::File))
            .collect()
    }

    fn visible(all: &[Entry]) -> Vec<&Entry> {
        all.iter().collect()
    }

    #[test]
    fn a_new_selection_is_empty() {
        let s = Selection::new();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
        assert_eq!(s.summary(0), "");
    }

    #[test]
    fn toggle_adds_then_removes() {
        let all = entries(&["a", "b"]);
        let mut s = Selection::new();
        s.toggle(OsStr::new("a"));
        assert!(s.contains(OsStr::new("a")));
        assert_eq!(s.len(), 1);
        s.toggle(OsStr::new("a"));
        assert!(!s.contains(OsStr::new("a")));
        assert!(s.is_empty());
        let _ = visible(&all);
    }

    #[test]
    fn selection_survives_a_re_sort() {
        // The listing is re-sorted; row numbers change, names do not.
        let asc = entries(&["a", "b", "c"]);
        let desc = entries(&["c", "b", "a"]);
        let mut s = Selection::new();
        s.toggle(OsStr::new("a"));
        s.toggle(OsStr::new("c"));

        let before: Vec<_> = s
            .resolve(&visible(&asc))
            .iter()
            .map(|e| e.name.clone())
            .collect();
        assert_eq!(before, vec![OsString::from("a"), OsString::from("c")]);

        // Same selection, different order — still both entries.
        let after: Vec<_> = s
            .resolve(&visible(&desc))
            .iter()
            .map(|e| e.name.clone())
            .collect();
        assert_eq!(after, vec![OsString::from("c"), OsString::from("a")]);
    }

    #[test]
    fn resolve_returns_entries_in_listing_order() {
        let all = entries(&["a", "b", "c", "d"]);
        let mut s = Selection::new();
        // Toggled out of order on purpose.
        s.toggle(OsStr::new("d"));
        s.toggle(OsStr::new("b"));
        let got: Vec<_> = s
            .resolve(&visible(&all))
            .iter()
            .map(|e| e.name.to_string_lossy().into_owned())
            .collect();
        assert_eq!(got, vec!["b", "d"], "listing order, not toggle order");
    }

    #[test]
    fn select_range_covers_the_inclusive_span() {
        let all = entries(&["a", "b", "c", "d", "e"]);
        let v = visible(&all);
        let mut s = Selection::new();
        s.set_anchor(OsStr::new("b"));
        s.select_range(&v, 3, RangeMode::Replace); // b..=d
        assert_eq!(s.len(), 3);
        for n in ["b", "c", "d"] {
            assert!(s.contains(OsStr::new(n)), "{n} should be selected");
        }
        assert!(!s.contains(OsStr::new("a")));
        assert!(!s.contains(OsStr::new("e")));
    }

    #[test]
    fn a_range_works_backwards_too() {
        let all = entries(&["a", "b", "c", "d"]);
        let v = visible(&all);
        let mut s = Selection::new();
        s.set_anchor(OsStr::new("d"));
        s.select_range(&v, 1, RangeMode::Replace); // d back to b
        assert_eq!(s.len(), 3);
        assert!(s.contains(OsStr::new("b")));
        assert!(s.contains(OsStr::new("d")));
        assert!(!s.contains(OsStr::new("a")));
    }

    #[test]
    fn extending_a_range_keeps_the_same_anchor() {
        // Shift+Down twice must grow from the original anchor, not creep.
        let all = entries(&["a", "b", "c", "d", "e"]);
        let v = visible(&all);
        let mut s = Selection::new();
        s.set_anchor(OsStr::new("a"));
        s.select_range(&v, 1, RangeMode::Replace);
        assert_eq!(s.len(), 2, "a..b");
        s.select_range(&v, 3, RangeMode::Replace);
        assert_eq!(s.len(), 4, "still from a: a..d");
        assert!(s.contains(OsStr::new("a")));
        assert!(s.contains(OsStr::new("d")));
    }

    #[test]
    fn shrinking_a_range_deselects_what_left_it() {
        let all = entries(&["a", "b", "c", "d"]);
        let v = visible(&all);
        let mut s = Selection::new();
        s.set_anchor(OsStr::new("a"));
        s.select_range(&v, 3, RangeMode::Replace); // a..d
        assert_eq!(s.len(), 4);
        s.select_range(&v, 1, RangeMode::Replace); // back to a..b
        assert_eq!(s.len(), 2, "d and c must drop out");
        assert!(!s.contains(OsStr::new("d")));
    }

    #[test]
    fn a_replacing_range_preserves_explicit_toggles() {
        let all = entries(&["a", "b", "c", "d", "e"]);
        let v = visible(&all);
        let mut s = Selection::new();
        s.toggle(OsStr::new("e")); // explicit Ctrl-pick
        s.set_anchor(OsStr::new("a"));
        s.select_range(&v, 1, RangeMode::Replace);
        assert!(
            s.contains(OsStr::new("e")),
            "a Ctrl-picked entry must survive a later Shift-range"
        );
        assert_eq!(s.len(), 3, "a, b, e");
    }

    #[test]
    fn select_all_takes_only_what_is_visible() {
        let all = entries(&["visible1", "visible2", "hidden-by-filter"]);
        // The caller passes only the two the user can see.
        let shown: Vec<&Entry> = all.iter().take(2).collect();
        let mut s = Selection::new();
        s.select_all_visible(&shown);
        assert_eq!(s.len(), 2);
        assert!(
            !s.contains(OsStr::new("hidden-by-filter")),
            "Ctrl+A must never select what the filter hides"
        );
    }

    #[test]
    fn entries_that_vanish_are_dropped_from_the_selection() {
        let before = entries(&["a", "b", "c"]);
        let mut s = Selection::new();
        s.select_all_visible(&visible(&before));
        assert_eq!(s.len(), 3);

        // "b" was deleted by another program.
        let after = entries(&["a", "c"]);
        s.retain_existing(&visible(&after));
        assert_eq!(s.len(), 2);
        assert!(
            !s.contains(OsStr::new("b")),
            "a vanished file must not stay selected and become a silent target"
        );
    }

    #[test]
    fn a_vanished_anchor_is_forgotten() {
        let mut s = Selection::new();
        s.set_anchor(OsStr::new("b"));
        let after = entries(&["a", "c"]);
        s.retain_existing(&visible(&after));
        // With no anchor, a range falls back to the cursor row.
        let v = visible(&after);
        assert_eq!(s.anchor_row(&v, 1), 1, "falls back, does not panic");
    }

    #[test]
    fn clearing_resets_the_anchor_too() {
        let all = entries(&["a", "b"]);
        let mut s = Selection::new();
        s.toggle(OsStr::new("a"));
        s.clear();
        assert!(s.is_empty());
        assert_eq!(s.anchor_row(&visible(&all), 7), 7, "anchor is gone");
    }

    #[test]
    fn select_only_discards_everything_else() {
        let mut s = Selection::new();
        s.toggle(OsStr::new("a"));
        s.toggle(OsStr::new("b"));
        s.select_only(OsStr::new("c"));
        assert_eq!(s.len(), 1);
        assert!(s.contains(OsStr::new("c")));
    }

    #[test]
    fn a_range_on_an_empty_listing_does_nothing() {
        let mut s = Selection::new();
        s.select_range(&[], 0, RangeMode::Replace);
        assert!(s.is_empty(), "must not panic or invent a selection");
    }

    #[test]
    fn a_cursor_past_the_end_is_clamped() {
        let all = entries(&["a", "b"]);
        let v = visible(&all);
        let mut s = Selection::new();
        s.set_anchor(OsStr::new("a"));
        s.select_range(&v, 99, RangeMode::Replace);
        assert_eq!(s.len(), 2, "clamped to the last row, no panic");
    }

    #[test]
    fn summary_reads_naturally() {
        let all = entries(&["a", "b", "c"]);
        let mut s = Selection::new();
        assert_eq!(s.summary(0), "");
        s.toggle(OsStr::new("a"));
        assert_eq!(s.summary(0), "1 selected");
        s.select_all_visible(&visible(&all));
        assert_eq!(s.summary(0), "3 selected");
        assert_eq!(s.summary(2048), "3 selected · 2.0 KB");
    }

    #[test]
    fn non_utf8_names_can_be_selected() {
        use std::os::unix::ffi::OsStringExt;
        let bad = OsString::from_vec(vec![b'b', 0xff, 0xfe, b'd']);
        let all = vec![Entry::new(bad.clone(), EntryKind::File)];
        let mut s = Selection::new();
        s.toggle(&bad);
        assert!(s.contains(&bad));
        assert_eq!(s.resolve(&visible(&all)).len(), 1);
    }
}

#[cfg(test)]
mod by_name_tests {
    use super::*;
    use crate::entry::EntryKind;

    fn names(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    /// The name-based helpers must behave exactly like the borrow-based ones,
    /// because the bridge uses one and the tests above prove the other.
    #[test]
    fn by_name_range_matches_the_borrowing_version() {
        let list = ["a", "b", "c", "d", "e"];
        let entries: Vec<Entry> = list
            .iter()
            .map(|n| Entry::new(OsString::from(n), EntryKind::File))
            .collect();
        let borrowed: Vec<&Entry> = entries.iter().collect();

        for (anchor, cursor) in [(0usize, 3usize), (3, 0), (2, 2), (0, 4)] {
            let mut a = Selection::new();
            let mut b = Selection::new();
            a.set_anchor(OsStr::new(list[anchor]));
            b.set_anchor(OsStr::new(list[anchor]));

            a.select_range(&borrowed, cursor, RangeMode::Replace);
            b.select_range_by_name(&names(&list), cursor, RangeMode::Replace);

            assert_eq!(
                a.len(),
                b.len(),
                "anchor={anchor} cursor={cursor}: different sizes"
            );
            for n in list {
                assert_eq!(
                    a.contains(OsStr::new(n)),
                    b.contains(OsStr::new(n)),
                    "anchor={anchor} cursor={cursor}: {n} differs"
                );
            }
        }
    }

    #[test]
    fn by_name_select_all_matches_the_borrowing_version() {
        let list = ["x", "y", "z"];
        let entries: Vec<Entry> = list
            .iter()
            .map(|n| Entry::new(OsString::from(n), EntryKind::File))
            .collect();
        let borrowed: Vec<&Entry> = entries.iter().collect();

        let mut a = Selection::new();
        let mut b = Selection::new();
        a.select_all_visible(&borrowed);
        b.select_all_by_name(&names(&list));
        assert_eq!(a.len(), b.len());
        for n in list {
            assert!(b.contains(OsStr::new(n)));
        }
    }

    #[test]
    fn by_name_range_on_an_empty_list_is_safe() {
        let mut s = Selection::new();
        s.select_range_by_name(&[], 0, RangeMode::Replace);
        assert!(s.is_empty());
    }

    #[test]
    fn by_name_range_preserves_pinned_entries() {
        let list = names(&["a", "b", "c", "d"]);
        let mut s = Selection::new();
        s.toggle(OsStr::new("d"));
        s.set_anchor(OsStr::new("a"));
        s.select_range_by_name(&list, 1, RangeMode::Replace);
        assert!(s.contains(OsStr::new("d")), "Ctrl-pick must survive");
        assert_eq!(s.len(), 3);
    }
}

impl Selection {
    /// `retain_existing` for a caller that already has the names.
    /// True when there is nothing a reload could prune: no selected name,
    /// no pinned name, no anchor. Lets callers skip building a set of 100k
    /// names on every listing of an untouched folder.
    pub fn is_untouched(&self) -> bool {
        self.names.is_empty() && self.pinned.is_empty() && self.anchor.is_none()
    }

    pub fn retain_names(&mut self, names: &[OsString]) {
        let present: HashSet<&OsStr> = names.iter().map(|n| n.as_os_str()).collect();
        self.names.retain(|n| present.contains(n.as_os_str()));
        self.pinned.retain(|n| present.contains(n.as_os_str()));
        if let Some(a) = &self.anchor {
            if !present.contains(a.as_os_str()) {
                self.anchor = None;
            }
        }
    }
}

#[cfg(test)]
mod invert_tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn invert_swaps_what_is_visible() {
        let mut s = Selection::new();
        s.toggle(OsStr::new("a"));
        s.toggle(OsStr::new("c"));
        s.invert_by_name(&names(&["a", "b", "c", "d"]));
        assert!(!s.contains(OsStr::new("a")));
        assert!(s.contains(OsStr::new("b")));
        assert!(!s.contains(OsStr::new("c")));
        assert!(s.contains(OsStr::new("d")));
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn invert_never_selects_or_keeps_what_the_filter_hides() {
        let mut s = Selection::new();
        s.toggle(OsStr::new("hidden-by-filter"));
        s.invert_by_name(&names(&["x", "y"]));
        assert!(
            !s.contains(OsStr::new("hidden-by-filter")),
            "an invisible target must not survive"
        );
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn invert_twice_is_the_original_visible_selection() {
        let mut s = Selection::new();
        s.toggle(OsStr::new("b"));
        let vis = names(&["a", "b", "c"]);
        s.invert_by_name(&vis);
        s.invert_by_name(&vis);
        assert_eq!(s.len(), 1);
        assert!(s.contains(OsStr::new("b")));
    }
}
