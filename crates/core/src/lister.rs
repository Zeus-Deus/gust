//! Directory listing. Runs on a worker thread — never the UI thread.
//!
//! Two passes: names+types first (cheap, from `getdents` d_type), then
//! metadata. Both are cancellable and generation-tagged.

use crate::entry::{Completeness, DirectorySnapshot, Entry, EntryKind, Metadata};
use crate::location::LocationKey;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Cooperative cancellation. Every listing/remote call takes one.
///
/// A transfer's token can also be paused. Workers call [`checkpoint`] where
/// they would check for cancellation: it waits there while paused, and a
/// cancel ends the wait at once. [`is_cancelled`] never waits, so the GUI
/// thread can always ask.
///
/// [`checkpoint`]: CancelToken::checkpoint
/// [`is_cancelled`]: CancelToken::is_cancelled
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst) && !self.is_cancelled()
    }
    /// Worker side: wait while paused, then report cancellation. Never
    /// call from the GUI thread.
    pub fn checkpoint(&self) -> bool {
        while self.paused.load(Ordering::SeqCst) && !self.is_cancelled() {
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        self.is_cancelled()
    }
}

#[cfg(test)]
mod pause_tests {
    use super::CancelToken;
    use std::time::{Duration, Instant};

    #[test]
    fn a_paused_checkpoint_waits_until_resumed() {
        let t = CancelToken::new();
        t.pause();
        let w = t.clone();
        let h = std::thread::spawn(move || {
            let s = Instant::now();
            let c = w.checkpoint();
            (c, s.elapsed())
        });
        std::thread::sleep(Duration::from_millis(250));
        assert!(t.is_paused());
        t.resume();
        let (cancelled, waited) = h.join().unwrap();
        assert!(!cancelled);
        assert!(
            waited >= Duration::from_millis(200),
            "returned early: {waited:?}"
        );
    }

    #[test]
    fn cancel_ends_a_pause_at_once() {
        let t = CancelToken::new();
        t.pause();
        let w = t.clone();
        let h = std::thread::spawn(move || w.checkpoint());
        std::thread::sleep(Duration::from_millis(100));
        let s = Instant::now();
        t.cancel();
        assert!(
            h.join().unwrap(),
            "a cancelled checkpoint reports cancelled"
        );
        assert!(s.elapsed() < Duration::from_millis(500));
        assert!(!t.is_paused(), "a cancelled job is not 'paused'");
    }

    #[test]
    fn is_cancelled_never_waits() {
        let t = CancelToken::new();
        t.pause();
        let s = Instant::now();
        assert!(!t.is_cancelled());
        assert!(s.elapsed() < Duration::from_millis(20));
    }
}

/// How often the lister reports progress while walking a huge directory.
const CHUNK: usize = 2000;

/// List names and types only. Calls `on_chunk` every `CHUNK` entries so the UI
/// can show a partial directory instead of waiting for 100k entries.
pub fn list_names(
    key: &LocationKey,
    generation: u64,
    cancel: &CancelToken,
    mut on_chunk: impl FnMut(&DirectorySnapshot),
) -> DirectorySnapshot {
    let mut snap = DirectorySnapshot::empty(key.clone(), generation);

    let path = match key.as_path() {
        Some(p) => p,
        None => {
            snap.complete = Completeness::Failed {
                error: "non-local listing not handled by this function".into(),
            };
            return snap;
        }
    };

    let rd = match std::fs::read_dir(path) {
        Ok(rd) => rd,
        Err(e) => {
            snap.complete = Completeness::Failed {
                error: format!("{e}"),
            };
            return snap;
        }
    };

    for item in rd {
        if cancel.is_cancelled() {
            snap.complete = Completeness::Failed {
                error: "cancelled".into(),
            };
            return snap;
        }
        let item = match item {
            Ok(i) => i,
            Err(_) => continue, // a single unreadable entry must not kill the listing
        };
        // file_type() here is from d_type / lstat: it does NOT follow symlinks.
        let kind = match item.file_type() {
            Ok(ft) => {
                if ft.is_dir() {
                    EntryKind::Dir
                } else if ft.is_symlink() {
                    EntryKind::Symlink
                } else if ft.is_file() {
                    EntryKind::File
                } else {
                    EntryKind::Special
                }
            }
            Err(_) => EntryKind::Unknown,
        };
        let name = OsString::from_vec(item.file_name().into_vec());
        snap.entries.push(Entry::new(name, kind));

        if snap.entries.len().is_multiple_of(CHUNK) {
            // Partial chunks arrive in directory order. Sorting each one
            // re-sorted the whole growing list 50 times for a 100k folder,
            // and no caller used the partial order.
            snap.complete = Completeness::Partial {
                seen: snap.entries.len(),
            };
            on_chunk(&snap);
        }
    }

    snap.sort_entries();
    snap.complete = Completeness::Complete;
    // Name sort settles immediately; size/mtime need the metadata pass.
    snap.settled = !snap.sort_needs_metadata();
    on_chunk(&snap);
    snap
}

