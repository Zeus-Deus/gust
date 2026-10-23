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
    /// Colour tags (DESIGN §6.25), as bits: bit 0 = Red … bit 6 = Gray.
    pub tags: u8,
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

/// The same two-stage order as `natural_cmp`, prepared once per name.
/// Numeric tokens start at ASCII `0` (all digits have the same order relative
/// to non-digits), followed by a big-endian significant length and digits.
/// Equal numeric tokens ignore leading zeros; the original bytes break ties.
#[derive(Eq, PartialEq)]
struct NaturalKey {
    bytes: Vec<u8>,
    original: usize,
}

impl NaturalKey {
    fn encoded_len(name: &OsStr) -> usize {
        let name = name.as_bytes();
        let mut len = name.len(); // Original bytes used to break ties.
        let mut i = 0;
        while i < name.len() {
            if name[i].is_ascii_digit() {
                let start = i;
                while i < name.len() && name[i].is_ascii_digit() {
                    i += 1;
                }
                len = len
                    .saturating_add(1 + std::mem::size_of::<usize>())
                    .saturating_add(trim_zeros(&name[start..i]).len());
            } else {
                len = len.saturating_add(1);
                i += 1;
            }
        }
        len
    }

    fn new(name: &OsStr) -> Self {
        let capacity = Self::encoded_len(name);
        let name = name.as_bytes();
        let mut bytes = Vec::with_capacity(capacity);
        let mut i = 0;
        while i < name.len() {
            if name[i].is_ascii_digit() {
                let start = i;
                while i < name.len() && name[i].is_ascii_digit() {
                    i += 1;
                }
                let digits = trim_zeros(&name[start..i]);
                bytes.push(b'0');
                bytes.extend_from_slice(&digits.len().to_be_bytes());
                bytes.extend_from_slice(digits);
            } else {
                bytes.push(name[i].to_ascii_lowercase());
                i += 1;
            }
        }
        let original = bytes.len();
        bytes.extend_from_slice(name);
        Self { bytes, original }
    }
}

impl Ord for NaturalKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.bytes[..self.original]
            .cmp(&other.bytes[..other.original])
            .then_with(|| self.bytes[self.original..].cmp(&other.bytes[other.original..]))
    }
}

impl PartialOrd for NaturalKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

const NATURAL_SORT_CACHE_BUDGET: usize = 16 * 1024 * 1024;

fn cached_natural_sort_fits(entries: &[Entry]) -> bool {
    // sort_by_cached_key stores (key, original index). usize conservatively
    // covers its smaller index types too; tuple padding is included.
    let record_size = std::mem::size_of::<((bool, NaturalKey), usize)>();
    let Some(remaining) = entries
        .len()
        .checked_mul(record_size)
        .and_then(|records| NATURAL_SORT_CACHE_BUDGET.checked_sub(records))
    else {
        return false;
    };
    // Short names can fit even with a digit run at every other byte. Prove
    // that cheap upper bound first, avoiding a second byte scan in that case.
    if entries
        .iter()
        .try_fold(remaining, |remaining, entry| {
            let len = entry.name.len();
            let upper_bound = len.saturating_mul(2).saturating_add(
                len.div_ceil(2)
                    .saturating_mul(1 + std::mem::size_of::<usize>()),
            );
            remaining.checked_sub(upper_bound)
        })
        .is_some()
    {
        return true;
    }
    entries
        .iter()
        .try_fold(remaining, |remaining, entry| {
            remaining.checked_sub(NaturalKey::encoded_len(&entry.name))
        })
        .is_some()
}

