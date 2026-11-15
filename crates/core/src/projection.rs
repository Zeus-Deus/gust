//! Cancellable pure-CPU preparation over immutable listing storage.
//!
//! These functions borrow entries and move only indices. Callers own listing
//! revisions and publication; indices never establish filesystem identity.

use crate::entry::{compare_entries, Entry, EntryKind, SortField, SortSpec};
use crate::lister::CancelToken;
use crate::search::PreparedQuery;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;

/// A worker checkpoint. `true` means abandon the result. Closures permit
/// deterministic cancellation/observation probes without OS timing races.
pub trait Cancellation {
    fn checkpoint(&self) -> bool;
}

impl Cancellation for CancelToken {
    fn checkpoint(&self) -> bool {
        CancelToken::checkpoint(self)
    }
}

impl<F: Fn() -> bool> Cancellation for F {
    fn checkpoint(&self) -> bool {
        self()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionError {
    Cancelled,
    ScratchOccupied,
    InvalidIndex { index: usize, len: usize },
    TooManyRows { len: usize },
}

// A work bound, not an OS deadline: allocation, a filename comparison/match,
// and a hash-table insertion can still take variable time.
const CHUNK: usize = 256;

pub(crate) fn checkpoint(cancel: &impl Cancellation) -> Result<(), ProjectionError> {
    if cancel.checkpoint() {
        Err(ProjectionError::Cancelled)
    } else {
        Ok(())
    }
}

pub(crate) fn check_row(row: usize, cancel: &impl Cancellation) -> Result<(), ProjectionError> {
    if row.is_multiple_of(CHUNK) {
        checkpoint(cancel)?;
    }
    Ok(())
}

pub(crate) fn entry_at(entries: &[Entry], index: usize) -> Result<&Entry, ProjectionError> {
    entries.get(index).ok_or(ProjectionError::InvalidIndex {
        index,
        len: entries.len(),
    })
}

/// Stable full permutation, with exactly the same comparison as
/// `entry::sort_entries`. Equal entries retain backing order. No entry or
/// filename is cloned. Sort runs contain at most 256 rows; merges and
/// index preparation checkpoint at most every 256 rows. Cancellation never
/// returns a partial permutation or mutates the backing storage.
// Preserve caller specialization as optional preparation grows the dispatcher.
#[inline(always)]
pub fn sort_indices(
    entries: &[Entry],
    spec: SortSpec,
    cancel: &impl Cancellation,
) -> Result<Vec<usize>, ProjectionError> {
    checkpoint(cancel)?;
    // Inspect order before allocating merge scratch. Stop at the first
    // disorder, so an arbitrary listing adds only a short prefix scan.
    let mut ordered = true;
    for (row, pair) in entries.windows(2).enumerate() {
        check_row(row, cancel)?;
        if compare_entries(&pair[0], &pair[1], spec).is_gt() {
            ordered = false;
            break;
        }
    }
    if !ordered
        && spec.field == SortField::Name
        && name_prefixes_distinguish(entries, spec, cancel)?
    {
        return sort_name_indices(entries, spec, cancel);
    }
    if !ordered
        && matches!(spec.field, SortField::Size | SortField::Modified)
        && u32::try_from(entries.len()).is_ok()
        && scalar_prefixes_help(entries, spec, cancel)?
    {
        return sort_scalar_indices(entries, spec, cancel);
    }
    let mut order = Vec::with_capacity(entries.len());
    for index in 0..entries.len() {
        check_row(index, cancel)?;
        order.push(index);
    }
    if ordered {
        checkpoint(cancel)?;
        return Ok(order);
    }
    for run in order.chunks_mut(CHUNK) {
        checkpoint(cancel)?;
        run.sort_by(|&a, &b| compare_entries(&entries[a], &entries[b], spec));
    }
    let mut scratch = Vec::with_capacity(order.len());
    let mut width = CHUNK;
    while width < order.len() {
        checkpoint(cancel)?;
        scratch.clear();
        for runs in order.chunks(width.saturating_mul(2)) {
            let mid = width.min(runs.len());
            let (mut a, mut b) = (0, mid);
            while a < mid || b < runs.len() {
                check_row(scratch.len(), cancel)?;
                // Choose the left row for ties, preserving stable order.
                let take_left = b == runs.len()
                    || (a < mid
                        && !compare_entries(&entries[runs[a]], &entries[runs[b]], spec).is_gt());
                scratch.push(if take_left {
                    let index = runs[a];
                    a += 1;
                    index
                } else {
                    let index = runs[b];
                    b += 1;
                    index
                });
            }
        }
        std::mem::swap(&mut order, &mut scratch);
        width = width.saturating_mul(2);
    }
    checkpoint(cancel)?;
    Ok(order)
}

// Normalized prefixes keep the common Name comparisons in compact contiguous
// records instead of randomly fetching metadata-bearing entries. Equal or
// truncated prefixes always defer to the shared exact comparator. The two
// record buffers request 48 bytes/entry on 64-bit hosts; no per-name allocation.
#[derive(Clone, Copy)]
struct NameIndex {
    prefix: [u64; 2],
    index: usize,
}

// A narrower backing index leaves twelve bytes for the same normalized name
// prefix in a 24-byte record. Listings beyond u32 keep the existing usize path.
// The group byte is compared before the scalar, independently of direction.
#[derive(Clone, Copy)]
struct ScalarIndex {
    scalar: u64,
    prefix: u64,
    tail: u32,
    index: u32,
}

fn scalar_index(
    entry: &Entry,
    index: u32,
    spec: SortSpec,
    cancel: &impl Cancellation,
) -> Result<ScalarIndex, ProjectionError> {
    let scalar = match spec.field {
        SortField::Size => entry.meta.map_or(0, |meta| meta.size),
        // Flipping the sign bit maps signed timestamp order to unsigned order.
        SortField::Modified => (entry.meta.map_or(0, |meta| meta.mtime) as u64) ^ (1 << 63),
        _ => unreachable!("scalar preparation is only admitted for Size/Modified"),
    };
    let name = name_prefix(entry, spec, cancel)?;
    Ok(ScalarIndex {
        scalar: if spec.ascending { scalar } else { !scalar },
        prefix: name[0],
        tail: (name[1] >> 32) as u32,
        index,
    })
}

fn compare_scalar_indices(
    a: &ScalarIndex,
    b: &ScalarIndex,
    entries: &[Entry],
    spec: SortSpec,
) -> std::cmp::Ordering {
    (a.prefix >> 56)
        .cmp(&(b.prefix >> 56))
        .then_with(|| a.scalar.cmp(&b.scalar))
        .then_with(|| a.prefix.cmp(&b.prefix))
        .then_with(|| a.tail.cmp(&b.tail))
        // Equal prefixes include truncation, saturation and raw-byte ties.
        .then_with(|| compare_entries(&entries[a.index as usize], &entries[b.index as usize], spec))
}

// Preparing names and moving larger records pays for repeated scalar ties
// with shared natural-name stems. A bounded stack sample keeps cheap scalar
// comparisons, names distinguished near their start, and colliding long stems
// on the existing index path. This conservative heuristic can miss a gain; it
// never participates in ordering and retains no storage between requests.
fn scalar_prefixes_help(
    entries: &[Entry],
    spec: SortSpec,
    cancel: &impl Cancellation,
) -> Result<bool, ProjectionError> {
    checkpoint(cancel)?;
    let count = entries.len().min(64);
    if count < 3 {
        return Ok(false);
    }
    let mut sample = [ScalarIndex {
        scalar: 0,
        prefix: 0,
        tail: 0,
        index: 0,
    }; 64];
    for (row, key) in sample[..count].iter_mut().enumerate() {
        check_row(row, cancel)?;
        let index = row * (entries.len() / count) + row.min(entries.len() % count);
        *key = scalar_index(&entries[index], index as u32, spec, cancel)?;
    }
    let sample = &mut sample[..count];
    checkpoint(cancel)?;
    sample.sort_unstable_by_key(|key| key.scalar);
    let primaries = 1 + sample
        .windows(2)
        .filter(|pair| pair[0].scalar != pair[1].scalar)
        .count();
    if primaries > count / 2 {
        return Ok(false);
    }
    // Ignore the directory-group byte when inspecting name work. At least
    // half the sample must share the first two normalized name bytes.
    let head = |key: &ScalarIndex| (key.prefix >> 40) & 0xffff;
    checkpoint(cancel)?;
    sample.sort_unstable_by_key(head);
    let (mut run, mut longest) = (1, 1);
    for pair in sample.windows(2) {
        run = if head(&pair[0]) == head(&pair[1]) {
            run + 1
        } else {
            1
        };
        longest = longest.max(run);
    }
    if longest < count.div_ceil(2) {
        return Ok(false);
    }
    checkpoint(cancel)?;
    let name = |key: &ScalarIndex| (key.prefix & 0x00ff_ffff_ffff_ffff, key.tail);
    sample.sort_unstable_by_key(name);
    let distinct = 1 + sample
        .windows(2)
        .filter(|pair| name(&pair[0]) != name(&pair[1]))
        .count();
    checkpoint(cancel)?;
    Ok(distinct > 2)
}

fn sort_scalar_indices(
    entries: &[Entry],
    spec: SortSpec,
    cancel: &impl Cancellation,
) -> Result<Vec<usize>, ProjectionError> {
    let mut rows = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        check_row(index, cancel)?;
        rows.push(scalar_index(entry, index as u32, spec, cancel)?);
    }
    let compare = |a: &ScalarIndex, b: &ScalarIndex| compare_scalar_indices(a, b, entries, spec);
    for run in rows.chunks_mut(CHUNK) {
        checkpoint(cancel)?;
        // Stable bounded runs retain existing partial order and exact ties.
        run.sort_by(compare);
    }
    let rows = merge_runs(rows, compare, cancel)?;
    // The other record buffer is gone before the 8-byte output is allocated:
    // 48 bytes/row during merges, 32 during conversion on a 64-bit host.
    let mut order = Vec::with_capacity(rows.len());
    for row in rows {
        check_row(order.len(), cancel)?;
        order.push(row.index as usize);
    }
    checkpoint(cancel)?;
    Ok(order)
}

// A common long stem can make every prepared prefix collide. Avoid the
// larger buffers and comparator overhead when the bounded sample conveys at
// most two distinct keys (commonly just the directory/file grouping).
fn name_prefixes_distinguish(
    entries: &[Entry],
    spec: SortSpec,
    cancel: &impl Cancellation,
) -> Result<bool, ProjectionError> {
    checkpoint(cancel)?;
    let count = entries.len().min(64);
    if count == 0 {
        return Ok(false);
    }
    let mut sample = [[0u64; 2]; 64];
    for (row, key) in sample[..count].iter_mut().enumerate() {
        let index = row * (entries.len() / count) + row.min(entries.len() % count);
        *key = name_prefix(&entries[index], spec, cancel)?;
    }
    checkpoint(cancel)?;
    sample[..count].sort_unstable();
    checkpoint(cancel)?;
    let distinct = 1 + sample[..count]
        .windows(2)
        .filter(|pair| pair[0] != pair[1])
        .count();
    Ok(distinct > 2)
}

fn sort_name_indices(
    entries: &[Entry],
    spec: SortSpec,
    cancel: &impl Cancellation,
) -> Result<Vec<usize>, ProjectionError> {
    let mut rows = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        check_row(index, cancel)?;
        rows.push(NameIndex {
            prefix: name_prefix(entry, spec, cancel)?,
            index,
        });
    }
    let compare = |a: &NameIndex, b: &NameIndex| {
        a.prefix
            .cmp(&b.prefix)
            .then_with(|| compare_entries(&entries[a.index], &entries[b.index], spec))
            // Makes bounded unstable run sorts globally stable for exact ties.
            .then_with(|| a.index.cmp(&b.index))
    };
    for run in rows.chunks_mut(CHUNK) {
        checkpoint(cancel)?;
        run.sort_unstable_by(compare);
    }
    let rows = merge_runs(rows, compare, cancel)?;
    // merge_runs released its other buffer before allocating the output, so
    // conversion peaks below the two-record-buffer phase. All buffers are flat.
    let mut order = Vec::with_capacity(rows.len());
    for row in rows {
        check_row(order.len(), cancel)?;
        order.push(row.index);
    }
    checkpoint(cancel)?;
    Ok(order)
}

