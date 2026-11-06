//! Differential contracts for worker projections, independent of publication.
use gust_core::entry::{sort_entries, Entry, EntryId, EntryKind, Metadata, SortField, SortSpec};
use gust_core::projection::{filter_indices, map_visible_rows, sort_indices, ProjectionError};
use gust_core::search::{matching_indices, PreparedQuery};
use gust_core::selection::Selection;
use gust_core::CancelToken;
use std::cell::Cell;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;

fn entries(n: usize) -> Vec<Entry> {
    let mut seed = 0x9876_5432_10ab_cdef_u64;
    (0..n)
        .map(|i| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            // Include exact duplicate names/kinds and sort keys to check stable
            // ties via unique test IDs, plus invalid UTF-8 and case/numeric ties.
            let mut name = match i % 8 {
                0 => b"same9".to_vec(),
                1 => format!(".File{:06}.txt", seed % 1000).into_bytes(),
                2 => format!("FILE{}.TXT", seed % 1000).into_bytes(),
                3 => format!("file{}.tar.zst", seed % 1000).into_bytes(),
                4 => format!("file{:03}", seed % 1000).into_bytes(),
                5 => b".\xff9".to_vec(),
                _ => format!("file{}", seed % 1000).into_bytes(),
            };
            if i.is_multiple_of(13) {
                name.push(0xfe);
            }
            let mut e = Entry::new(
                OsString::from_vec(name),
                match i % 5 {
                    0 => EntryKind::Dir,
                    1 => EntryKind::Symlink,
                    2 => EntryKind::Special,
                    3 => EntryKind::Unknown,
                    _ => EntryKind::File,
                },
            );
            e.id = EntryId(i as u64);
            if !i.is_multiple_of(3) {
                e.meta = Some(Metadata {
                    size: seed % 4,
                    mtime: (seed % 7) as i64 - 3,
                    ino: i as u64,
                    ..Metadata::default()
                });
            }
            e
        })
        .collect()
}

#[test]
fn permutations_match_existing_stable_sort_for_every_spec() {
    let entries = entries(4099); // multiple runs and incomplete merge tails
    let original_ids: Vec<_> = entries.iter().map(|e| e.id).collect();
    for field in [
        SortField::Name,
        SortField::Size,
        SortField::Modified,
        SortField::Kind,
    ] {
        for ascending in [false, true] {
            for dirs_first in [false, true] {
                let spec = SortSpec {
                    field,
                    ascending,
                    dirs_first,
                };
                let mut expected = entries.clone();
                sort_entries(&mut expected, spec);
                let order = sort_indices(&entries, spec, &CancelToken::new()).unwrap();
                let ids: Vec<_> = order.iter().map(|&i| entries[i].id).collect();
                assert_eq!(
                    ids,
                    expected.iter().map(|e| e.id).collect::<Vec<_>>(),
                    "{spec:?}"
                );
                assert_eq!(
                    entries.iter().map(|e| e.id).collect::<Vec<_>>(),
                    original_ids
                );
            }
        }
    }
}

#[test]
fn ordinary_filter_reuses_full_order_and_matches_existing_hidden_and_byte_rules() {
    let entries = entries(1301);
    for field in [
        SortField::Name,
        SortField::Size,
        SortField::Modified,
        SortField::Kind,
    ] {
        let order = sort_indices(
            &entries,
            SortSpec {
                field,
                ascending: false,
                dirs_first: true,
            },
            &|| false,
        )
        .unwrap();
        let sorted: Vec<_> = order.iter().map(|&i| entries[i].clone()).collect();
        let unchanged = order.clone();
        for needle in ["", "fl9", "FILE", ".", "é", "TXT", "does-not-match"] {
            let query = PreparedQuery::new(needle);
            for hidden in [false, true] {
                let actual = filter_indices(&entries, &order, &query, hidden, &|| false).unwrap();
                let expected: Vec<_> = matching_indices(&sorted, needle, hidden)
                    .iter()
                    .map(|&i| order[i])
                    .collect();
                assert_eq!(actual, expected, "{field:?} / {needle:?} / {hidden}");
                assert_eq!(order, unchanged);
            }
        }
    }
    let order = [9, 3, 9, 0];
    assert_eq!(
        filter_indices(&entries, &order, &PreparedQuery::new(""), true, &|| false),
        Ok(order.to_vec())
    );
}

