//! Worker-only reuse of sorted local names; file metadata is always refreshed.

use gust_core::entry::{Completeness, DirectorySnapshot, Entry, EntryKind};
use gust_core::lister::{list_names, CancelToken};
use gust_core::LocationKey;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_LOCATIONS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp(u64, u64, i64, i64, i64, i64);

fn stamp(path: &Path) -> Option<Stamp> {
    let m = path.metadata().ok()?;
    m.is_dir().then(|| {
        Stamp(
            m.dev(),
            m.ino(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
    })
}

struct Names {
    path: PathBuf,
    stamp: Stamp,
    rows: Vec<(OsString, EntryKind)>,
    bytes: usize,
}

#[derive(Default)]
struct Cache {
    recent: VecDeque<Arc<Names>>,
    bytes: usize,
}

impl Cache {
    fn get(&mut self, path: &Path, stamp: Stamp) -> Option<Arc<Names>> {
        let index = self.recent.iter().position(|n| n.path == path)?;
        let names = self.recent.remove(index)?;
        if names.stamp != stamp {
            self.bytes -= names.bytes;
            return None;
        }
        self.recent.push_back(names.clone());
        Some(names)
    }

    fn insert(&mut self, path: &Path, stamp: Stamp, snapshot: &DirectorySnapshot) {
        if let Some(i) = self.recent.iter().position(|n| n.path == path) {
            self.bytes -= self.recent.remove(i).unwrap().bytes;
        }
        let bytes = std::mem::size_of::<Names>()
            + path.as_os_str().len()
            + snapshot.entries.len() * std::mem::size_of::<(OsString, EntryKind)>()
            + snapshot.entries.iter().map(|e| e.name.len()).sum::<usize>();
        if bytes > MAX_BYTES {
            return;
        }
        while self.bytes + bytes > MAX_BYTES || self.recent.len() >= MAX_LOCATIONS {
            if let Some(old) = self.recent.pop_front() {
                self.bytes -= old.bytes;
            }
        }
        let rows = snapshot
            .entries
            .iter()
            .map(|e| (e.name.clone(), e.kind))
            .collect();
        self.recent.push_back(Arc::new(Names {
            path: path.to_owned(),
            stamp,
            rows,
            bytes,
        }));
        self.bytes += bytes;
    }
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Cache::default()))
}

pub fn list(
    key: &LocationKey,
    generation: u64,
    cancel: &CancelToken,
    reuse: bool,
) -> DirectorySnapshot {
    let local = key.as_path().filter(|p| !gust_core::stall::needs_probe(p));
    let before = local.and_then(stamp);
    if reuse && !cancel.is_cancelled() {
        if let (Some(path), Some(before)) = (local, before) {
            let names = cache().lock().ok().and_then(|mut c| c.get(path, before));
            if let Some(names) = names {
                let mut snapshot = DirectorySnapshot::empty(key.clone(), generation);
                snapshot.entries = names
                    .rows
                    .iter()
                    .map(|(n, k)| Entry::new(n.clone(), *k))
                    .collect();
                if !cancel.is_cancelled() && stamp(path) == Some(before) {
                    snapshot.complete = Completeness::Complete;
                    snapshot.settled = true;
                    return snapshot;
                }
            }
        }
    }
    let snapshot = list_names(key, generation, cancel, |_| {});
    if !cancel.is_cancelled() && snapshot.complete == Completeness::Complete {
        if let (Some(path), Some(before)) = (local, before) {
            if stamp(path) == Some(before) {
                if let Ok(mut cache) = cache().lock() {
                    cache.insert(path, before, &snapshot);
                }
            }
        }
    }
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "gust-name-cache-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn reuse_preserves_order_bytes_and_new_generation_but_never_metadata() {
        use std::os::unix::ffi::OsStringExt;
        let path = fixture();
        for name in [
            OsString::from("photo10"),
            OsString::from("photo2"),
            OsString::from_vec(vec![0xff]),
        ] {
            std::fs::write(path.join(name), b"old").unwrap();
        }
        let key = LocationKey::local(&path);
        let cancel = CancelToken::new();
        let original = list(&key, 1, &cancel, false);
        std::fs::write(path.join("photo2"), b"new contents").unwrap();
        let reused = list(&key, 9, &cancel, true);
        assert_eq!(reused.generation, 9);
        assert_eq!(
            original.entries.iter().map(|e| &e.name).collect::<Vec<_>>(),
            reused.entries.iter().map(|e| &e.name).collect::<Vec<_>>()
        );
        assert!(reused.entries.iter().all(|e| e.meta.is_none()));
        assert!(cache()
            .lock()
            .unwrap()
            .get(&path, stamp(&path).unwrap())
            .is_some());
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn membership_changes_and_refresh_replace_cached_names() {
        let path = fixture();
        let key = LocationKey::local(&path);
        let cancel = CancelToken::new();
        list(&key, 1, &cancel, true);
        std::fs::write(path.join("new"), b"x").unwrap();
        assert_eq!(list(&key, 2, &cancel, true).entries.len(), 1);
        std::fs::remove_file(path.join("new")).unwrap();
        assert!(list(&key, 3, &cancel, false).entries.is_empty());
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn explicit_refresh_bypasses_even_a_matching_cached_stamp() {
        let path = fixture();
        let key = LocationKey::local(&path);
        let cancel = CancelToken::new();
        let mut old = DirectorySnapshot::empty(key.clone(), 1);
        old.entries
            .push(Entry::new(OsString::from("cached-only"), EntryKind::File));
        cache()
            .lock()
            .unwrap()
            .insert(&path, stamp(&path).unwrap(), &old);
        assert_eq!(list(&key, 2, &cancel, true).entries.len(), 1);
        assert!(list(&key, 3, &cancel, false).entries.is_empty());
        assert!(list(&key, 4, &cancel, true).entries.is_empty());
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn lru_budget_and_stamps_are_enforced() {
        let mut cache = Cache::default();
        let key = LocationKey::local("/fixture");
        let mut snapshot = DirectorySnapshot::empty(key, 1);
        let s = Stamp(1, 2, 3, 4, 5, 6);
        for i in 0..MAX_LOCATIONS + 1 {
            cache.insert(Path::new(&format!("/{i}")), s, &snapshot);
        }
        assert!(cache.get(Path::new("/0"), s).is_none());
        assert!(cache
            .get(Path::new("/1"), Stamp(1, 2, 3, 7, 5, 6))
            .is_none());
        snapshot.entries.push(Entry::new(
            OsString::from("x".repeat(MAX_BYTES)),
            EntryKind::File,
        ));
        cache.insert(Path::new("/large"), s, &snapshot);
        assert!(cache.get(Path::new("/large"), s).is_none());
        assert!(cache.bytes <= MAX_BYTES && cache.recent.len() <= MAX_LOCATIONS);
    }
}