fn name_prefix(
    entry: &Entry,
    spec: SortSpec,
    cancel: &impl Cancellation,
) -> Result<[u64; 2], ProjectionError> {
    let mut prefix = [0u8; 16];
    prefix[0] = u8::from(spec.dirs_first && entry.kind != EntryKind::Dir);
    let name = entry.name.as_bytes();
    let (mut source, mut target) = (0, 1);
    while source < name.len() && target < prefix.len() {
        if name[source].is_ascii_digit() {
            let start = source;
            while source < name.len() && name[source].is_ascii_digit() {
                if source > 0 {
                    check_row(source, cancel)?;
                }
                source += 1;
            }
            let mut significant = start;
            while significant + 1 < source && name[significant] == b'0' {
                if significant > 0 {
                    check_row(significant, cancel)?;
                }
                significant += 1;
            }
            prefix[target] = b'0';
            target += 1;
            if target == prefix.len() {
                break;
            }
            let digits = source - significant;
            prefix[target] = digits.min(255) as u8;
            target += 1;
            // Saturation must end the prefix. Otherwise the digits of two
            // long runs could disagree before their exact lengths are compared.
            if digits >= 255 {
                break;
            }
            for &byte in name[significant..source].iter().take(prefix.len() - target) {
                prefix[target] = byte;
                target += 1;
            }
        } else {
            prefix[target] = name[source].to_ascii_lowercase();
            target += 1;
            source += 1;
        }
    }
    if !spec.ascending {
        for byte in &mut prefix[1..] {
            *byte = !*byte;
        }
    }
    Ok(std::array::from_fn(|word| {
        u64::from_be_bytes(std::array::from_fn(|byte| prefix[word * 8 + byte]))
    }))
}

