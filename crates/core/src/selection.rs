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

    /// Rubber-band select: the selection becomes `base` (what was selected
    /// when the band started: nothing for a plain drag, the old selection
    /// for Ctrl+drag) plus `picked` (what the band covers now). Called on
    /// every pointer move, so shrinking the band unselects again.
    pub fn set_band(&mut self, base: &[OsString], picked: &[OsString]) {
        self.names = base.iter().chain(picked.iter()).cloned().collect();
        self.pinned.clear();
    }

    /// The selected names, for a band's starting snapshot.
    pub fn names_vec(&self) -> Vec<OsString> {
        self.names.iter().cloned().collect()
    }

    /// Shift+arrow / Shift+click: extend from the anchor to `target`. With
    /// no anchor yet (after a plain click, which clears the selection), the
    /// range starts at the row the cursor is on NOW, `from` — the row the
    /// user clicked — never at the row being moved to. Click a file,
    /// Shift+Down: both rows are selected, as everywhere else.
    pub fn extend_by_name(
        &mut self,
        names: &[OsString],
        from: usize,
        target: usize,
        mode: RangeMode,
    ) {
        let anchored = self
            .anchor
            .as_ref()
            .is_some_and(|a| names.iter().any(|n| n == a));
        if !anchored {
            if let Some(n) = names.get(from) {
                self.anchor = Some(n.clone());
            }
        }
        self.select_range_by_name(names, target, mode);
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
    fn shift_after_a_plain_click_extends_from_the_clicked_row() {
        // Found in the real window: click the first file, Shift+Down, and
        // only the SECOND file was selected — the range anchored at the row
        // it moved to. F2 then renamed one file instead of two.
        let list: Vec<OsString> = ["a", "b", "c", "d"].iter().map(OsString::from).collect();
        let mut s = Selection::new();
        s.clear(); // what a plain click does
        s.extend_by_name(&list, 0, 1, RangeMode::Replace);
        assert_eq!(s.len(), 2);
        assert!(s.contains(OsStr::new("a")) && s.contains(OsStr::new("b")));
        // Further Shift+Down keeps growing from the same anchor.
        s.extend_by_name(&list, 1, 2, RangeMode::Replace);
        assert_eq!(s.len(), 3);
        // An existing anchor wins over the cursor.
        let mut t = Selection::new();
        t.set_anchor(OsStr::new("d"));
        t.extend_by_name(&list, 0, 2, RangeMode::Replace);
        assert!(t.contains(OsStr::new("c")) && t.contains(OsStr::new("d")) && t.len() == 2);
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

fn collect_names_into<'a>(
    names: impl Iterator<Item = &'a OsString>,
    prepared: &mut HashSet<OsString>,
    cancel: &impl crate::projection::Cancellation,
) -> Result<(), crate::projection::ProjectionError> {
    use crate::projection::{check_row, checkpoint};
    checkpoint(cancel)?;
    for (row, name) in names.enumerate() {
        check_row(row, cancel)?;
        prepared.insert(name.clone());
    }
    checkpoint(cancel)
}

/// Caller-owned selection preparation and retirement storage.
///
/// One scratch owns at most two name sets and one anchor. Supply a vacant
/// scratch only after admitting a bounded retirement slot. On failure the
/// partially prepared sets remain here; on a successful gesture replaced
/// sets move here. Move this owner to the cleanup recipient before starting
/// replacement CPU work. Core never clears or queues occupied scratch.
#[derive(Debug, Default)]
pub struct SelectionScratch {
    storage: Selection,
}

impl SelectionScratch {
    /// Empty retained allocations still require retirement.
    pub fn is_vacant(&self) -> bool {
        self.storage.names.capacity() == 0
            && self.storage.pinned.capacity() == 0
            && self.storage.anchor.is_none()
    }

    pub fn tracked_names(&self) -> usize {
        self.storage
            .names
            .len()
            .saturating_add(self.storage.pinned.len())
            .saturating_add(usize::from(self.storage.anchor.is_some()))
    }

    fn require_vacant(&self) -> Result<(), crate::projection::ProjectionError> {
        if self.is_vacant() {
            Ok(())
        } else {
            Err(crate::projection::ProjectionError::ScratchOccupied)
        }
    }
}

fn find_name_cancellable(
    names: &[OsString],
    target: Option<&OsStr>,
    cancel: &impl crate::projection::Cancellation,
) -> Result<Option<usize>, crate::projection::ProjectionError> {
    use crate::projection::{check_row, checkpoint};
    checkpoint(cancel)?;
    if let Some(target) = target {
        for (row, name) in names.iter().enumerate() {
            check_row(row, cancel)?;
            if name == target {
                return Ok(Some(row));
            }
        }
    }
    checkpoint(cancel)?;
    Ok(None)
}

/// Worker-prepared selection totals. Presentation belongs to the app.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SelectionSummary {
    pub selected_count: usize,
    pub known_bytes: u64,
    pub missing_metadata: usize,
}