#[test]
fn row_mapping_uses_exact_raw_names_across_revisions() {
    let make = |names: &[&[u8]]| {
        names
            .iter()
            .map(|n| Entry::new(OsString::from_vec(n.to_vec()), EntryKind::File))
            .collect::<Vec<_>>()
    };
    let old = make(&[b"a", b"\xff", b"\xfe", b".hidden", b"gone", b"A"]);
    let new = make(&[b"added", b"\xfe", b"A", b"\xff", b"a"]);
    assert_eq!(
        map_visible_rows(&old, &[5, 4, 3, 2, 1, 0], &new, &[3, 1, 2, 4, 0], &|| false),
        Ok(vec![2, -1, -1, 1, 0, 3])
    );
    assert_eq!(
        map_visible_rows(&old, &[], &new, &[], &|| false),
        Ok(vec![])
    );
}

#[test]
fn worker_summary_uses_selected_displayed_targets_and_known_metadata_only() {
    let mut entries = entries(4);
    entries[0].meta = Some(Metadata {
        size: u64::MAX,
        ..Metadata::default()
    });
    entries[1].meta = None;
    entries[2].meta = Some(Metadata {
        size: 9,
        ..Metadata::default()
    });
    let mut selection = Selection::new();
    for e in &entries[..3] {
        selection.toggle(&e.name);
    }
    let summary = selection
        .visible_summary(&entries, &[2, 1, 3, 0], &|| false)
        .unwrap();
    assert_eq!(summary.selected_count, 3);
    assert_eq!(summary.known_bytes, u64::MAX);
    assert_eq!(summary.missing_metadata, 1);
    assert_eq!(
        selection
            .visible_summary(&entries, &[], &|| false)
            .unwrap()
            .selected_count,
        0
    );
}

#[test]
fn bad_indices_are_errors_even_for_empty_query_or_untouched_selection() {
    let entries = entries(3);
    let bad = ProjectionError::InvalidIndex { index: 3, len: 3 };
    assert_eq!(
        filter_indices(&entries, &[0, 3], &PreparedQuery::new(""), false, &|| false),
        Err(bad)
    );
    assert_eq!(
        Selection::new()
            .clone_retain_visible(&entries, &[3], &|| false)
            .unwrap_err(),
        bad
    );
    assert_eq!(
        Selection::new().visible_summary(&entries, &[3], &|| false),
        Err(bad)
    );
    assert_eq!(
        map_visible_rows(&entries, &[3], &entries, &[0], &|| false),
        Err(bad)
    );
    assert_eq!(
        map_visible_rows(&entries, &[0], &entries, &[3], &|| false),
        Err(bad)
    );
}

#[test]
fn all_preparations_observe_pre_cancel_even_on_empty_inputs() {
    let cancel = CancelToken::new();
    cancel.cancel();
    assert_eq!(
        sort_indices(&[], SortSpec::default(), &cancel),
        Err(ProjectionError::Cancelled)
    );
    assert_eq!(
        filter_indices(&[], &[], &PreparedQuery::new(""), true, &cancel),
        Err(ProjectionError::Cancelled)
    );
    assert_eq!(
        map_visible_rows(&[], &[], &[], &[], &cancel),
        Err(ProjectionError::Cancelled)
    );
    assert_eq!(
        Selection::new()
            .clone_retain_visible(&[], &[], &cancel)
            .unwrap_err(),
        ProjectionError::Cancelled
    );
    assert_eq!(
        Selection::new().visible_summary(&[], &[], &cancel),
        Err(ProjectionError::Cancelled)
    );
}

#[test]
fn sort_observes_mid_work_cancellation_during_preparation_runs_and_merges() {
    let entries = entries(16_385);
    // Separate row-copy and run-sort passes each need at least one
    // checkpoint per 256 rows. Later budgets land in merge work.
    for budget in [2, 34, 70, 110, 150, 210, 400] {
        let calls = Cell::new(0);
        let cancel = || {
            calls.set(calls.get() + 1);
            calls.get() == budget
        };
        assert_eq!(
            sort_indices(&entries, SortSpec::default(), &cancel),
            Err(ProjectionError::Cancelled),
            "checkpoint {budget}"
        );
        assert_eq!(calls.get(), budget);
    }
}