fn merge_runs<T: Copy>(
    mut order: Vec<T>,
    compare: impl Fn(&T, &T) -> std::cmp::Ordering,
    cancel: &impl Cancellation,
) -> Result<Vec<T>, ProjectionError> {
    let mut scratch = Vec::with_capacity(order.len());
    let mut width = CHUNK;
    while width < order.len() {
        checkpoint(cancel)?;
        scratch.clear();
        for runs in order.chunks(width.saturating_mul(2)) {
            let mid = width.min(runs.len());
            let (mut a, mut b) = (0, mid);
            while a < mid || b < runs.len() {
                check_row(scratch.len(), cancel)?;
                let take_left =
                    b == runs.len() || (a < mid && !compare(&runs[a], &runs[b]).is_gt());
                scratch.push(if take_left {
                    let row = runs[a];
                    a += 1;
                    row
                } else {
                    let row = runs[b];
                    b += 1;
                    row
                });
            }
        }
        std::mem::swap(&mut order, &mut scratch);
        width = width.saturating_mul(2);
    }
    checkpoint(cancel)?;
    Ok(order)
}

/// Ordinary filter over an explicit full order (or visible subset). Prepare
/// the query once and reuse the full order for query/hidden-policy changes.
/// Input order and duplicate occurrences are preserved; all indices must be
/// in bounds. Ranked Find is independent of this API.
pub fn filter_indices(
    entries: &[Entry],
    order: &[usize],
    query: &PreparedQuery,
    show_hidden: bool,
    cancel: &impl Cancellation,
) -> Result<Vec<usize>, ProjectionError> {
    checkpoint(cancel)?;
    let mut visible = Vec::new();
    for (row, &index) in order.iter().enumerate() {
        check_row(row, cancel)?;
        let entry = entry_at(entries, index)?;
        if (show_hidden || !entry.is_hidden()) && query.fuzzy_matches(&entry.name) {
            visible.push(index);
        }
    }
    checkpoint(cancel)?;
    Ok(visible)
}