impl Selection {
    /// Compatibility wrapper. Drops cancelled/replaced storage on the caller.
    /// CPU projection workers must use the companion with admitted scratch.
    pub fn select_all_by_name_cancellable(
        &mut self,
        names: &[OsString],
        cancel: &impl crate::projection::Cancellation,
    ) -> Result<(), crate::projection::ProjectionError> {
        self.select_all_by_name_cancellable_with_scratch(
            names,
            &mut SelectionScratch::default(),
            cancel,
        )
    }

    /// Prepare atomically; replaced names and pins move into scratch.
    pub fn select_all_by_name_cancellable_with_scratch(
        &mut self,
        names: &[OsString],
        scratch: &mut SelectionScratch,
        cancel: &impl crate::projection::Cancellation,
    ) -> Result<(), crate::projection::ProjectionError> {
        scratch.require_vacant()?;
        collect_names_into(names.iter(), &mut scratch.storage.names, cancel)?;
        std::mem::swap(&mut self.names, &mut scratch.storage.names);
        std::mem::swap(&mut self.pinned, &mut scratch.storage.pinned);
        Ok(())
    }

    /// Compatibility wrapper; use the scratch companion on CPU workers.
    pub fn invert_by_name_cancellable(
        &mut self,
        visible: &[OsString],
        cancel: &impl crate::projection::Cancellation,
    ) -> Result<(), crate::projection::ProjectionError> {
        self.invert_by_name_cancellable_with_scratch(
            visible,
            &mut SelectionScratch::default(),
            cancel,
        )
    }

    /// Invisible selections are dropped and anchor semantics are preserved.
    /// Failure leaves self unchanged and partial names owned by scratch.
    pub fn invert_by_name_cancellable_with_scratch(
        &mut self,
        visible: &[OsString],
        scratch: &mut SelectionScratch,
        cancel: &impl crate::projection::Cancellation,
    ) -> Result<(), crate::projection::ProjectionError> {
        use crate::projection::{check_row, checkpoint};
        scratch.require_vacant()?;
        checkpoint(cancel)?;
        for (row, name) in visible.iter().enumerate() {
            check_row(row, cancel)?;
            if !self.names.contains(name.as_os_str()) {
                scratch.storage.names.insert(name.clone());
            }
        }
        checkpoint(cancel)?;
        std::mem::swap(&mut self.names, &mut scratch.storage.names);
        std::mem::swap(&mut self.pinned, &mut scratch.storage.pinned);
        Ok(())
    }

    /// Compatibility wrapper; use the scratch companion on CPU workers.
    pub fn set_band_cancellable(
        &mut self,
        base: &[OsString],
        picked: &[OsString],
        cancel: &impl crate::projection::Cancellation,
    ) -> Result<(), crate::projection::ProjectionError> {
        self.set_band_cancellable_with_scratch(
            base,
            picked,
            &mut SelectionScratch::default(),
            cancel,
        )
    }

    /// Both input walks checkpoint. Replaced names and pins move to scratch.
    pub fn set_band_cancellable_with_scratch(
        &mut self,
        base: &[OsString],
        picked: &[OsString],
        scratch: &mut SelectionScratch,
        cancel: &impl crate::projection::Cancellation,
    ) -> Result<(), crate::projection::ProjectionError> {
        scratch.require_vacant()?;
        collect_names_into(
            base.iter().chain(picked.iter()),
            &mut scratch.storage.names,
            cancel,
        )?;
        std::mem::swap(&mut self.names, &mut scratch.storage.names);
        std::mem::swap(&mut self.pinned, &mut scratch.storage.pinned);
        Ok(())
    }

    /// Compatibility wrapper; use the scratch companion on CPU workers.
    pub fn select_range_by_name_cancellable(
        &mut self,
        names: &[OsString],
        cursor: usize,
        mode: RangeMode,
        cancel: &impl crate::projection::Cancellation,
    ) -> Result<(), crate::projection::ProjectionError> {
        self.select_range_by_name_cancellable_with_scratch(
            names,
            cursor,
            mode,
            &mut SelectionScratch::default(),
            cancel,
        )
    }

    /// Pins and anchor keep their original meanings, including pins outside
    /// this visible list. Only the replaced selected set moves to scratch.
    pub fn select_range_by_name_cancellable_with_scratch(
        &mut self,
        names: &[OsString],
        cursor: usize,
        mode: RangeMode,
        scratch: &mut SelectionScratch,
        cancel: &impl crate::projection::Cancellation,
    ) -> Result<(), crate::projection::ProjectionError> {
        scratch.require_vacant()?;
        crate::projection::checkpoint(cancel)?;
        if names.is_empty() {
            return Ok(());
        }
        let cursor = cursor.min(names.len() - 1);
        let anchor =
            find_name_cancellable(names, self.anchor.as_deref(), cancel)?.unwrap_or(cursor);
        self.prepare_range_into(
            names,
            anchor,
            cursor,
            mode,
            &mut scratch.storage.names,
            cancel,
        )?;
        std::mem::swap(&mut self.names, &mut scratch.storage.names);
        Ok(())
    }

