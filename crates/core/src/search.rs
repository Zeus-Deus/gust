//! Filter and scoped search.
//!
//! Two separate things, deliberately:
//! - `filter` (`/`) narrows the *already listed* current directory. No I/O.
//! - `find` (`:find`) walks a scope the user explicitly chose. Never a silent
//!   recursive crawl, never automatic on typing (plan §8, Nautilus #1157).

use crate::entry::Entry;
use crate::lister::CancelToken;
use crate::location::LocationKey;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

/// Subsequence match, case-insensitive on ASCII: "dwn" matches "Downloads".
pub fn fuzzy_matches(needle: &str, haystack: &OsStr) -> bool {
    if needle.is_empty() {
        return true;
    }
    let n: Vec<u8> = needle.bytes().map(|b| b.to_ascii_lowercase()).collect();
    let h = haystack.as_bytes();
    let mut ni = 0;
    for &c in h {
        if c.to_ascii_lowercase() == n[ni] {
            ni += 1;
            if ni == n.len() {
                return true;
            }
        }
    }
    false
}

/// Score a match so better matches sort first (lower is better).
/// Prefix matches beat substring matches beat scattered subsequences.
pub fn match_score(needle: &str, haystack: &OsStr) -> Option<u32> {
    if needle.is_empty() {
        return Some(0);
    }
    let hl = String::from_utf8_lossy(haystack.as_bytes()).to_ascii_lowercase();
    let nl = needle.to_ascii_lowercase();
    if hl.starts_with(&nl) {
        return Some(0);
    }
    if let Some(p) = hl.find(&nl) {
        return Some(1 + p as u32);
    }
    if fuzzy_matches(needle, haystack) {
        return Some(1000);
    }
    None
}

/// Filter an in-memory listing. Pure, no I/O — safe to run on every keystroke.
pub fn filter_entries<'a>(entries: &'a [Entry], needle: &str) -> Vec<&'a Entry> {
    let mut v: Vec<(u32, &Entry)> = entries
        .iter()
        .filter_map(|e| match_score(needle, &e.name).map(|s| (s, e)))
        .collect();
    v.sort_by_key(|(s, _)| *s);
    v.into_iter().map(|(_, e)| e).collect()
}

/// Type-ahead: find the next entry starting with `prefix`, beginning the
/// search *after* `from`, wrapping once.
///
/// This SELECTS an existing row. It never changes the directory contents and
/// never starts a search (the Nautilus #1157 regression test).
pub fn type_ahead_index(entries: &[Entry], prefix: &str, from: Option<usize>) -> Option<usize> {
    if prefix.is_empty() || entries.is_empty() {
        return None;
    }
    let p: Vec<u8> = prefix.bytes().map(|b| b.to_ascii_lowercase()).collect();
    let start = from.map(|i| i + 1).unwrap_or(0);
    let n = entries.len();
    for k in 0..n {
        let i = (start + k) % n;
        let name = entries[i].name.as_bytes();
        if name.len() >= p.len()
            && name[..p.len()]
                .iter()
                .map(|c| c.to_ascii_lowercase())
                .eq(p.iter().copied())
        {
            return Some(i);
        }
    }
    None
}

/// Accumulates type-ahead keystrokes.
///
/// Typing "Pro" must look for the prefix "Pro" — not for "P", then "r", then
/// "o" independently, which lands on whatever starts with "o". Characters
/// typed within `timeout` join the current buffer; a longer pause starts a new
/// one. Repeating the *same* single character cycles through matches, which is
/// the long-standing file-manager behaviour.
#[derive(Debug, Clone)]
pub struct TypeAhead {
    buffer: String,
    last_input_ms: u64,
    timeout_ms: u64,
}

impl Default for TypeAhead {
    fn default() -> Self {
        TypeAhead {
            buffer: String::new(),
            last_input_ms: 0,
            timeout_ms: 1000,
        }
    }
}

impl TypeAhead {
    pub fn new(timeout_ms: u64) -> Self {
        TypeAhead {
            buffer: String::new(),
            last_input_ms: 0,
            timeout_ms,
        }
    }

    pub fn buffer(&self) -> &str {
        &self.buffer
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    /// Feed one typed character at time `now_ms`.
    ///
    /// Returns the prefix to search for, and whether this is a "cycle to the
    /// next match" request rather than a fresh search.
    pub fn push(&mut self, ch: char, now_ms: u64) -> (String, bool) {
        let expired = now_ms.saturating_sub(self.last_input_ms) > self.timeout_ms;
        self.last_input_ms = now_ms;

        if expired {
            self.buffer.clear();
            self.buffer.push(ch);
            return (self.buffer.clone(), false);
        }

        // Same single character pressed again: cycle through matches.
        if self.buffer.len() == 1 && self.buffer.starts_with(ch) {
            return (self.buffer.clone(), true);
        }

        self.buffer.push(ch);
        (self.buffer.clone(), false)
    }
}

/// What a find is allowed to touch.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchScope {
    pub root: LocationKey,
    /// Recursion is opt-in and bounded.
    pub recursive: bool,
    pub max_depth: usize,
    /// Never cross into another filesystem (keeps a find off network mounts).
    pub stay_on_device: bool,
    pub include_hidden: bool,
    pub max_results: usize,
}