/// Map every old displayed row to its new displayed row, or -1 if absent.
/// Names are borrowed raw OS bytes, including across backing revisions.
/// New visible names must be unique (as directory listings are); if repeated,
/// the last occurrence wins. Every supplied index is checked. Both map
/// construction and lookup checkpoint in bounded row batches.
pub fn map_visible_rows(
    old_entries: &[Entry],
    old_visible: &[usize],
    new_entries: &[Entry],
    new_visible: &[usize],
    cancel: &impl Cancellation,
) -> Result<Vec<i32>, ProjectionError> {
    checkpoint(cancel)?;
    if new_visible.len() > i32::MAX as usize {
        return Err(ProjectionError::TooManyRows {
            len: new_visible.len(),
        });
    }
    let mut new_rows: HashMap<&OsStr, i32> = HashMap::with_capacity(new_visible.len());
    for (row, &index) in new_visible.iter().enumerate() {
        check_row(row, cancel)?;
        new_rows.insert(entry_at(new_entries, index)?.name.as_os_str(), row as i32);
    }
    let mut mapped = Vec::with_capacity(old_visible.len());
    for (row, &index) in old_visible.iter().enumerate() {
        check_row(row, cancel)?;
        let name = entry_at(old_entries, index)?.name.as_os_str();
        mapped.push(new_rows.get(name).copied().unwrap_or(-1));
    }
    checkpoint(cancel)?;
    Ok(mapped)
}