    /// Compatibility wrapper; use the scratch companion on CPU workers.
    pub fn extend_by_name_cancellable(
        &mut self,
        names: &[OsString],
        from: usize,
        target: usize,
        mode: RangeMode,
        cancel: &impl crate::projection::Cancellation,
    ) -> Result<(), crate::projection::ProjectionError> {
        self.extend_by_name_cancellable_with_scratch(
            names,
            from,
            target,
            mode,
            &mut SelectionScratch::default(),
            cancel,
        )
    }

    /// Anchor searches, base copies and range additions checkpoint. Failure
    /// preserves self; success moves replaced names and anchor into scratch.
    #[allow(clippy::too_many_arguments)] // Existing gesture plus explicit scratch and cancellation.
    pub fn extend_by_name_cancellable_with_scratch(
        &mut self,
        names: &[OsString],
        from: usize,
        target: usize,
        mode: RangeMode,
        scratch: &mut SelectionScratch,
        cancel: &impl crate::projection::Cancellation,
    ) -> Result<(), crate::projection::ProjectionError> {
        use crate::projection::checkpoint;
        scratch.require_vacant()?;
        checkpoint(cancel)?;
        if names.is_empty() {
            return Ok(());
        }
        let target = target.min(names.len() - 1);
        scratch.storage.anchor = self.anchor.clone();
        let anchor = match find_name_cancellable(names, self.anchor.as_deref(), cancel)? {
            Some(row) => row,
            None => match names.get(from) {
                Some(name) => {
                    scratch.storage.anchor = Some(name.clone());
                    find_name_cancellable(names, Some(name), cancel)?.unwrap_or(target)
                }
                None => target,
            },
        };
        self.prepare_range_into(
            names,
            anchor,
            target,
            mode,
            &mut scratch.storage.names,
            cancel,
        )?;
        checkpoint(cancel)?;
        std::mem::swap(&mut self.names, &mut scratch.storage.names);
        std::mem::swap(&mut self.anchor, &mut scratch.storage.anchor);
        Ok(())
    }

    fn prepare_range_into(
        &self,
        names: &[OsString],
        anchor: usize,
        cursor: usize,
        mode: RangeMode,
        prepared: &mut HashSet<OsString>,
        cancel: &impl crate::projection::Cancellation,
    ) -> Result<(), crate::projection::ProjectionError> {
        use crate::projection::{check_row, checkpoint};
        let base = match mode {
            RangeMode::Replace => &self.pinned,
            RangeMode::Add => &self.names,
        };
        collect_names_into(base.iter(), prepared, cancel)?;
        let (lo, hi) = (anchor.min(cursor), anchor.max(cursor));
        for (row, name) in names[lo..=hi].iter().enumerate() {
            check_row(row, cancel)?;
            prepared.insert(name.clone());
        }
        checkpoint(cancel)
    }

    /// Compatibility wrapper; use the scratch companion on CPU workers.
    pub fn clone_retain_visible(
        &self,
        entries: &[Entry],
        visible: &[usize],
        cancel: &impl crate::projection::Cancellation,
    ) -> Result<Self, crate::projection::ProjectionError> {
        self.clone_retain_visible_with_scratch(
            entries,
            visible,
            &mut SelectionScratch::default(),
            cancel,
        )
    }

    /// Copy only surviving selected/pinned names and anchor. On error no
    /// partial selection is destroyed: caller retains it in scratch. On
    /// success the prepared selection is returned and scratch becomes vacant.
    /// Invalid indices are errors, including for untouched selections.
    pub fn clone_retain_visible_with_scratch(
        &self,
        entries: &[Entry],
        visible: &[usize],
        scratch: &mut SelectionScratch,
        cancel: &impl crate::projection::Cancellation,
    ) -> Result<Self, crate::projection::ProjectionError> {
        use crate::projection::{check_row, checkpoint, entry_at};
        scratch.require_vacant()?;
        checkpoint(cancel)?;
        for (row, &index) in visible.iter().enumerate() {
            check_row(row, cancel)?;
            let name = entry_at(entries, index)?.name.as_os_str();
            if self.names.contains(name) {
                scratch.storage.names.insert(name.to_os_string());
            }
            if self.pinned.contains(name) {
                scratch.storage.pinned.insert(name.to_os_string());
            }
            if self.anchor.as_deref() == Some(name) {
                scratch.storage.anchor = Some(name.to_os_string());
            }
        }
        checkpoint(cancel)?;
        Ok(std::mem::take(&mut scratch.storage))
    }