/// Read one entry's metadata. Symlinks are lstat'ed (never followed) and
/// separately probed for whether their target exists.
pub fn stat_entry(dir: &Path, name: &OsStr, kind: EntryKind) -> Option<Metadata> {
    let p = dir.join(name);
    let md = std::fs::symlink_metadata(&p).ok()?;
    let link_ok = if kind == EntryKind::Symlink {
        Some(std::fs::metadata(&p).is_ok())
    } else {
        None
    };
    Some(Metadata {
        size: md.size(),
        mtime: md.mtime(),
        mode: md.mode(),
        uid: md.uid(),
        gid: md.gid(),
        dev: md.dev(),
        ino: md.ino(),
        link_ok,
    })
}

/// Fill metadata for a snapshot's entries. Symlinks are lstat'ed (never
/// followed) and separately probed for target existence.
///
/// Every entry is read. Use [`fill_metadata_for`] when only some rows are
/// on screen: a 100k folder whose sort does not depend on metadata settles
/// on the first screenful instead of on 100k `stat` calls.
pub fn fill_metadata(snap: &mut DirectorySnapshot, cancel: &CancelToken) {
    let all: Vec<usize> = (0..snap.entries.len()).collect();
    if fill_metadata_for(snap, &all, cancel) {
        snap.sort_entries();
        snap.settled = true;
    }
}

/// Fill metadata for the entries at `indices` only, leaving the rest `None`.
/// Returns false if cancelled part-way (what was read is kept).
pub fn fill_metadata_for(
    snap: &mut DirectorySnapshot,
    indices: &[usize],
    cancel: &CancelToken,
) -> bool {
    let Some(dir) = snap.key.as_path().map(|p| p.to_path_buf()) else {
        return false;
    };
    for &i in indices {
        if cancel.is_cancelled() {
            return false;
        }
        let Some(e) = snap.entries.get_mut(i) else {
            continue;
        };
        if e.meta.is_none() {
            e.meta = stat_entry(&dir, &e.name, e.kind);
        }
    }
    !cancel.is_cancelled()
}

/// How many rows count as "the first screen". Generous: a 4K portrait
/// monitor at compact density shows about 110 rows.
pub const FIRST_WINDOW: usize = 256;