#[test]
fn filter_mapping_selection_and_summary_observe_mid_work_cancellation() {
    let entries = entries(4097);
    let order: Vec<_> = (0..entries.len()).rev().collect();
    let mut selection = Selection::new();
    for e in &entries {
        selection.toggle(&e.name);
    }
    let original = selection.names_vec();
    for budget in [2, 5, 12] {
        let calls = Cell::new(0);
        let cancel = || {
            calls.set(calls.get() + 1);
            calls.get() == budget
        };
        assert_eq!(
            filter_indices(&entries, &order, &PreparedQuery::new("f"), true, &cancel),
            Err(ProjectionError::Cancelled)
        );
        calls.set(0);
        assert_eq!(
            selection
                .clone_retain_visible(&entries, &order, &cancel)
                .unwrap_err(),
            ProjectionError::Cancelled
        );
        calls.set(0);
        assert_eq!(
            selection.visible_summary(&entries, &order, &cancel),
            Err(ProjectionError::Cancelled)
        );
        calls.set(0);
        assert_eq!(
            map_visible_rows(&entries, &order, &entries, &order, &cancel),
            Err(ProjectionError::Cancelled)
        );
    }
    // A later checkpoint is reached only after the new-name map is built.
    let calls = Cell::new(0);
    assert_eq!(
        map_visible_rows(&entries, &order, &entries, &order, &|| {
            calls.set(calls.get() + 1);
            calls.get() == 25
        }),
        Err(ProjectionError::Cancelled)
    );
    assert_eq!(selection.len(), original.len());
    assert!(original.iter().all(|name| selection.contains(name)));
}

#[test]
fn already_ordered_fast_path_preserves_stable_ties_and_checks_during_scan_and_index_copy() {
    let mut entries = entries(4099);
    sort_entries(&mut entries, SortSpec::default());
    let original_ids: Vec<_> = entries.iter().map(|e| e.id).collect();
    assert_eq!(
        sort_indices(&entries, SortSpec::default(), &|| false).unwrap(),
        (0..entries.len()).collect::<Vec<_>>()
    );
    // These budgets cover the order scan and the later index-only copy.
    for budget in [2, 8, 18, 26] {
        let calls = Cell::new(0);
        let result = sort_indices(&entries, SortSpec::default(), &|| {
            calls.set(calls.get() + 1);
            calls.get() == budget
        });
        assert_eq!(result, Err(ProjectionError::Cancelled));
        assert_eq!(
            entries.iter().map(|e| e.id).collect::<Vec<_>>(),
            original_ids
        );
    }
}

#[test]
fn prefix_collisions_long_numbers_and_exact_ties_match_all_shared_sort_specs() {
    let stems = [
        b"same".to_vec(),
        b"a common stem exceeding sixteen bytes 0009".to_vec(),
        b"a common stem exceeding sixteen bytes 10".to_vec(),
        b"A00b".to_vec(),
        b"a0B".to_vec(),
        "é🦀9".as_bytes().to_vec(),
        b"\xff0\xfe".to_vec(),
        [vec![b'9'; 255], b".txt".to_vec()].concat(),
        [vec![b'9'; 256], b".txt".to_vec()].concat(),
        [vec![b'0'; 300], b"9.txt".to_vec()].concat(),
    ];
    let entries: Vec<_> = (0..4103)
        .map(|i| {
            let mut entry = Entry::new(
                OsString::from_vec(stems[(i * 37) % stems.len()].clone()),
                if i.is_multiple_of(3) {
                    EntryKind::Dir
                } else {
                    EntryKind::File
                },
            );
            entry.id = EntryId(i as u64);
            if !i.is_multiple_of(4) {
                entry.meta = Some(Metadata {
                    size: (i % 5) as u64,
                    mtime: (i % 7) as i64 - 3,
                    ..Metadata::default()
                });
            }
            entry
        })
        .collect();
    for field in [
        SortField::Name,
        SortField::Size,
        SortField::Modified,
        SortField::Kind,
    ] {
        for ascending in [false, true] {
            for dirs_first in [false, true] {
                let spec = SortSpec {
                    field,
                    ascending,
                    dirs_first,
                };
                let mut expected = entries.clone();
                sort_entries(&mut expected, spec);
                let actual = sort_indices(&entries, spec, &CancelToken::new()).unwrap();
                assert_eq!(
                    actual.iter().map(|&i| entries[i].id).collect::<Vec<_>>(),
                    expected.iter().map(|e| e.id).collect::<Vec<_>>(),
                    "{spec:?}"
                );
            }
        }
    }
}