    /// Count selected displayed rows and sum known metadata sizes without
    /// allocating names. Visible indices normally form a unique subset;
    /// duplicate occurrences contribute once per displayed row. Unknown
    /// metadata contributes to missing_metadata, never to known_bytes.
    pub fn visible_summary(
        &self,
        entries: &[Entry],
        visible: &[usize],
        cancel: &impl crate::projection::Cancellation,
    ) -> Result<SelectionSummary, crate::projection::ProjectionError> {
        use crate::projection::{check_row, checkpoint, entry_at};
        checkpoint(cancel)?;
        let mut summary = SelectionSummary::default();
        for (row, &index) in visible.iter().enumerate() {
            check_row(row, cancel)?;
            let entry = entry_at(entries, index)?;
            if self.contains(&entry.name) {
                summary.selected_count += 1;
                match entry.meta {
                    Some(meta) => {
                        summary.known_bytes = summary.known_bytes.saturating_add(meta.size)
                    }
                    None => summary.missing_metadata += 1,
                }
            }
        }
        checkpoint(cancel)?;
        Ok(summary)
    }
}

impl Selection {
    /// True when there is nothing a reload could prune: no selected name,
    /// no pinned name, no anchor. Lets callers skip building a set of 100k
    /// names on every listing of an untouched folder.
    pub fn is_untouched(&self) -> bool {
        self.names.is_empty() && self.pinned.is_empty() && self.anchor.is_none()
    }

    /// Drop selected, pinned and anchor names absent from the visible rows.
    ///
    /// Borrows raw names instead of cloning the listing. Expected work is
    /// linear in visible rows plus selection capacity; scratch membership is
    /// bounded by the smaller of visible rows and tracked names. Indices are
    /// membership within this snapshot, never authority for a file operation.
    /// Out-of-range indices contribute no name.
    pub fn retain_visible(&mut self, entries: &[Entry], visible: &[usize]) {
        if self.is_untouched() {
            return;
        }
        let tracked = self
            .names
            .len()
            .saturating_add(self.pinned.len())
            .saturating_add(usize::from(self.anchor.is_some()));
        let mut present = HashSet::with_capacity(visible.len().min(tracked));
        // When the visible side is smaller, store it directly. Looking up
        // every name in the selection first only adds work for Ctrl+A.
        let keep_all_visible = visible.len() <= tracked;
        for &index in visible {
            let Some(entry) = entries.get(index) else {
                continue;
            };
            let name = entry.name.as_os_str();
            if keep_all_visible
                || self.names.contains(name)
                || self.pinned.contains(name)
                || self.anchor.as_deref() == Some(name)
            {
                present.insert(name);
            }
        }
        self.names.retain(|n| present.contains(n.as_os_str()));
        self.pinned.retain(|n| present.contains(n.as_os_str()));
        if self
            .anchor
            .as_ref()
            .is_some_and(|a| !present.contains(a.as_os_str()))
        {
            self.anchor = None;
        }
    }

    /// `retain_existing` for a caller that already has the names.
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
mod retain_visible_tests {
    use super::*;
    use crate::entry::{EntryId, EntryKind};
    use std::os::unix::ffi::OsStringExt;

    fn entries() -> Vec<Entry> {
        ["picked", "range", "anchor", "hidden", "replacement"]
            .into_iter()
            .map(|n| Entry::new(n.into(), EntryKind::File))
            .collect()
    }

    #[test]
    fn pruning_matches_names_for_selected_pinned_and_anchor_state() {
        let entries = entries();
        // Exhaust every visible subset and independently vary the anchor:
        // empty selection does not imply an untouched anchor or pinned set.
        for mask in 0..(1 << entries.len()) {
            for anchor in [None, Some("anchor"), Some("deleted")] {
                for selected in [false, true] {
                    let mut actual = Selection::new();
                    actual.toggle(OsStr::new("picked"));
                    actual.toggle(OsStr::new("hidden"));
                    actual.toggle(OsStr::new("deleted"));
                    actual.names.insert("range".into());
                    if !selected {
                        // Pinned-only state can survive replacement helpers.
                        actual.names.clear();
                    }
                    actual.anchor = anchor.map(OsString::from);
                    let mut expected = actual.clone();
                    let visible: Vec<_> = (0..entries.len())
                        .filter(|i| mask & (1 << i) != 0)
                        .collect();
                    let names: Vec<_> = visible.iter().map(|&i| entries[i].name.clone()).collect();
                    expected.retain_names(&names);
                    actual.retain_visible(&entries, &visible);
                    assert_eq!(actual.names, expected.names);
                    assert_eq!(actual.pinned, expected.pinned);
                    assert_eq!(actual.anchor, expected.anchor);
                }
            }
        }
    }

    #[test]
    fn hidden_or_deleted_picks_cannot_reappear_in_a_later_range() {
        let entries = entries();
        let mut s = Selection::new();
        s.toggle(OsStr::new("picked"));
        s.toggle(OsStr::new("hidden"));
        s.toggle(OsStr::new("deleted"));
        s.set_anchor(OsStr::new("hidden"));
        s.retain_visible(&entries, &[0, 1, 2]);
        assert_eq!(s.names, HashSet::from([OsString::from("picked")]));
        assert_eq!(s.pinned, s.names);
        assert!(s.anchor.is_none());
        let visible: Vec<_> = entries.iter().take(3).collect();
        s.select_range(&visible, 1, RangeMode::Replace);
        assert_eq!(
            s.resolve(&visible)
                .iter()
                .map(|e| e.name.as_os_str())
                .collect::<Vec<_>>(),
            [OsStr::new("picked"), OsStr::new("range")]
        );
    }