/// Which rows of a listing to read metadata for before calling it settled:
/// the first `size` shown rows, plus `size` rows centred on `focus` (a file
/// the cursor is about to land on, e.g. "Show in folder"). `shown` is the
/// on-screen order as indices into the entries. Returns entry indices,
/// deduplicated, in on-screen order.
pub fn first_window(shown: &[usize], focus: Option<usize>, size: usize) -> Vec<usize> {
    let mut rows: Vec<usize> = (0..shown.len().min(size)).collect();
    if let Some(f) = focus.filter(|f| *f < shown.len()) {
        let lo = f.saturating_sub(size / 2);
        let hi = (lo + size).min(shown.len());
        rows.extend(lo..hi);
    }
    rows.sort_unstable();
    rows.dedup();
    rows.into_iter().map(|r| shown[r]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::SortField;
    use std::fs;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("gust-list-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn lists_names_and_types_without_following_symlinks() {
        let d = tmpdir("kinds");
        fs::create_dir(d.join("sub")).unwrap();
        fs::write(d.join("a.txt"), b"hi").unwrap();
        std::os::unix::fs::symlink("/nonexistent-target", d.join("dangling")).unwrap();

        let key = LocationKey::local(&d);
        let snap = list_names(&key, 1, &CancelToken::new(), |_| {});
        assert_eq!(snap.complete, Completeness::Complete);

        let find = |n: &str| {
            snap.entries
                .iter()
                .find(|e| e.display_name() == n)
                .unwrap_or_else(|| panic!("missing {n}"))
                .kind
        };
        assert_eq!(find("sub"), EntryKind::Dir);
        assert_eq!(find("a.txt"), EntryKind::File);
        // A dangling symlink must list as a symlink, not vanish or error.
        assert_eq!(find("dangling"), EntryKind::Symlink);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn metadata_marks_dangling_symlink() {
        let d = tmpdir("dangling");
        std::os::unix::fs::symlink("/definitely/not/here", d.join("broken")).unwrap();
        let key = LocationKey::local(&d);
        let mut snap = list_names(&key, 1, &CancelToken::new(), |_| {});
        fill_metadata(&mut snap, &CancelToken::new());
        let e = snap
            .entries
            .iter()
            .find(|e| e.display_name() == "broken")
            .unwrap();
        assert_eq!(e.meta.unwrap().link_ok, Some(false));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn cancelled_listing_reports_failure_not_partial_success() {
        let d = tmpdir("cancel");
        for i in 0..10 {
            fs::write(d.join(format!("f{i}")), b"x").unwrap();
        }
        let key = LocationKey::local(&d);
        let tok = CancelToken::new();
        tok.cancel();
        let snap = list_names(&key, 1, &tok, |_| {});
        assert!(matches!(snap.complete, Completeness::Failed { .. }));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn missing_directory_fails_cleanly() {
        let key = LocationKey::local("/gust-does-not-exist-xyz");
        let snap = list_names(&key, 7, &CancelToken::new(), |_| {});
        assert!(matches!(snap.complete, Completeness::Failed { .. }));
        assert_eq!(snap.generation, 7, "generation survives failure");
    }

    #[test]
    fn size_sort_only_settles_after_metadata() {
        let d = tmpdir("settle");
        fs::write(d.join("big"), vec![0u8; 4096]).unwrap();
        fs::write(d.join("small"), b"x").unwrap();
        let key = LocationKey::local(&d);
        let mut snap = list_names(&key, 1, &CancelToken::new(), |_| {});
        snap.sort.field = SortField::Size;
        snap.settled = false;
        snap.sort_entries();
        assert!(!snap.settled);
        fill_metadata(&mut snap, &CancelToken::new());
        assert!(snap.settled);
        let names: Vec<_> = snap.entries.iter().map(|e| e.display_name()).collect();
        assert_eq!(names, vec!["small", "big"]);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn first_window_is_the_top_of_the_screen_plus_the_focus() {
        let shown: Vec<usize> = (0..1000).rev().collect(); // on-screen order != entry order
        let w = first_window(&shown, None, 10);
        assert_eq!(w, (990..1000).rev().collect::<Vec<_>>());
        let w = first_window(&shown, Some(500), 10);
        assert_eq!(w.len(), 20, "top 10 plus 10 around the focus: {w:?}");
        assert!(w.contains(&shown[500]));
        // A focus near the top overlaps and is not counted twice.
        assert_eq!(first_window(&shown, Some(3), 10).len(), 10);
        // Small folders: everything.
        assert_eq!(first_window(&[4, 2, 0], None, 256), vec![4, 2, 0]);
        assert!(first_window(&[], Some(9), 256).is_empty());
    }

    #[test]
    fn metadata_for_a_window_reads_only_those_rows() {
        let d = tmpdir("window");
        for i in 0..50 {
            fs::write(d.join(format!("f{i:02}")), vec![b'x'; i]).unwrap();
        }
        let key = LocationKey::local(&d);
        let mut snap = list_names(&key, 1, &CancelToken::new(), |_| {});
        let want = [0usize, 7, 49];
        assert!(fill_metadata_for(&mut snap, &want, &CancelToken::new()));
        for (i, e) in snap.entries.iter().enumerate() {
            assert_eq!(e.meta.is_some(), want.contains(&i), "row {i}");
        }
        // Sizes are real, not defaults.
        let e = &snap.entries[7];
        assert_eq!(
            e.meta.unwrap().size as usize,
            fs::metadata(d.join(&e.name)).unwrap().len() as usize
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_cancelled_window_fill_says_so() {
        let d = tmpdir("window-cancel");
        fs::write(d.join("a"), b"a").unwrap();
        let key = LocationKey::local(&d);
        let mut snap = list_names(&key, 1, &CancelToken::new(), |_| {});
        let tok = CancelToken::new();
        tok.cancel();
        assert!(!fill_metadata_for(&mut snap, &[0], &tok));
        assert!(snap.entries[0].meta.is_none());
        fs::remove_dir_all(&d).ok();
    }

    /// The point of the window: a 100k folder must not cost 100k stats
    /// before it can settle. Guarded by count, not by wall time, so it
    /// cannot flake on a slow CI box.
    #[test]
    fn settling_a_big_name_sorted_folder_stats_one_screenful() {
        let d = tmpdir("window-big");
        for i in 0..3000 {
            fs::File::create(d.join(format!("n{i}"))).unwrap();
        }
        let key = LocationKey::local(&d);
        let mut snap = list_names(&key, 1, &CancelToken::new(), |_| {});
        assert!(snap.settled, "a name sort settles on names alone");
        let shown: Vec<usize> = (0..snap.entries.len()).collect();
        let w = first_window(&shown, None, FIRST_WINDOW);
        fill_metadata_for(&mut snap, &w, &CancelToken::new());
        let read = snap.entries.iter().filter(|e| e.meta.is_some()).count();
        assert_eq!(read, FIRST_WINDOW);
        fs::remove_dir_all(&d).ok();
    }
}
