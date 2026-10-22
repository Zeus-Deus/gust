//! Freedesktop Trash.
//!
//! Trash and permanent delete are different commands with different text
//! (DESIGN.md §8). Where trash is unsupported it stays unsupported — a delete
//! is never silently substituted.
//!
//! The trash directory is an explicit parameter, never read from process
//! globals inside the operation. That keeps tests honest (a test can never
//! accidentally touch the user's real Trash) and makes parallel execution
//! race-free.

#[path = "trash_volume.rs"]
mod volume;
pub use volume::{
    locations, select_location, RestoredReceipt, TrashLocation, TrashPartial, TrashReceipt,
};

use crate::anchor::resolve_parent;
use gust_core::location::Cap;
use std::io;
use std::path::{Path, PathBuf};

/// A freedesktop trash location: `files/` holds the data, `info/` the records.
#[derive(Debug, Clone, PartialEq)]
pub struct TrashDir {
    pub files: PathBuf,
    pub info: PathBuf,
}

impl TrashDir {
    /// Build a trash rooted at `base` (i.e. `base/files`, `base/info`).
    pub fn at(base: impl AsRef<Path>) -> Self {
        let b = base.as_ref();
        TrashDir {
            files: b.join("files"),
            info: b.join("info"),
        }
    }

    /// The user's home trash: `$XDG_DATA_HOME/Trash`.
    ///
    /// Resolved once at the call site and passed down, so no deeper function
    /// depends on environment state.
    pub fn home() -> Option<Self> {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
        Some(TrashDir::at(base.join("Trash")))
    }

    /// The directory the trash lives in (used for the same-filesystem test).
    pub fn root(&self) -> &Path {
        self.files.parent().unwrap_or(Path::new("/"))
    }
}

/// Device id of a path, for the "is this the same filesystem?" test.
fn dev_of(p: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(p).ok().map(|m| m.dev())
}

/// Is trash available for `path` given `trash`?
///
/// A trash move is a rename, which cannot cross filesystems. Anything we
/// cannot prove reports `Unknown` rather than pretending.
pub fn trash_capability(path: &Path, trash: &TrashDir) -> Cap {
    // Compare against the nearest existing ancestor of the trash root, since
    // the trash dirs may not have been created yet.
    let mut probe = trash.root().to_path_buf();
    while !probe.exists() {
        match probe.parent() {
            Some(p) => probe = p.to_path_buf(),
            None => return Cap::Unknown,
        }
    }
    let src = path.parent().unwrap_or(path);
    match (dev_of(src), dev_of(&probe)) {
        (Some(a), Some(b)) if a == b => Cap::Yes,
        (Some(_), Some(_)) => {
            // The explicit supplied location is on another volume. Selection
            // is separate; never substitute a permanent delete here.
            Cap::Unknown
        }
        _ => Cap::Unknown,
    }
}

fn percent_encode(bytes: &[u8]) -> String {
    let mut s = String::new();
    for &b in bytes {
        let unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/');
        if unreserved {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

/// Now, in LOCAL time: the freedesktop Trash spec's DeletionDate is local,
/// and GIO/Nautilus write it that way, so records must agree.
fn iso_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        + gust_core::properties::local_offset_seconds();
    let days = secs / 86400;
    let rem = secs % 86400;
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // civil_from_days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}")
}

/// Result of a trash operation — enough to restore it.
#[derive(Debug, Clone)]
pub struct TrashedItem {
    pub original: PathBuf,
    pub trashed_name: std::ffi::OsString,
    pub info_file: PathBuf,
}

/// Move a file into `trash`, writing the `.trashinfo` record first.
///
/// Returns `Err(Unsupported)` if trash is unavailable — the caller must then
/// offer a clearly-labelled permanent delete, not silently perform one.
pub fn trash_path(path: &Path, trash: &TrashDir) -> io::Result<TrashedItem> {
    if trash_capability(path, trash) != Cap::Yes {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "trash is not available for this location",
        ));
    }
    TrashLocation {
        trash: trash.clone(),
        topdir: None,
    }
    .trash_path(path, &gust_core::lister::CancelToken::new())
    .map(|receipt| receipt.item)
}

/// Restore without replacing any current name. Typed callers retain the
/// stronger original data/info identity receipt across this operation.
pub fn restore(item: &TrashedItem, trash: &TrashDir) -> io::Result<()> {
    let location = TrashLocation {
        trash: trash.clone(),
        topdir: None,
    };
    let cancel = gust_core::lister::CancelToken::new();
    let receipt = location.receipt_named(&item.trashed_name, &cancel)?;
    if receipt.item.original != item.original || receipt.item.info_file != item.info_file {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Trash receipt changed; data was preserved",
        ));
    }
    location.restore(&receipt, &cancel).map(|_| ())
}

/// One item in the Trash, as the Trash view lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct TrashEntry {
    /// Name inside `files/` (unique within the Trash).
    pub trashed_name: std::ffi::OsString,
    /// Where it was deleted from.
    pub original: PathBuf,
    /// `YYYY-MM-DDThh:mm:ss`, as the record says.
    pub deleted: String,
}

/// Existing restorable records, newest first. Compatibility callers retain
/// their Vec result; typed location callers can inspect enumeration errors.
pub fn list(trash: &TrashDir) -> Vec<TrashEntry> {
    TrashLocation {
        trash: trash.clone(),
        topdir: None,
    }
    .list()
    .unwrap_or_default()
}