    #[test]
    fn anchor_only_and_untouched_states_prune_safely() {
        let entries = entries();
        let mut s = Selection::new();
        s.retain_visible(&entries, &[usize::MAX]);
        assert!(s.is_untouched());
        s.set_anchor(OsStr::new("anchor"));
        s.retain_visible(&entries, &[2, 2, usize::MAX]);
        assert_eq!(s.anchor.as_deref(), Some(OsStr::new("anchor")));
        s.retain_visible(&entries, &[]);
        assert!(s.is_untouched());
    }

    #[test]
    fn raw_names_survive_but_an_index_or_colliding_id_never_selects_a_replacement() {
        let raw = OsString::from_vec(b"raw-\xff\n".to_vec());
        let mut entries = entries();
        entries.push(Entry::new(raw.clone(), EntryKind::File));
        let mut s = Selection::new();
        s.toggle(&raw);
        s.toggle(OsStr::new("picked"));
        // Selection follows names after a reorder, not the old row or ID.
        entries.swap(0, 4);
        entries[0].id = entries[4].id;
        s.retain_visible(&entries, &[0, 5]);
        assert_eq!(s.names, HashSet::from([raw.clone()]));
        assert_eq!(s.pinned, s.names);
        assert_eq!(s.anchor, None);
        assert!(!s.contains(&entries[0].name));
        assert_eq!(entries[0].id, entries[4].id);
        assert!(entries.iter().all(|e| e.meta.is_none()));
        // These APIs supply name membership only. No metadata/identity proof
        // is created for operation admission, even for the same name.
        entries[5].id = EntryId(0);
        s.retain_visible(&entries, &[5]);
        assert!(s.contains(&raw));
        assert!(entries[5].meta.is_none());
    }