pub fn sort_entries(entries: &mut [Entry], spec: SortSpec) {
    use std::cmp::Ordering;
    let compare = |x: &Entry, y: &Entry| {
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
    };
    if spec.field == SortField::Name && entries.len() >= 1000 {
        // Metadata fills commonly revisit an already sorted snapshot. Avoid
        // allocating keys in that case, while still accepting unsorted input.
        if entries
            .windows(2)
            .all(|pair| compare(&pair[0], &pair[1]) != Ordering::Greater)
        {
            return;
        }
        // Reverse runs need no keys. Equal rows must keep their input order,
        // so only a strictly reversed run can take this path.
        if entries
            .windows(2)
            .all(|pair| compare(&pair[0], &pair[1]) == Ordering::Greater)
        {
            entries.reverse();
            return;
        }
        // Bound requested scratch bytes before allocating any expanded keys.
        // Digit-heavy or very large listings keep the stable comparator path.
        if !cached_natural_sort_fits(entries) {
            entries.sort_by(compare);
            return;
        }
        // Cache numeric parsing and sort compact keys instead of repeatedly
        // scanning names while moving the much larger metadata-bearing rows.
        if spec.ascending {
            entries.sort_by_cached_key(|e| {
                (
                    spec.dirs_first && e.kind != EntryKind::Dir,
                    NaturalKey::new(&e.name),
                )
            });
        } else {
            entries.sort_by_cached_key(|e| {
                (
                    spec.dirs_first && e.kind != EntryKind::Dir,
                    std::cmp::Reverse(NaturalKey::new(&e.name)),
                )
            });
        }
    } else {
        entries.sort_by(compare);
    }
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

    #[test]
    fn cached_natural_keys_match_raw_comparison() {
        let mut names = vec![
            OsString::new(),
            OsString::from("A00a"),
            OsString::from("a0A"),
            OsString::from("a00"),
            OsString::from("a0000a"),
            OsString::from_vec(vec![b'0'; 255]),
            OsString::from_vec(vec![b'9'; 255]),
        ];
        for number in [0, 1, 9, 10, 99, 100, u64::MAX] {
            for zeros in [0, 1, 8] {
                for prefix in ["", "file", "FILE", "."] {
                    for suffix in ["", "a", ".txt", "00", "\u{80}"] {
                        names.push(OsString::from(format!(
                            "{prefix}{}{number}{suffix}",
                            "0".repeat(zeros)
                        )));
                    }
                }
            }
        }
        // Include every byte (even NUL, although the filesystem disallows it)
        // and digit/case boundaries in names of different lengths.
        let alphabet = b"00129aAZ./\x00\x7f\x80\xff";
        let mut seed = 0x1428b02d_u64;
        for byte in 0..=255 {
            names.push(OsString::from_vec(vec![byte]));
        }
        for length in 0..256 {
            let bytes = (0..length)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    alphabet[seed as usize % alphabet.len()]
                })
                .collect();
            names.push(OsString::from_vec(bytes));
        }
        let keys: Vec<_> = names.iter().map(|n| NaturalKey::new(n)).collect();
        for (name, key) in names.iter().zip(&keys) {
            assert_eq!(key.bytes.len(), NaturalKey::encoded_len(name));
            assert_eq!(key.bytes.capacity(), key.bytes.len());
        }
        for (i, a) in names.iter().enumerate() {
            for (j, b) in names.iter().enumerate() {
                assert_eq!(keys[i].cmp(&keys[j]), natural_cmp(a, b), "{a:?} / {b:?}");
            }
        }
    }

    #[test]
    fn large_name_sort_preserves_grouping_direction_and_equal_rows() {
        let entries: Vec<_> = (0..1300)
            .map(|i| {
                let mut name = format!("file{:03}", (i * 37) % 257).into_bytes();
                if i % 7 == 0 {
                    name.push(0xff);
                }
                let mut entry = Entry::new(
                    OsString::from_vec(name),
                    if i % 3 == 0 {
                        EntryKind::Dir
                    } else {
                        EntryKind::File
                    },
                );
                // Equal names can represent distinct rows. Their original
                // order must survive the cached stable sort and its fast path.
                entry.id = EntryId(i as u64);
                entry
            })
            .collect();
        assert!(cached_natural_sort_fits(&entries));
        assert_name_sort_matches_comparator(&entries);
    }

    #[test]
    fn strictly_reversed_100k_names_match_comparator() {
        let entries: Vec<_> = (0..100_000)
            .rev()
            .map(|i| {
                let mut entry = e(&format!("entry-{i:06}.dat"), EntryKind::File);
                entry.id = EntryId(i as u64);
                entry
            })
            .collect();
        assert_eq!(entries[0].name.as_bytes(), b"entry-099999.dat");
        assert_eq!(entries.last().unwrap().name.as_bytes(), b"entry-000000.dat");
        assert!(cached_natural_sort_fits(&entries));
        assert_name_sort_matches_comparator(&entries);
    }

    #[test]
    fn reverse_name_sort_respects_direction_and_directories() {
        for ascending in [true, false] {
            let mut entries = Vec::new();
            // With directories first, files must lead the reversed run for
            // both name directions. Include non-UTF-8 raw names as well.
            for kind in [EntryKind::File, EntryKind::Dir] {
                for i in 0..700 {
                    let number = if ascending { 699 - i } else { i };
                    let mut name = format!("entry-{number:06}.dat").into_bytes();
                    name.push(0xff);
                    let mut entry = Entry::new(OsString::from_vec(name), kind);
                    entry.id = EntryId(entries.len() as u64);
                    entries.push(entry);
                }
            }
            assert_name_sort_matches_comparator(&entries);
        }
    }

    #[test]
    fn reverse_name_sort_keeps_equal_rows_stable() {
        let mut entries: Vec<_> = (0..1300)
            .rev()
            .map(|i| {
                let mut entry = e(&format!("entry-{i:06}.dat"), EntryKind::File);
                entry.id = EntryId(i as u64);
                entry
            })
            .collect();
        // A late equal pair must reject reversal even after a long strict run.
        let mut equal = entries[1200].clone();
        equal.id = EntryId(1300);
        entries.insert(1201, equal);
        assert_name_sort_matches_comparator(&entries);
    }

    #[test]
    fn digit_heavy_100k_names_fall_back_without_changing_order() {
        // Valid 236-byte names with 116 numeric runs previously used almost
        // 200 MiB of sort scratch through expansion and repeated reallocations.
        let suffix = "a1".repeat(115);
        let entries: Vec<_> = (0..100_000)
            .map(|i| {
                let mut entry = e(
                    &format!("{:05}-{suffix}", (i * 7919) % 50_000),
                    if i % 3 == 0 {
                        EntryKind::Dir
                    } else {
                        EntryKind::File
                    },
                );
                entry.id = EntryId(i as u64);
                entry
            })
            .collect();
        assert_eq!(entries[0].name.len(), 236);
        assert!(!cached_natural_sort_fits(&entries));
        assert_name_sort_matches_comparator(&entries);
    }

    #[test]
    fn normal_100k_names_still_use_cached_sort() {
        let entries: Vec<_> = (0..100_000)
            .map(|i| {
                let mut entry = e(
                    &format!("file{:05}.txt", (i * 7919) % 50_000),
                    EntryKind::File,
                );
                entry.id = EntryId(i as u64);
                entry
            })
            .collect();
        assert!(cached_natural_sort_fits(&entries));
        assert_name_sort_matches_comparator(&entries);
    }

    #[test]
    fn cached_sort_budget_includes_record_overhead() {
        let entries = vec![e(&"a".repeat(60), EntryKind::File); 100_000];
        let key_bytes = entries.len() * NaturalKey::encoded_len(&entries[0].name);
        assert!(key_bytes < NATURAL_SORT_CACHE_BUDGET);
        assert!(!cached_natural_sort_fits(&entries));
    }

    fn assert_name_sort_matches_comparator(entries: &[Entry]) {
        for ascending in [true, false] {
            for dirs_first in [true, false] {
                let spec = SortSpec {
                    field: SortField::Name,
                    ascending,
                    dirs_first,
                };
                let mut expected = entries.to_vec();
                expected.sort_by(|a, b| {
                    if dirs_first && (a.kind == EntryKind::Dir) != (b.kind == EntryKind::Dir) {
                        return (b.kind == EntryKind::Dir).cmp(&(a.kind == EntryKind::Dir));
                    }
                    let order = natural_cmp(&a.name, &b.name);
                    if ascending {
                        order
                    } else {
                        order.reverse()
                    }
                });
                let expected_ids: Vec<_> = expected.iter().map(|e| e.id).collect();
                let mut actual = entries.to_vec();
                sort_entries(&mut actual, spec);
                assert_eq!(
                    actual.iter().map(|e| e.id).collect::<Vec<_>>(),
                    expected_ids
                );
                sort_entries(&mut actual, spec);
                assert_eq!(
                    actual.iter().map(|e| e.id).collect::<Vec<_>>(),
                    expected_ids
                );
            }
        }
    }
}
