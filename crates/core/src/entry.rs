//! Directory entries, snapshots and sorting.
//!
//! Names and types arrive first; metadata is deferred (DESIGN.md §3).
//! Sorting is natural/locale-aware-ish: `file10` sorts after `file9`.

use crate::location::LocationKey;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Dir,
    File,
    Symlink,
    /// FIFO, socket, block/char device. Never copied blindly.
    Special,
    Unknown,
}

impl EntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::Dir => "dir",
            EntryKind::File => "file",
            EntryKind::Symlink => "symlink",
            EntryKind::Special => "special",
            EntryKind::Unknown => "unknown",
        }
    }
}

/// Stable identity of an entry within a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntryId(pub u64);

/// Metadata that arrives after the name/type pass.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Metadata {
    pub size: u64,
    pub mtime: i64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub dev: u64,
    pub ino: u64,
    /// For symlinks: does the target resolve?
    pub link_ok: Option<bool>,
    /// For symlinks: is the target a folder? (Picks the icon; the link
    /// itself is still never followed for anything else.)
    pub link_dir: bool,
    /// Is something mounted here (a disk, a bind mount, a network share)?
    /// Read without triggering an automount.
    pub mount_root: bool,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: EntryId,
    /// Raw OS bytes of the file name (not the full path).
    pub name: OsString,
    pub kind: EntryKind,
    /// `None` until the metadata pass delivers it.
    pub meta: Option<Metadata>,
}

impl Entry {
    pub fn new(name: OsString, kind: EntryKind) -> Self {
        let id = EntryId(fnv1a(name.as_bytes()));
        Entry {
            id,
            name,
            kind,
            meta: None,
        }
    }

    pub fn display_name(&self) -> String {
        crate::location::escape_for_display(&self.name)
    }

    pub fn is_hidden(&self) -> bool {
        self.name.as_bytes().first() == Some(&b'.')
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortField {
    Name,
    Size,
    Modified,
    Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortSpec {
    pub field: SortField,
    pub ascending: bool,
    /// Directories grouped before files regardless of field.
    pub dirs_first: bool,
}

impl Default for SortSpec {
    fn default() -> Self {
        SortSpec {
            field: SortField::Name,
            ascending: true,
            dirs_first: true,
        }
    }
}

/// How complete a snapshot is. A partial snapshot is still displayable.
#[derive(Debug, Clone, PartialEq)]
pub enum Completeness {
    /// Still listing; `seen` entries so far.
    Partial {
        seen: usize,
    },
    Complete,
    Failed {
        error: String,
    },
}

#[derive(Debug, Clone)]
pub struct DirectorySnapshot {
    pub key: LocationKey,
    /// Monotonic per pane. Results tagged with an older generation are dropped.
    pub generation: u64,
    pub entries: Vec<Entry>,
    pub sort: SortSpec,
    pub complete: Completeness,
    /// False until the metadata pass finished — the UI shows a sorting hint,
    /// so a provisional order is never mistaken for the settled order.
    pub settled: bool,
}

impl DirectorySnapshot {
    pub fn empty(key: LocationKey, generation: u64) -> Self {
        DirectorySnapshot {
            key,
            generation,
            entries: Vec::new(),
            sort: SortSpec::default(),
            complete: Completeness::Partial { seen: 0 },
            settled: false,
        }
    }

    /// Whether metadata is required before this sort can settle.
    pub fn sort_needs_metadata(&self) -> bool {
        matches!(self.sort.field, SortField::Size | SortField::Modified)
    }

    pub fn sort_entries(&mut self) {
        sort_entries(&mut self.entries, self.sort);
    }
}

/// Natural-order comparison of two raw names.
///
/// Digit runs compare numerically so `file9 < file10`. Comparison is
/// case-insensitive for ASCII, with a case-sensitive tiebreak so ordering is
/// total and stable. Operates on bytes: non-UTF-8 names never panic.
pub fn natural_cmp(a: &OsStr, b: &OsStr) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0usize, 0usize);

    while i < a.len() && j < b.len() {
        let (ca, cb) = (a[i], b[j]);
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            // Compare whole digit runs numerically, ignoring leading zeros.
            let si = i;
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            let sj = j;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let da = trim_zeros(&a[si..i]);
            let db = trim_zeros(&b[sj..j]);
            match da.len().cmp(&db.len()).then_with(|| da.cmp(db)) {
                Ordering::Equal => {}
                ord => return ord,
            }
            continue;
        }
        let la = ca.to_ascii_lowercase();
        let lb = cb.to_ascii_lowercase();
        match la.cmp(&lb) {
            Ordering::Equal => {
                i += 1;
                j += 1;
            }
            ord => return ord,
        }
    }
    // Prefix ordering, then a case-sensitive tiebreak for total ordering.
    (a.len() - i).cmp(&(b.len() - j)).then_with(|| a.cmp(b))
}

fn trim_zeros(s: &[u8]) -> &[u8] {
    let mut k = 0;
    while k + 1 < s.len() && s[k] == b'0' {
        k += 1;
    }
    &s[k..]
}

