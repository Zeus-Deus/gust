//! Directory listing. Runs on a worker thread — never the UI thread.
//!
//! Two passes: names+types first (cheap, from `getdents` d_type), then
//! metadata. Both are cancellable and generation-tagged.

use crate::entry::{Completeness, DirectorySnapshot, Entry, EntryKind, Metadata};
use crate::location::LocationKey;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::MetadataExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Cooperative cancellation. Every listing/remote call takes one.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
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
            snap.complete = Completeness::Partial {
                seen: snap.entries.len(),
            };
            snap.sort_entries();
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

/// Fill metadata for a snapshot's entries. Symlinks are lstat'ed (never
/// followed) and separately probed for target existence.
pub fn fill_metadata(snap: &mut DirectorySnapshot, cancel: &CancelToken) {
    let Some(dir) = snap.key.as_path().map(|p| p.to_path_buf()) else {
        return;
    };
    for e in snap.entries.iter_mut() {
        if cancel.is_cancelled() {
            return;
        }
        let p = dir.join(&e.name);
        let Ok(md) = std::fs::symlink_metadata(&p) else {
            continue;
        };
        let link_ok = if e.kind == EntryKind::Symlink {
            Some(std::fs::metadata(&p).is_ok())
        } else {
            None
        };
        e.meta = Some(Metadata {
            size: md.size(),
            mtime: md.mtime(),
            mode: md.mode(),
            uid: md.uid(),
            gid: md.gid(),
            dev: md.dev(),
            ino: md.ino(),
            link_ok,
        });
    }
    if !cancel.is_cancelled() {
        snap.sort_entries();
        snap.settled = true;
    }
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
}