impl SearchScope {
    /// The safe default: current directory only, no recursion.
    pub fn current_dir(root: LocationKey) -> Self {
        SearchScope {
            root,
            recursive: false,
            max_depth: 1,
            stay_on_device: true,
            include_hidden: false,
            max_results: 5000,
        }
    }

    /// Explicit user widening.
    pub fn recursive(root: LocationKey, max_depth: usize) -> Self {
        SearchScope {
            root,
            recursive: true,
            max_depth,
            stay_on_device: true,
            include_hidden: false,
            max_results: 5000,
        }
    }

    /// A remote location must never be crawled implicitly.
    pub fn allows_remote_crawl(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub path: PathBuf,
    pub score: u32,
}

/// A folder full of generated files nobody searches for by name: build
/// output and caches. Searching "bench" in a project otherwise returns
/// hundreds of compiler objects before the user's own files. A cache marks
/// itself with `CACHEDIR.TAG` (the Cache Directory Tagging spec, which Cargo,
/// pip and many others follow); `node_modules` is the one big exception.
/// Only descent is skipped: a name match on the folder itself still shows.
fn is_generated_tree(dir: &std::path::Path, name: &OsStr) -> bool {
    if name == "node_modules" || name == "__pycache__" {
        return true;
    }
    std::fs::symlink_metadata(dir.join("CACHEDIR.TAG")).is_ok_and(|m| m.is_file())
}

/// Filename find within an explicit scope. Cancellable and bounded.
pub fn find_by_name(
    scope: &SearchScope,
    needle: &str,
    cancel: &CancelToken,
    mut on_hit: impl FnMut(&SearchHit),
) -> Vec<SearchHit> {
    let mut hits: Vec<(SearchHit, usize)> = Vec::new();
    let Some(root) = scope.root.as_path() else {
        return Vec::new();
    };
    let root_dev = std::fs::metadata(root).ok().map(|m| {
        use std::os::unix::fs::MetadataExt;
        m.dev()
    });

    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if cancel.is_cancelled() || hits.len() >= scope.max_results {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for item in rd.flatten() {
            if cancel.is_cancelled() || hits.len() >= scope.max_results {
                break;
            }
            let name = item.file_name();
            if !scope.include_hidden && name.as_bytes().first() == Some(&b'.') {
                continue;
            }
            let Ok(ft) = item.file_type() else { continue };
            if let Some(score) = match_score(needle, &name) {
                let hit = SearchHit {
                    path: item.path(),
                    score,
                };
                on_hit(&hit);
                hits.push((hit, depth));
            }
            // Descend only into real directories — never follow a symlink,
            // which is how a "search here" becomes an infinite loop or a
            // silent crawl into a network mount.
            if ft.is_dir()
                && scope.recursive
                && depth + 1 < scope.max_depth
                && !is_generated_tree(&item.path(), &name)
            {
                if scope.stay_on_device {
                    use std::os::unix::fs::MetadataExt;
                    let same = std::fs::metadata(item.path())
                        .ok()
                        .map(|m| Some(m.dev()) == root_dev)
                        .unwrap_or(false);
                    if !same {
                        continue;
                    }
                }
                stack.push((item.path(), depth + 1));
            }
        }
    }
    // Best match first; among equals the one nearer the folder being
    // browsed (what the user most likely meant), then by name so the order
    // does not depend on the order the disk returned entries in.
    hits.sort_by(|(a, da), (b, db)| {
        a.score
            .cmp(&b.score)
            .then(da.cmp(db))
            .then_with(|| a.path.cmp(&b.path))
    });
    hits.into_iter().map(|(h, _)| h).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::EntryKind;
    use std::ffi::OsString;
    use std::fs;

    fn e(n: &str) -> Entry {
        Entry::new(OsString::from(n), EntryKind::File)
    }

    #[test]
    fn type_ahead_buffers_a_multi_character_prefix() {
        // REGRESSION: typing "Pro" must search for "Pro". Treating each key
        // independently searched for "o" and selected the wrong file, which
        // then navigated the user into the wrong folder on Enter.
        let mut ta = TypeAhead::new(1000);
        assert_eq!(ta.push('P', 100), ("P".to_string(), false));
        assert_eq!(ta.push('r', 200), ("Pr".to_string(), false));
        assert_eq!(ta.push('o', 300), ("Pro".to_string(), false));
        assert_eq!(ta.buffer(), "Pro");
    }

    #[test]
    fn type_ahead_buffer_expires_after_a_pause() {
        let mut ta = TypeAhead::new(1000);
        ta.push('a', 1000);
        ta.push('b', 1100);
        assert_eq!(ta.buffer(), "ab");
        // Long pause: a new search starts.
        let (prefix, cycle) = ta.push('c', 5000);
        assert_eq!(prefix, "c");
        assert!(!cycle);
    }

    #[test]
    fn repeating_one_character_cycles_matches() {
        let mut ta = TypeAhead::new(1000);
        assert_eq!(ta.push('b', 100), ("b".to_string(), false));
        // Second 'b' in quick succession means "next match", not prefix "bb".
        assert_eq!(ta.push('b', 200), ("b".to_string(), true));
        assert_eq!(ta.buffer(), "b");
    }

    #[test]
    fn type_ahead_with_buffer_selects_the_right_entry() {
        // End-to-end of the fix: the fixture-style listing that broke.
        let entries = vec![
            e("Documents"),
            e("Downloads"),
            e("Music"),
            e("Pictures"),
            e("Projects"),
            e("Videos"),
        ];
        let mut ta = TypeAhead::new(1000);
        let mut cursor: Option<usize> = None;
        let mut t = 0;
        for ch in "Pro".chars() {
            t += 50;
            let (prefix, cycle) = ta.push(ch, t);
            let from = if cycle { cursor } else { None };
            cursor = type_ahead_index(&entries, &prefix, from);
        }
        assert_eq!(
            entries[cursor.expect("a match")].display_name(),
            "Projects",
            "typing Pro must land on Projects, not Pictures"
        );
    }

    #[test]
    fn fuzzy_subsequence_matches() {
        assert!(fuzzy_matches("dwn", OsStr::new("Downloads")));
        assert!(fuzzy_matches("", OsStr::new("anything")));
        assert!(!fuzzy_matches("zzz", OsStr::new("Downloads")));
    }

    #[test]
    fn prefix_scores_better_than_scattered() {
        let pre = match_score("doc", OsStr::new("documents")).unwrap();
        let scat = match_score("dts", OsStr::new("documents")).unwrap();
        assert!(pre < scat);
    }

    #[test]
    fn filter_orders_best_match_first() {
        let entries = vec![e("my-report.txt"), e("report.txt"), e("xyz.txt")];
        let got = filter_entries(&entries, "report");
        assert_eq!(got[0].display_name(), "report.txt");
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn type_ahead_selects_and_cycles_without_searching() {
        // The Nautilus #1157 lesson: typing selects a row in the CURRENT list.
        let entries = vec![e("alpha"), e("beta"), e("bravo")];
        let first = type_ahead_index(&entries, "b", None).unwrap();
        assert_eq!(first, 1);
        let second = type_ahead_index(&entries, "b", Some(first)).unwrap();
        assert_eq!(second, 2);
        // wraps back around
        assert_eq!(type_ahead_index(&entries, "b", Some(second)).unwrap(), 1);
    }

    #[test]
    fn type_ahead_returns_none_when_nothing_matches() {
        assert!(type_ahead_index(&[e("alpha")], "z", None).is_none());
    }

    #[test]
    fn default_scope_is_not_recursive() {
        let s = SearchScope::current_dir(LocationKey::local("/tmp"));
        assert!(!s.recursive);
        assert_eq!(s.max_depth, 1);
        assert!(!s.allows_remote_crawl());
    }

    #[test]
    fn find_is_bounded_by_depth() {
        let d = std::env::temp_dir().join(format!("gust-search-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("a/b/c")).unwrap();
        fs::write(d.join("target.txt"), b"x").unwrap();
        fs::write(d.join("a/target.txt"), b"x").unwrap();
        fs::write(d.join("a/b/c/target.txt"), b"x").unwrap();

        let shallow = find_by_name(
            &SearchScope::current_dir(LocationKey::local(&d)),
            "target",
            &CancelToken::new(),
            |_| {},
        );
        assert_eq!(shallow.len(), 1, "non-recursive scope stays put");

        let deep = find_by_name(
            &SearchScope::recursive(LocationKey::local(&d), 2),
            "target",
            &CancelToken::new(),
            |_| {},
        );
        assert_eq!(deep.len(), 2, "depth 2 sees root + one level");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn find_does_not_follow_symlinked_directories() {
        let d = std::env::temp_dir().join(format!("gust-symloop-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("real")).unwrap();
        fs::write(d.join("real/target.txt"), b"x").unwrap();
        // A self-referential symlink would hang a naive walker.
        std::os::unix::fs::symlink(&d, d.join("real/loop")).unwrap();

        let hits = find_by_name(
            &SearchScope::recursive(LocationKey::local(&d), 6),
            "target",
            &CancelToken::new(),
            |_| {},
        );
        assert_eq!(hits.len(), 1);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn live_search_ranks_prefix_then_nearer_then_name() {
        // Search as you type shows results in this order, so it must be
        // stable: what you typed as a prefix first, the copy nearer to where
        // you are before a deeper one, and never the disk's own order.
        let d = std::env::temp_dir().join(format!("gust-rank-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("a/b")).unwrap();
        fs::write(d.join("a/b/bench.md"), b"x").unwrap();
        fs::write(d.join("bench.md"), b"x").unwrap();
        fs::write(d.join("a/field-bench.md"), b"x").unwrap();
        fs::write(d.join("benches.txt"), b"x").unwrap();
        let hits = find_by_name(
            &SearchScope::recursive(LocationKey::local(&d), 8),
            "bench",
            &CancelToken::new(),
            |_| {},
        );
        let rel: Vec<String> = hits
            .iter()
            .map(|h| h.path.strip_prefix(&d).unwrap().display().to_string())
            .collect();
        assert_eq!(
            rel,
            [
                "bench.md",
                "benches.txt",
                "a/b/bench.md",
                "a/field-bench.md"
            ]
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn find_skips_build_output_and_caches() {
        let d = std::env::temp_dir().join(format!("gust-cachedir-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("target/release/build")).unwrap();
        fs::write(
            d.join("target/CACHEDIR.TAG"),
            b"Signature: 8a477f597d28d172789f06886806bc55",
        )
        .unwrap();
        fs::write(d.join("target/release/build/bench.o"), b"x").unwrap();
        fs::create_dir_all(d.join("node_modules/bench")).unwrap();
        fs::write(d.join("node_modules/bench/bench.js"), b"x").unwrap();
        fs::write(d.join("bench.md"), b"x").unwrap();
        let hits = find_by_name(
            &SearchScope::recursive(LocationKey::local(&d), 8),
            "bench",
            &CancelToken::new(),
            |_| {},
        );
        let rel: Vec<String> = hits
            .iter()
            .map(|h| h.path.strip_prefix(&d).unwrap().display().to_string())
            .collect();
        assert_eq!(rel, ["bench.md"]);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn find_stops_when_cancelled() {
        let d = std::env::temp_dir().join(format!("gust-cancel-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        for i in 0..50 {
            fs::write(d.join(format!("target{i}")), b"x").unwrap();
        }
        let tok = CancelToken::new();
        tok.cancel();
        let hits = find_by_name(
            &SearchScope::current_dir(LocationKey::local(&d)),
            "target",
            &tok,
            |_| {},
        );
        assert!(hits.is_empty());
        fs::remove_dir_all(&d).ok();
    }
}

#[cfg(test)]
mod perf_tests {
    use super::*;
    use crate::entry::{Entry, EntryKind};

    fn entries(n: usize) -> Vec<Entry> {
        (0..n)
            .map(|i| Entry::new(format!("entry-{i:06}.dat").into(), EntryKind::File))
            .collect()
    }

    /// A 100k directory must filter in well under a second.
    ///
    /// Guards a real bug: the model's visible-row recompute was O(n²)
    /// (`Vec::contains` per entry), which made a six-figure folder appear to
    /// hang forever. Filtering itself must stay linear so that fix holds.
    #[test]
    fn filtering_a_hundred_thousand_entries_is_fast() {
        let all = entries(100_000);
        let t = std::time::Instant::now();
        let hits = filter_entries(&all, "entry-09");
        let elapsed = t.elapsed();
        assert!(!hits.is_empty(), "the needle should match something");
        assert!(
            elapsed < std::time::Duration::from_millis(800),
            "filtering 100k entries took {elapsed:?} — quadratic behaviour is back"
        );
    }

    /// Every returned reference must point into the slice that was passed in,
    /// because the caller converts them back to indices by pointer offset.
    #[test]
    fn filter_results_are_borrows_into_the_input_slice() {
        let all = entries(500);
        let hits = filter_entries(&all, "entry-0001");
        let base = all.as_ptr();
        for h in &hits {
            let off = unsafe { (*h as *const Entry).offset_from(base) };
            assert!(
                off >= 0 && (off as usize) < all.len(),
                "a result pointed outside the input slice"
            );
            assert_eq!(
                all[off as usize].name, h.name,
                "pointer offset must identify the same entry"
            );
        }
    }
}