pub fn restore_named(trash: &TrashDir, name: &std::ffi::OsStr) -> io::Result<()> {
    let location = TrashLocation {
        trash: trash.clone(),
        topdir: None,
    };
    let cancel = gust_core::lister::CancelToken::new();
    let receipt = location.receipt_named(name, &cancel)?;
    location.restore(&receipt, &cancel).map(|_| ())
}

/// Permanently delete. Deliberately separate from `trash_path`, and callers
/// must have shown a distinct confirmation.
pub fn delete_permanently(path: &Path) -> io::Result<()> {
    let (dir, name) = resolve_parent(path)?;
    let st = dir.stat_child(&name)?;
    let is_dir = (st.st_mode & 0o170000) == 0o040000;
    if is_dir {
        rustix::fs::unlinkat(dir.as_fd(), &name, rustix::fs::AtFlags::REMOVEDIR)?;
    } else {
        rustix::fs::unlinkat(dir.as_fd(), &name, rustix::fs::AtFlags::empty())?;
    }
    Ok(())
}

/// Wording shown to the user. Trash and delete must never share a label.
pub fn action_label(cap: Cap) -> &'static str {
    match cap {
        Cap::Yes => "Move to Trash",
        _ => "Delete permanently…",
    }
}

pub fn is_destructive_without_recovery(cap: Cap) -> bool {
    cap != Cap::Yes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Every test gets a PRIVATE trash inside its own temp dir. No environment
    /// variable is touched, so tests are parallel-safe and the user's real
    /// Trash can never be reached.
    fn fixture(tag: &str) -> (PathBuf, TrashDir) {
        let root = std::env::temp_dir().join(format!(
            "gust-trash-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("work")).unwrap();
        let trash = TrashDir::at(root.join("Trash"));
        (root, trash)
    }

    #[test]
    fn trash_then_restore_round_trip() {
        let (root, trash) = fixture("roundtrip");
        let f = root.join("work/doc.txt");
        fs::write(&f, b"important").unwrap();

        let item = trash_path(&f, &trash).unwrap();
        assert!(!f.exists(), "file left its original location");
        assert!(item.info_file.exists(), "trashinfo written");

        restore(&item, &trash).unwrap();
        assert_eq!(fs::read(&f).unwrap(), b"important");
        assert!(!item.info_file.exists(), "info cleaned up after restore");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn trashinfo_percent_encodes_the_original_path() {
        let (root, trash) = fixture("encode");
        let f = root.join("work/a b#c.txt");
        fs::write(&f, b"x").unwrap();
        let item = trash_path(&f, &trash).unwrap();
        let info = fs::read_to_string(&item.info_file).unwrap();
        assert!(info.contains("%20"), "space encoded: {info}");
        assert!(info.contains("%23"), "hash encoded: {info}");
        assert!(info.starts_with("[Trash Info]"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn second_file_with_same_name_gets_a_distinct_slot() {
        let (root, trash) = fixture("dupe");
        for i in 0..2 {
            let f = root.join("work/same.txt");
            fs::write(&f, format!("copy {i}")).unwrap();
            trash_path(&f, &trash).unwrap();
        }
        let n = fs::read_dir(&trash.files).unwrap().count();
        assert_eq!(n, 2, "both files kept, neither overwritten");
        let infos = fs::read_dir(&trash.info).unwrap().count();
        assert_eq!(infos, 2, "each has its own restore record");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn restore_refuses_to_overwrite_a_new_file_at_the_original_path() {
        let (root, trash) = fixture("occupied");
        let f = root.join("work/doc.txt");
        fs::write(&f, b"old").unwrap();
        let item = trash_path(&f, &trash).unwrap();
        // The user created NEW work at the same path afterwards.
        fs::write(&f, b"new work").unwrap();
        let err = restore(&item, &trash).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&f).unwrap(), b"new work", "new work preserved");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn trash_and_delete_have_different_labels() {
        assert_ne!(action_label(Cap::Yes), action_label(Cap::No));
        assert!(action_label(Cap::Yes).contains("Trash"));
        assert!(action_label(Cap::No).contains("permanently"));
        assert!(is_destructive_without_recovery(Cap::Unknown));
        assert!(!is_destructive_without_recovery(Cap::Yes));
    }

    #[test]
    fn cross_filesystem_trash_refuses_instead_of_deleting() {
        // A trash on a different filesystem cannot be reached by rename.
        // The file must survive, and the error must be explicit.
        let (root, _) = fixture("crossfs");
        let f = root.join("work/keepme.txt");
        fs::write(&f, b"precious").unwrap();

        // /dev/shm is a separate tmpfs mount from the test's filesystem.
        let other = TrashDir::at("/dev/shm/gust-not-my-trash");
        if dev_of(Path::new("/dev/shm")) != dev_of(&root) {
            let cap = trash_capability(&f, &other);
            assert_ne!(cap, Cap::Yes, "different filesystem must not claim trash");
            let err = trash_path(&f, &other).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::Unsupported);
            assert_eq!(
                fs::read(&f).unwrap(),
                b"precious",
                "refused trash must never delete the file"
            );
        }
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn permanent_delete_removes_file() {
        let (root, _) = fixture("perm");
        let f = root.join("work/gone.txt");
        fs::write(&f, b"x").unwrap();
        delete_permanently(&f).unwrap();
        assert!(!f.exists());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn home_trash_points_under_the_data_dir() {
        // Resolution only — nothing is created or moved.
        if let Some(t) = TrashDir::home() {
            assert!(t.files.ends_with("Trash/files"));
            assert!(t.info.ends_with("Trash/info"));
        }
    }
}