#[cfg(test)]
mod name_prefix_tests {
    use super::*;
    use std::cell::Cell;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn non_equal_prefixes_agree_with_shared_comparator_for_adversarial_bytes() {
        let mut names: Vec<Vec<u8>> = (0..=255).map(|byte| vec![byte]).collect();
        names.extend([
            b"".to_vec(),
            b"a\0".to_vec(),
            b"a\0z".to_vec(),
            b"a00B".to_vec(),
            b"A0b".to_vec(),
            "é🦀10".as_bytes().to_vec(),
        ]);
        for length in [1, 2, 9, 10, 254, 255, 256, 512] {
            for zeros in [0, 3, 300] {
                for stem in [
                    b"".as_slice(),
                    b"file",
                    b"a common stem exceeding sixteen bytes ",
                ] {
                    let mut name = stem.to_vec();
                    name.extend(vec![b'0'; zeros]);
                    name.extend(vec![b'9'; length]);
                    name.extend(b"\xff.txt");
                    names.push(name);
                }
            }
        }
        let alphabet = b"00129aAZ./\x00\x7f\x80\xff";
        let mut seed = 0x84af_1273_u64;
        for length in 0..160 {
            names.push(
                (0..length)
                    .map(|_| {
                        seed ^= seed << 13;
                        seed ^= seed >> 7;
                        seed ^= seed << 17;
                        alphabet[seed as usize % alphabet.len()]
                    })
                    .collect(),
            );
        }
        let entries: Vec<_> = names
            .into_iter()
            .enumerate()
            .map(|(i, name)| {
                Entry::new(
                    OsString::from_vec(name),
                    if i.is_multiple_of(3) {
                        EntryKind::Dir
                    } else {
                        EntryKind::File
                    },
                )
            })
            .collect();
        for ascending in [false, true] {
            for dirs_first in [false, true] {
                let spec = SortSpec {
                    field: SortField::Name,
                    ascending,
                    dirs_first,
                };
                let keys: Vec<_> = entries
                    .iter()
                    .map(|entry| name_prefix(entry, spec, &|| false).unwrap())
                    .collect();
                for (a, ea) in entries.iter().enumerate() {
                    for (b, eb) in entries.iter().enumerate() {
                        let prefix_order = keys[a].cmp(&keys[b]);
                        if !prefix_order.is_eq() {
                            assert_eq!(
                                prefix_order,
                                compare_entries(ea, eb, spec),
                                "{spec:?}: {:?} / {:?}",
                                ea.name,
                                eb.name
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn scalar_records_preserve_extremes_raw_names_and_stable_equality() {
        use crate::entry::{EntryId, Metadata};
        assert_eq!(std::mem::size_of::<ScalarIndex>(), 24);
        let mut names: Vec<Vec<u8>> = (0..=255).map(|b| vec![b]).collect();
        names.extend([
            b"".to_vec(),
            b"A00b".to_vec(),
            b"a0B".to_vec(),
            b"a\0".to_vec(),
            b"a\0z".to_vec(),
            b"same".to_vec(),
            b"same".to_vec(),
        ]);
        for digits in [254, 255, 256, 512] {
            for stem in [
                b"".as_slice(),
                b"a common filename stem exceeding twelve bytes ",
            ] {
                let mut name = stem.to_vec();
                name.extend(vec![b'9'; digits]);
                names.push(name);
            }
        }
        let entries: Vec<_> = names
            .into_iter()
            .enumerate()
            .flat_map(|(i, name)| {
                [
                    None,
                    Some((0, 0)),
                    Some((1, -1)),
                    Some((u64::MAX, i64::MIN)),
                    Some((u64::MAX - 1, i64::MAX)),
                ]
                .into_iter()
                .enumerate()
                .map(move |(m, meta)| {
                    let mut entry = Entry::new(
                        OsString::from_vec(name.clone()),
                        if (i + m).is_multiple_of(3) {
                            EntryKind::Dir
                        } else {
                            EntryKind::File
                        },
                    );
                    entry.id = EntryId((i * 5 + m) as u64);
                    entry.meta = meta.map(|(size, mtime)| Metadata {
                        size,
                        mtime,
                        ..Metadata::default()
                    });
                    entry
                })
            })
            .collect();
        for field in [SortField::Size, SortField::Modified] {
            for ascending in [false, true] {
                for dirs_first in [false, true] {
                    let spec = SortSpec {
                        field,
                        ascending,
                        dirs_first,
                    };
                    let keys: Vec<_> = entries
                        .iter()
                        .enumerate()
                        .map(|(i, e)| scalar_index(e, i as u32, spec, &|| false).unwrap())
                        .collect();
                    for (a, ea) in entries.iter().enumerate() {
                        for (b, eb) in entries.iter().enumerate() {
                            assert_eq!(
                                compare_scalar_indices(&keys[a], &keys[b], &entries, spec),
                                compare_entries(ea, eb, spec),
                                "{spec:?} {a} {b}"
                            );
                        }
                    }
                    let mut expected: Vec<_> = (0..entries.len()).collect();
                    expected.sort_by(|&a, &b| compare_entries(&entries[a], &entries[b], spec));
                    assert_eq!(
                        sort_scalar_indices(&entries, spec, &|| false).unwrap(),
                        expected
                    );
                    assert_eq!(sort_indices(&entries, spec, &|| false).unwrap(), expected);
                }
            }
        }
    }

    #[test]
    fn scalar_admission_avoids_cheap_or_colliding_keys_and_is_cancellable() {
        use crate::entry::Metadata;
        let make = |case: &str| -> Vec<Entry> {
            (0usize..4099)
                .map(|i| {
                    let name = match case {
                        "long" => format!("a common long stem beyond twelve bytes {i}"),
                        "head" => format!("{}-file{i}", char::from(b'!' + (i % 90) as u8)),
                        _ => format!("file{i}"),
                    };
                    let mut e = Entry::new(name.into(), EntryKind::File);
                    e.meta = Some(Metadata {
                        size: if case == "high" {
                            i as u64
                        } else {
                            (i % 17) as u64
                        },
                        mtime: if case == "high" {
                            i as i64
                        } else {
                            (i % 19) as i64 - 9
                        },
                        ..Metadata::default()
                    });
                    e
                })
                .collect()
        };
        for field in [SortField::Size, SortField::Modified] {
            let spec = SortSpec {
                field,
                ..SortSpec::default()
            };
            for (case, expected) in [
                ("low", true),
                ("high", false),
                ("long", false),
                ("head", false),
            ] {
                let entries = make(case);
                assert_eq!(
                    scalar_prefixes_help(&entries, spec, &|| false).unwrap(),
                    expected,
                    "{case}"
                );
            }
            let entries = make("low");
            for budget in 1..=6 {
                let count = Cell::new(0);
                assert_eq!(
                    scalar_prefixes_help(&entries, spec, &|| {
                        count.set(count.get() + 1);
                        count.get() == budget
                    }),
                    Err(ProjectionError::Cancelled)
                );
                assert_eq!(count.get(), budget);
            }
        }
    }

    #[test]
    fn scalar_cancellation_covers_preparation_runs_merges_and_conversion() {
        let entries: Vec<_> = (0..4099)
            .rev()
            .map(|i| Entry::new(format!("file{i}").into(), EntryKind::File))
            .collect();
        let original: Vec<_> = entries
            .iter()
            .map(|e| {
                (
                    e.id,
                    e.name.as_bytes().to_vec(),
                    e.name.as_bytes().as_ptr(),
                    e.kind,
                    e.meta,
                )
            })
            .collect();
        let chunks = entries.len().div_ceil(CHUNK);
        for field in [SortField::Size, SortField::Modified] {
            let spec = SortSpec {
                field,
                ..SortSpec::default()
            };
            let calls = Cell::new(0);
            let complete = sort_indices(&entries, spec, &|| {
                calls.set(calls.get() + 1);
                false
            })
            .unwrap();
            let total = calls.get();
            assert_eq!(complete.len(), entries.len());
            // First-pair disorder and short digit runs give two initial and
            // six admitted-sample checkpoints before full preparation.
            for (phase, budget) in [
                ("prepare", 9 + chunks / 2),
                ("run", 9 + chunks + chunks / 2),
                ("merge", 10 + chunks * 2 + chunks / 2),
                ("convert", total - chunks / 2),
            ] {
                let count = Cell::new(0);
                assert_eq!(
                    sort_indices(&entries, spec, &|| {
                        count.set(count.get() + 1);
                        count.get() == budget
                    }),
                    Err(ProjectionError::Cancelled),
                    "{field:?} {phase}"
                );
                assert_eq!(count.get(), budget);
                for (entry, before) in entries.iter().zip(&original) {
                    assert_eq!(
                        (
                            entry.id,
                            entry.name.as_bytes(),
                            entry.name.as_bytes().as_ptr(),
                            entry.kind,
                            entry.meta
                        ),
                        (before.0, before.1.as_slice(), before.2, before.3, before.4)
                    );
                }
            }
        }
    }

    #[test]
    fn bounded_prefix_sample_rejects_colliding_stems_and_observes_cancellation() {
        let entries: Vec<_> = (0usize..1025)
            .map(|i| {
                Entry::new(
                    format!("a common long stem beyond sixteen bytes {i}").into(),
                    if i.is_multiple_of(3) {
                        EntryKind::Dir
                    } else {
                        EntryKind::File
                    },
                )
            })
            .collect();
        assert!(!name_prefixes_distinguish(&entries, SortSpec::default(), &|| false).unwrap());
        let diverse: Vec<_> = (0..1025)
            .map(|i| Entry::new(format!("file{i}").into(), EntryKind::File))
            .collect();
        assert!(name_prefixes_distinguish(&diverse, SortSpec::default(), &|| false).unwrap());
        for (distinct, expected) in [(2, false), (3, true)] {
            let boundary: Vec<_> = (0..64)
                .map(|i| Entry::new(format!("file{}", i % distinct).into(), EntryKind::File))
                .collect();
            assert_eq!(
                name_prefixes_distinguish(&boundary, SortSpec::default(), &|| false).unwrap(),
                expected
            );
        }
        for budget in [1, 2, 3] {
            let calls = Cell::new(0);
            assert_eq!(
                name_prefixes_distinguish(&entries, SortSpec::default(), &|| {
                    calls.set(calls.get() + 1);
                    calls.get() == budget
                }),
                Err(ProjectionError::Cancelled)
            );
        }
    }

    #[test]
    fn long_digit_scan_and_leading_zero_trim_observe_cancellation() {
        let mut name = vec![b'0'; 8192];
        name.push(b'1');
        let entry = Entry::new(OsString::from_vec(name), EntryKind::File);
        for budget in [1, 20, 40, 60] {
            let calls = Cell::new(0);
            assert_eq!(
                name_prefix(&entry, SortSpec::default(), &|| {
                    calls.set(calls.get() + 1);
                    calls.get() == budget
                }),
                Err(ProjectionError::Cancelled)
            );
            assert_eq!(calls.get(), budget);
        }
    }
}