    #[test]
    fn few_and_all_selected_scale_to_large_listings_without_changing_membership() {
        let entries: Vec<_> = (0..100_000)
            .map(|i| Entry::new(format!("file-{i:06}").into(), EntryKind::File))
            .collect();
        let visible: Vec<_> = (0..entries.len()).step_by(2).collect();
        for all in [false, true] {
            let mut s = Selection::new();
            if all {
                s.select_all_visible(&entries.iter().collect::<Vec<_>>());
            } else {
                s.toggle(&entries[0].name);
                s.toggle(&entries[1].name);
                s.set_anchor(&entries[1].name);
            }
            s.retain_visible(&entries, &visible);
            assert_eq!(s.len(), if all { visible.len() } else { 1 });
            assert!(s.anchor.is_none());
            for (i, e) in entries.iter().enumerate() {
                assert_eq!(s.contains(&e.name), i % 2 == 0 && (all || i == 0));
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

#[cfg(test)]
mod band_tests {
    use super::*;

    fn n(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn a_band_replaces_the_selection_and_shrinking_it_unselects() {
        let mut s = Selection::new();
        s.select_only(OsStr::new("old"));
        s.set_band(&[], &n(&["a", "b", "c"]));
        assert_eq!(s.len(), 3);
        assert!(!s.contains(OsStr::new("old")));
        s.set_band(&[], &n(&["a"]));
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn ctrl_band_adds_to_what_was_selected() {
        let mut s = Selection::new();
        s.select_only(OsStr::new("keep"));
        let base = s.names_vec();
        s.set_band(&base, &n(&["a", "b"]));
        assert_eq!(s.len(), 3);
        s.set_band(&base, &[]);
        assert!(s.contains(OsStr::new("keep")) && s.len() == 1);
    }
}

#[cfg(test)]
mod worker_selection_tests {
    use super::*;
    use crate::entry::EntryKind;
    use std::os::unix::ffi::OsStringExt;

    fn assert_same(a: &Selection, b: &Selection) {
        assert_eq!(a.names, b.names);
        assert_eq!(a.pinned, b.pinned);
        assert_eq!(a.anchor, b.anchor);
        assert_eq!(a.is_untouched(), b.is_untouched());
    }

    #[test]
    fn worker_copy_matches_clone_then_prune_across_selection_gestures_and_deletions() {
        let names: Vec<_> = [
            b"a".as_slice(),
            b"B",
            b"c9",
            b"c10",
            b"\xff",
            b"\xfe",
            b".hidden",
        ]
        .iter()
        .map(|n| OsString::from_vec(n.to_vec()))
        .collect();
        let entries: Vec<_> = names
            .iter()
            .map(|n| Entry::new(n.clone(), EntryKind::File))
            .collect();
        let mut selection = Selection::new();
        let mut seed = 345678_u64;
        for step in 0..250 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let target = seed as usize % names.len();
            match step % 7 {
                0 => selection.toggle(&names[target]),
                1 => selection.set_anchor(&names[target]),
                2 => selection.select_range_by_name(&names, target, RangeMode::Replace),
                3 => selection.select_range_by_name(&names, target, RangeMode::Add),
                4 => selection.select_only(&names[target]),
                5 => selection.clear(),
                _ => selection.select_all_by_name(&names),
            }
            for mask in 0..1 << names.len() {
                let visible: Vec<_> = (0..names.len())
                    .rev()
                    .filter(|i| mask & (1 << i) != 0)
                    .collect();
                let mut expected = selection.clone();
                expected.retain_visible(&entries, &visible);
                let actual = selection
                    .clone_retain_visible(&entries, &visible, &|| false)
                    .unwrap();
                assert_same(&actual, &expected);
                // Subsequent range replacement must preserve exactly the
                // same Ctrl-picks and surviving anchor after target deletion.
                let mut actual = actual;
                actual.select_range_by_name(&names, target, RangeMode::Replace);
                expected.select_range_by_name(&names, target, RangeMode::Replace);
                assert_same(&actual, &expected);
            }
        }
    }

    #[test]
    fn worker_prunes_anchor_only_and_preserves_duplicate_membership_semantics() {
        let entries = vec![Entry::new("raw".into(), EntryKind::File)];
        let mut selection = Selection::new();
        selection.set_anchor(OsStr::new("raw"));
        let surviving = selection
            .clone_retain_visible(&entries, &[0, 0], &|| false)
            .unwrap();
        assert_same(&selection, &surviving);
        assert!(selection
            .clone_retain_visible(&entries, &[], &|| false)
            .unwrap()
            .is_untouched());
    }
}

#[cfg(test)]
mod worker_gesture_tests {
    use super::*;
    use crate::projection::ProjectionError;
    use std::cell::Cell;
    use std::os::unix::ffi::OsStringExt;

    fn same(a: &Selection, b: &Selection) {
        assert_eq!(a.names, b.names);
        assert_eq!(a.pinned, b.pinned);
        assert_eq!(a.anchor, b.anchor);
    }

    #[test]
    fn worker_gestures_match_existing_semantics_including_missing_anchors_and_pins() {
        let names: Vec<_> = [b"a".as_slice(), b"B", b"\xff", b"c9", b"c10", b"\xfe", b"a"]
            .iter()
            .map(|n| OsString::from_vec(n.to_vec()))
            .collect();
        let mut expected = Selection::new();
        let mut actual = Selection::new();
        let mut seed = 891_734_u64;
        for step in 0usize..700 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let cut = seed as usize % (names.len() + 1);
            let visible = &names[cut..];
            let from = (seed >> 5) as usize % (names.len() + 3);
            let target = if step.is_multiple_of(17) {
                usize::MAX
            } else {
                (seed >> 9) as usize % (names.len() + 3)
            };
            let mode = if step.is_multiple_of(2) {
                RangeMode::Add
            } else {
                RangeMode::Replace
            };
            match step % 9 {
                0 => {
                    expected.select_all_by_name(visible);
                    actual
                        .select_all_by_name_cancellable(visible, &|| false)
                        .unwrap();
                }
                1 => {
                    expected.invert_by_name(visible);
                    actual
                        .invert_by_name_cancellable(visible, &|| false)
                        .unwrap();
                }
                2 => {
                    expected.set_band(&names[..cut], visible);
                    actual
                        .set_band_cancellable(&names[..cut], visible, &|| false)
                        .unwrap();
                }
                3 => {
                    expected.extend_by_name(visible, from, target, mode);
                    actual
                        .extend_by_name_cancellable(visible, from, target, mode, &|| false)
                        .unwrap();
                }
                4 => {
                    expected.select_range_by_name(visible, target, mode);
                    actual
                        .select_range_by_name_cancellable(visible, target, mode, &|| false)
                        .unwrap();
                }
                5 => {
                    let name = &names[from % names.len()];
                    expected.toggle(name);
                    actual.toggle(name);
                }
                6 => {
                    let name = &names[from % names.len()];
                    expected.set_anchor(name);
                    actual.set_anchor(name);
                }
                7 => {
                    expected.set_anchor(OsStr::new("deleted"));
                    actual.set_anchor(OsStr::new("deleted"));
                }
                _ => {
                    expected.clear();
                    actual.clear();
                }
            }
            same(&actual, &expected);
        }
    }

    fn cancel_without_mutating(
        seed: &Selection,
        budget: usize,
        run: impl Fn(&mut Selection, &dyn Fn() -> bool) -> Result<(), ProjectionError>,
    ) {
        let mut actual = seed.clone();
        let calls = Cell::new(0);
        let cancel = || {
            calls.set(calls.get() + 1);
            calls.get() == budget
        };
        assert_eq!(
            run(&mut actual, &cancel),
            Err(ProjectionError::Cancelled),
            "checkpoint {budget}"
        );
        assert_eq!(calls.get(), budget);
        same(&actual, seed);
    }

    #[test]
    fn worker_gesture_pre_cancel_and_mid_work_cancellation_are_atomic() {
        let names: Vec<_> = (0..16_385)
            .map(|i| OsString::from(format!("file{i}")))
            .collect();
        let mut seed = Selection::new();
        // A large selected set and pinned set make copying either side a
        // directory-sized operation, even for a tiny resulting range.
        for name in &names {
            seed.toggle(name);
        }
        for budget in [1, 5, 35] {
            cancel_without_mutating(&seed, budget, |s, c| {
                s.select_all_by_name_cancellable(&names, &c)
            });
            cancel_without_mutating(&seed, budget, |s, c| {
                s.invert_by_name_cancellable(&names, &c)
            });
            cancel_without_mutating(&seed, budget, |s, c| {
                s.set_band_cancellable(&names[..100], &names[100..], &c)
            });
            cancel_without_mutating(&seed, budget, |s, c| {
                s.extend_by_name_cancellable(&names, 0, 0, RangeMode::Replace, &c)
            });
            cancel_without_mutating(&seed, budget, |s, c| {
                s.select_range_by_name_cancellable(&names, 0, RangeMode::Add, &c)
            });
        }
        // Later budgets reach selected/pinned copies after the anchor scan,
        // and then the range-addition phase after copying the base set.
        for budget in [80, 145] {
            cancel_without_mutating(&seed, budget, |s, c| {
                s.extend_by_name_cancellable(&names, 0, 0, RangeMode::Replace, &c)
            });
            cancel_without_mutating(&seed, budget, |s, c| {
                s.select_range_by_name_cancellable(&names, 0, RangeMode::Add, &c)
            });
        }
        for run in [0, 1, 2, 3, 4] {
            let mut empty = Selection::new();
            let result = match run {
                0 => empty.select_all_by_name_cancellable(&[], &|| true),
                1 => empty.invert_by_name_cancellable(&[], &|| true),
                2 => empty.set_band_cancellable(&[], &[], &|| true),
                3 => empty.extend_by_name_cancellable(&[], 0, 0, RangeMode::Add, &|| true),
                _ => empty.select_range_by_name_cancellable(&[], 0, RangeMode::Add, &|| true),
            };
            assert_eq!(result, Err(ProjectionError::Cancelled));
        }
    }
}

#[cfg(test)]
mod scratch_handoff_tests {
    use super::*;
    use crate::{
        entry::EntryKind,
        projection::{Cancellation, ProjectionError},
    };
    use std::cell::Cell;
    use std::sync::mpsc;

    fn same(a: &Selection, b: &Selection) {
        assert_eq!(a.names, b.names);
        assert_eq!(a.pinned, b.pinned);
        assert_eq!(a.anchor, b.anchor);
    }

    fn gesture(
        kind: usize,
        selection: &mut Selection,
        names: &[OsString],
        scratch: &mut SelectionScratch,
        cancel: &impl Cancellation,
    ) -> Result<(), ProjectionError> {
        match kind {
            0 => selection.select_all_by_name_cancellable_with_scratch(names, scratch, cancel),
            1 => selection.invert_by_name_cancellable_with_scratch(names, scratch, cancel),
            2 => selection.set_band_cancellable_with_scratch(
                &names[..100],
                &names[100..],
                scratch,
                cancel,
            ),
            3 => selection.select_range_by_name_cancellable_with_scratch(
                names,
                0,
                RangeMode::Replace,
                scratch,
                cancel,
            ),
            _ => selection.extend_by_name_cancellable_with_scratch(
                names,
                0,
                0,
                RangeMode::Add,
                scratch,
                cancel,
            ),
        }
    }

    #[test]
    fn errors_transfer_partial_storage_and_cannot_reuse_occupied_scratch() {
        let entries: Vec<_> = (0..4097)
            .map(|i| Entry::new(format!("file{i}").into(), EntryKind::File))
            .collect();
        let visible: Vec<_> = (0..entries.len()).collect();
        let mut source = Selection::new();
        for entry in &entries {
            source.toggle(&entry.name);
        }
        let unchanged = source.clone();
        let mut scratch = SelectionScratch::default();
        let calls = Cell::new(0);
        let cancel = || {
            calls.set(calls.get() + 1);
            calls.get() == 5
        };
        assert_eq!(
            source
                .clone_retain_visible_with_scratch(&entries, &visible, &mut scratch, &cancel)
                .unwrap_err(),
            ProjectionError::Cancelled
        );
        same(&source, &unchanged);
        assert!(!scratch.is_vacant());
        assert!(scratch.tracked_names() > 0);
        assert!(scratch.tracked_names() <= source.names.len() + source.pinned.len() + 1);
        let partial_names = scratch.storage.names.clone();
        let partial_pins = scratch.storage.pinned.clone();
        let retained = scratch.tracked_names();
        assert_eq!(
            source
                .clone_retain_visible_with_scratch(&entries, &visible, &mut scratch, &|| false)
                .unwrap_err(),
            ProjectionError::ScratchOccupied
        );
        assert_eq!(scratch.storage.names, partial_names);
        assert_eq!(scratch.storage.pinned, partial_pins);
        assert_eq!(scratch.tracked_names(), retained);
        // Non-cancellation errors hand off their partial output too.
        let mut bad_scratch = SelectionScratch::default();
        assert_eq!(
            source
                .clone_retain_visible_with_scratch(
                    &entries,
                    &[0, 1, usize::MAX],
                    &mut bad_scratch,
                    &|| false
                )
                .unwrap_err(),
            ProjectionError::InvalidIndex {
                index: usize::MAX,
                len: entries.len()
            }
        );
        assert_eq!(bad_scratch.storage.names.len(), 2);
        same(&source, &unchanged);
    }

    #[test]
    fn every_gesture_keeps_cancelled_partials_and_moves_replaced_sets_on_success() {
        let names: Vec<_> = (0..16_385)
            .map(|i| OsString::from(format!("file{i}")))
            .collect();
        let mut source = Selection::new();
        for name in &names {
            source.toggle(name);
        }
        for kind in 0..5 {
            let mut original = source.clone();
            if kind == 1 {
                for name in names.iter().step_by(2) {
                    original.toggle(name);
                }
            }
            let mut actual = original.clone();
            let mut scratch = SelectionScratch::default();
            let calls = Cell::new(0);
            let budget = if kind >= 3 { 80 } else { 5 };
            let cancel = || {
                calls.set(calls.get() + 1);
                calls.get() == budget
            };
            assert_eq!(
                gesture(kind, &mut actual, &names, &mut scratch, &cancel),
                Err(ProjectionError::Cancelled)
            );
            same(&actual, &original);
            assert!(!scratch.is_vacant());
            assert!(scratch.tracked_names() <= names.len() * 2 + 1);
            let mut scratch = SelectionScratch::default();
            gesture(kind, &mut actual, &names, &mut scratch, &|| false).unwrap();
            let mut expected = original.clone();
            match kind {
                0 => expected.select_all_by_name(&names),
                1 => expected.invert_by_name(&names),
                2 => expected.set_band(&names[..100], &names[100..]),
                3 => expected.select_range_by_name(&names, 0, RangeMode::Replace),
                _ => expected.extend_by_name(&names, 0, 0, RangeMode::Add),
            }
            same(&actual, &expected);
            assert_eq!(scratch.storage.names, original.names);
            if kind < 3 {
                assert_eq!(scratch.storage.pinned, original.pinned);
            } else {
                assert!(scratch.storage.pinned.is_empty());
            }
        }
    }

    #[test]
    fn successful_clone_returns_storage_and_vacates_scratch() {
        let entries = vec![Entry::new("raw".into(), EntryKind::File)];
        let mut source = Selection::new();
        source.toggle(&entries[0].name);
        let mut scratch = SelectionScratch::default();
        let prepared = source
            .clone_retain_visible_with_scratch(&entries, &[0], &mut scratch, &|| false)
            .unwrap();
        same(&source, &prepared);
        assert!(scratch.is_vacant());
        assert_eq!(scratch.tracked_names(), 0);
        // A zero-length set with allocation is still occupied.
        scratch.storage.names.reserve(1000);
        assert!(!scratch.is_vacant());
        assert_eq!(scratch.tracked_names(), 0);
        assert_eq!(
            source.select_all_by_name_cancellable_with_scratch(&[], &mut scratch, &|| false),
            Err(ProjectionError::ScratchOccupied)
        );
    }

    #[test]
    fn recipient_thread_destroys_retained_error_and_success_storage() {
        let names: Vec<_> = (0..4097)
            .map(|i| OsString::from(format!("file{i}")))
            .collect();
        let mut source = Selection::new();
        for name in &names {
            source.toggle(name);
        }
        let mut scratch = SelectionScratch::default();
        source
            .select_all_by_name_cancellable_with_scratch(&names, &mut scratch, &|| false)
            .unwrap();
        assert!(scratch.tracked_names() >= names.len() * 2);
        let mut failed = SelectionScratch::default();
        let calls = Cell::new(0);
        assert_eq!(
            source.set_band_cancellable_with_scratch(&names, &[], &mut failed, &|| {
                calls.set(calls.get() + 1);
                calls.get() == 5
            }),
            Err(ProjectionError::Cancelled)
        );
        assert!(failed.tracked_names() > 0);
        let caller = std::thread::current().id();
        // This test models a pre-admitted single-owner retirement slot.
        let (send, recv) = mpsc::sync_channel::<SelectionScratch>(1);
        let (allow, wait) = mpsc::sync_channel::<()>(0);
        let (done, cleaned) = mpsc::sync_channel(0);
        let recipient = std::thread::spawn(move || {
            for _ in 0..2 {
                let held = recv.recv().unwrap();
                let retained = held.tracked_names();
                wait.recv().unwrap();
                drop(held);
                done.send((std::thread::current().id(), retained)).unwrap();
            }
        });
        // The CPU call has returned and its old sets still belong to scratch.
        for owner in [failed, scratch] {
            send.send(owner).unwrap();
            assert!(cleaned.try_recv().is_err());
            allow.send(()).unwrap();
            let (cleaned_on, retained) = cleaned.recv().unwrap();
            assert_ne!(cleaned_on, caller);
            assert!(retained > 0);
        }
        recipient.join().unwrap();
    }
}