pub fn sort_entries(entries: &mut [Entry], spec: SortSpec) {
    use std::cmp::Ordering;
    entries.sort_by(|x, y| {
        if spec.dirs_first {
            let dx = x.kind == EntryKind::Dir;
            let dy = y.kind == EntryKind::Dir;
            if dx != dy {
                return if dx {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }
        }
        let ord = match spec.field {
            SortField::Name => natural_cmp(&x.name, &y.name),
            SortField::Size => {
                let sx = x.meta.map(|m| m.size).unwrap_or(0);
                let sy = y.meta.map(|m| m.size).unwrap_or(0);
                sx.cmp(&sy).then_with(|| natural_cmp(&x.name, &y.name))
            }
            SortField::Modified => {
                let tx = x.meta.map(|m| m.mtime).unwrap_or(0);
                let ty = y.meta.map(|m| m.mtime).unwrap_or(0);
                tx.cmp(&ty).then_with(|| natural_cmp(&x.name, &y.name))
            }
            SortField::Kind => {
                let ex = extension_of(&x.name);
                let ey = extension_of(&y.name);
                natural_cmp(ex, ey).then_with(|| natural_cmp(&x.name, &y.name))
            }
        };
        if spec.ascending {
            ord
        } else {
            ord.reverse()
        }
    });
}

/// What the Kind column shows: "folder", "link", the extension in lower
/// case ("pdf"), or "" for a file without one. Sort by kind orders by the
/// same extension (Flea #190), so the column and the order agree.
pub fn kind_text(e: &Entry) -> String {
    match e.kind {
        EntryKind::Dir => "folder".into(),
        EntryKind::Symlink => "link".into(),
        _ => {
            let ext = extension_of(&e.name).to_string_lossy().to_lowercase();
            // A blank cell reads as a rendering glitch; say what it is.
            if ext.is_empty() {
                "file".into()
            } else {
                ext
            }
        }
    }
}

fn extension_of(name: &OsStr) -> &OsStr {
    let b = name.as_bytes();
    // A leading dot is a hidden-file marker, not an extension separator.
    match b.iter().rposition(|c| *c == b'.') {
        Some(p) if p > 0 => OsStr::from_bytes(&b[p + 1..]),
        _ => OsStr::from_bytes(b""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    fn e(name: &str, kind: EntryKind) -> Entry {
        Entry::new(OsString::from(name), kind)
    }

    #[test]
    fn natural_order_puts_file9_before_file10() {
        let mut v = vec![
            e("file10.txt", EntryKind::File),
            e("file9.txt", EntryKind::File),
            e("file1.txt", EntryKind::File),
        ];
        sort_entries(&mut v, SortSpec::default());
        let names: Vec<_> = v.iter().map(|x| x.display_name()).collect();
        assert_eq!(names, vec!["file1.txt", "file9.txt", "file10.txt"]);
    }

    #[test]
    fn natural_order_handles_leading_zeros() {
        let mut v = vec![
            e("track007", EntryKind::File),
            e("track8", EntryKind::File),
            e("track06", EntryKind::File),
        ];
        sort_entries(&mut v, SortSpec::default());
        let names: Vec<_> = v.iter().map(|x| x.display_name()).collect();
        assert_eq!(names, vec!["track06", "track007", "track8"]);
    }

    #[test]
    fn dirs_group_before_files() {
        let mut v = vec![e("zzz-dir", EntryKind::Dir), e("aaa-file", EntryKind::File)];
        sort_entries(&mut v, SortSpec::default());
        assert_eq!(v[0].display_name(), "zzz-dir");
    }

    #[test]
    fn sorting_never_panics_on_non_utf8() {
        let mut v = vec![
            Entry::new(OsString::from_vec(b"\xff\xfe9".to_vec()), EntryKind::File),
            Entry::new(OsString::from_vec(b"\xff\xfe10".to_vec()), EntryKind::File),
        ];
        sort_entries(&mut v, SortSpec::default());
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn size_sort_is_marked_as_needing_metadata() {
        let mut s = DirectorySnapshot::empty(LocationKey::local("/tmp"), 1);
        s.sort.field = SortField::Size;
        assert!(s.sort_needs_metadata());
        s.sort.field = SortField::Name;
        assert!(!s.sort_needs_metadata());
    }

    #[test]
    fn fresh_snapshot_is_not_settled() {
        let s = DirectorySnapshot::empty(LocationKey::local("/tmp"), 1);
        assert!(!s.settled, "provisional order must be distinguishable");
    }

    #[test]
    fn hidden_detection_uses_bytes() {
        assert!(e(".config", EntryKind::Dir).is_hidden());
        assert!(!e("config", EntryKind::Dir).is_hidden());
    }

    #[test]
    fn dotfile_has_no_extension() {
        assert_eq!(extension_of(OsStr::new(".bashrc")), OsStr::new(""));
        assert_eq!(extension_of(OsStr::new("a.tar.zst")), OsStr::new("zst"));
    }
}
