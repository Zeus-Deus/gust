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

use crate::anchor::resolve_parent;
use gust_core::location::Cap;
use std::io;
use std::os::unix::ffi::OsStrExt;
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
            // A volume trash (.Trash-$uid) may exist but is not implemented
            // yet: Unknown, never a silent permanent delete.
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
    std::fs::create_dir_all(&trash.files)?;
    std::fs::create_dir_all(&trash.info)?;

    let base = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;

    // Find a free slot. Both the data and info name must be free, so two files
    // with the same name never collide.
    let mut name = base.to_os_string();
    let mut n = 1;
    loop {
        let info_candidate = trash
            .info
            .join(format!("{}.trashinfo", name.to_string_lossy()));
        if !trash.files.join(&name).exists() && !info_candidate.exists() {
            break;
        }
        let mut cand = base.to_os_string();
        cand.push(format!(".{n}"));
        name = cand;
        n += 1;
        if n > 10_000 {
            return Err(io::Error::other("cannot find a free trash name"));
        }
    }

    // Info file BEFORE the move: a crash must not leave an unrestorable file.
    let info_path = trash
        .info
        .join(format!("{}.trashinfo", name.to_string_lossy()));
    let info = format!(
        "[Trash Info]\nPath={}\nDeletionDate={}\n",
        percent_encode(path.as_os_str().as_bytes()),
        iso_now()
    );
    std::fs::write(&info_path, info)?;

    // Anchored rename into the trash.
    let (src_dir, src_name) = resolve_parent(path)?;
    let (trash_dir, _) = resolve_parent(&trash.files.join("placeholder"))?;
    match rustix::fs::renameat(src_dir.as_fd(), &src_name, trash_dir.as_fd(), &name) {
        Ok(()) => Ok(TrashedItem {
            original: path.to_path_buf(),
            trashed_name: name,
            info_file: info_path,
        }),
        Err(e) => {
            // Roll back the info file so the trash stays consistent.
            let _ = std::fs::remove_file(&info_path);
            Err(e.into())
        }
    }
}

/// Restore a trashed item to its original location.
///
/// Refuses if something now occupies the original path — restoring must never
/// overwrite work created after the delete.
pub fn restore(item: &TrashedItem, trash: &TrashDir) -> io::Result<()> {
    let from = trash.files.join(&item.trashed_name);
    if item.original.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "original path is occupied; restore would overwrite",
        ));
    }
    if let Some(parent) = item.original.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let (fd_from, n_from) = resolve_parent(&from)?;
    let (fd_to, n_to) = resolve_parent(&item.original)?;
    rustix::fs::renameat(fd_from.as_fd(), &n_from, fd_to.as_fd(), &n_to)?;
    let _ = std::fs::remove_file(&item.info_file);
    Ok(())
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

fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Read one `.trashinfo` record. None for anything malformed or relative:
/// a record must name an absolute path, or restoring it could write
/// somewhere relative to wherever Gust happens to run.
fn read_info(path: &Path) -> Option<(PathBuf, String)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut orig = None;
    let mut date = String::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("Path=") {
            orig = Some(PathBuf::from(
                <std::ffi::OsString as std::os::unix::ffi::OsStringExt>::from_vec(percent_decode(
                    v.trim(),
                )),
            ));
        } else if let Some(v) = line.strip_prefix("DeletionDate=") {
            date = v.trim().to_string();
        }
    }
    let orig = orig.filter(|p| p.is_absolute())?;
    Some((orig, date))
}

/// Everything in the Trash that can be restored: a record with its data.
/// Newest first. Entries other file managers wrote are included.
pub fn list(trash: &TrashDir) -> Vec<TrashEntry> {
    let Ok(rd) = std::fs::read_dir(&trash.info) else {
        return Vec::new();
    };
    let mut out: Vec<TrashEntry> = rd
        .flatten()
        .filter_map(|e| {
            let fname = e.file_name();
            let stem = fname.as_bytes().strip_suffix(b".trashinfo")?;
            let name = std::ffi::OsStr::from_bytes(stem).to_os_string();
            // The data must be there (lstat: a trashed symlink counts).
            std::fs::symlink_metadata(trash.files.join(&name)).ok()?;
            let (original, deleted) = read_info(&e.path())?;
            Some(TrashEntry {
                trashed_name: name,
                original,
                deleted,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.deleted
            .cmp(&a.deleted)
            .then_with(|| a.trashed_name.cmp(&b.trashed_name))
    });
    out
}

/// Restore one Trash item by its name in the Trash. Refuses to overwrite
/// anything now at the original path; recreates missing parent folders.
pub fn restore_named(trash: &TrashDir, trashed_name: &std::ffi::OsStr) -> io::Result<()> {
    // A name, never a path: nothing outside files/ can be addressed.
    if Path::new(trashed_name).components().count() != 1 || trashed_name.as_bytes().contains(&b'/')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a Trash item name",
        ));
    }
    let mut info_name = trashed_name.to_os_string();
    info_name.push(".trashinfo");
    let info_file = trash.info.join(&info_name);
    let (original, _) = read_info(&info_file).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "the Trash record is missing or damaged",
        )
    })?;
    let item = TrashedItem {
        original,
        trashed_name: trashed_name.to_os_string(),
        info_file,
    };
    restore(&item, trash)
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
