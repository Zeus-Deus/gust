//! Release-only CPU projection parity and cooperative cancellation receipt.
//! No GUI/frame/presentation or app scheduler latency is measured here.
use gust_core::entry::{sort_entries, Entry, EntryId, EntryKind, Metadata, SortSpec};
use gust_core::projection::{filter_indices, map_visible_rows, sort_indices, ProjectionError};
use gust_core::search::{matching_indices, PreparedQuery};
use gust_core::selection::Selection;
use gust_core::CancelToken;
use std::cell::Cell;
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::time::Instant;

fn checksum<'a>(entries: impl Iterator<Item = &'a Entry>) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64;
    for entry in entries {
        for byte in entry.id.0.to_le_bytes().iter().chain(entry.name.as_bytes()) {
            h = (h ^ *byte as u64).wrapping_mul(0x1000_0000_01b3);
        }
    }
    h
}

fn cancellation_receipt<T>(
    label: &str,
    n: usize,
    cancel_at: usize,
    run: impl FnOnce(&dyn Fn() -> bool) -> Result<T, ProjectionError>,
) {
    let calls = Cell::new(0);
    let requested = Cell::new(None);
    let observed = Cell::new(None);
    let token = CancelToken::new();
    let cancel = || {
        calls.set(calls.get() + 1);
        if calls.get() == cancel_at {
            requested.set(Some(Instant::now()));
            token.cancel();
            // Request just after this sample: the next batch must run.
            return false;
        }
        let cancelled = token.is_cancelled();
        if cancelled && observed.get().is_none() {
            observed.set(Some(Instant::now()));
        }
        cancelled
    };
    assert!(matches!(run(&cancel), Err(ProjectionError::Cancelled)));
    let returned = Instant::now();
    let requested = requested.get().unwrap();
    let observed = observed.get().unwrap();
    println!("cancel phase={label} n={n} checkpoints={} observed_us={:.3} return_and_scratch_drop_us={:.3} deterministic_after_sample=true app_scheduler_measured=false", calls.get(), observed.duration_since(requested).as_secs_f64()*1e6, returned.duration_since(requested).as_secs_f64()*1e6);
}

fn run(n: usize) {
    let entries: Vec<_> = (0..n)
        .map(|i| {
            let number = (i * 524_287) % n;
            let mut name = format!(
                "{}File{number:08}.txt",
                if number.is_multiple_of(11) { "." } else { "" }
            )
            .into_bytes();
            if number.is_multiple_of(29) {
                name.push(0xff);
            }
            let mut e = Entry::new(
                OsString::from_vec(name),
                if number.is_multiple_of(7) {
                    EntryKind::Dir
                } else {
                    EntryKind::File
                },
            );
            e.id = EntryId(i as u64);
            if !number.is_multiple_of(17) {
                e.meta = Some(Metadata {
                    size: number as u64,
                    mtime: number as i64 - 1000,
                    ..Metadata::default()
                });
            }
            e
        })
        .collect();
    let token = CancelToken::new();
    let mut baseline = entries.clone();
    let t = Instant::now();
    sort_entries(&mut baseline, SortSpec::default());
    let baseline_sort_ms = t.elapsed().as_secs_f64() * 1000.;
    let t = Instant::now();
    let order = sort_indices(&entries, SortSpec::default(), &token).unwrap();
    let sort_ms = t.elapsed().as_secs_f64() * 1000.;
    let mut seen = vec![false; n];
    for (&i, expected) in order.iter().zip(&baseline) {
        assert!(!seen[i]);
        seen[i] = true;
        assert_eq!(entries[i].id, expected.id);
        assert_eq!(entries[i].name, expected.name);
    }
    assert!(seen.iter().all(|v| *v));
    let sorted_checksum = checksum(order.iter().map(|&i| &entries[i]));
    assert_eq!(sorted_checksum, checksum(baseline.iter()));
    let mut last_visible = Vec::new();
    for needle in ["", "f17", "TXT", "unmatched"] {
        for hidden in [false, true] {
            let t = Instant::now();
            let visible = filter_indices(
                &entries,
                &order,
                &PreparedQuery::new(needle),
                hidden,
                &token,
            )
            .unwrap();
            let filter_ms = t.elapsed().as_secs_f64() * 1000.;
            let expected = matching_indices(&baseline, needle, hidden);
            assert_eq!(visible.len(), expected.len());
            for (&index, &row) in visible.iter().zip(&expected) {
                assert_eq!(entries[index].id, baseline[row].id);
            }
            let sum = checksum(visible.iter().map(|&i| &entries[i]));
            assert_eq!(sum, checksum(expected.iter().map(|&i| &baseline[i])));
            println!("filter n={n} query={needle:?} hidden={hidden} count={} checksum={sum:016x} exact_equal=true ms={filter_ms:.3}", visible.len());
            if needle == "f17" && !hidden {
                last_visible = visible;
            }
        }
    }
    drop(baseline);
    drop(seen);
    let mut selection = Selection::new();
    for e in &entries {
        selection.toggle(&e.name);
    }
    let t = Instant::now();
    let prepared = selection
        .clone_retain_visible(&entries, &last_visible, &token)
        .unwrap();
    let clone_prune_ms = t.elapsed().as_secs_f64() * 1000.;
    let mut expected = selection.clone();
    expected.retain_visible(&entries, &last_visible);
    assert_eq!(prepared.len(), expected.len());
    assert!(expected
        .names_vec()
        .iter()
        .all(|name| prepared.contains(name)));
    let summary = prepared
        .visible_summary(&entries, &last_visible, &token)
        .unwrap();
    assert_eq!(summary.selected_count, last_visible.len());
    let mapping = map_visible_rows(&entries, &order, &entries, &last_visible, &token).unwrap();
    assert_eq!(
        mapping.iter().filter(|&&i| i >= 0).count(),
        last_visible.len()
    );
    for (old, &new) in mapping.iter().enumerate() {
        if new >= 0 {
            assert_eq!(
                entries[order[old]].name,
                entries[last_visible[new as usize]].name
            );
        }
    }
    println!("sort n={n} count={} checksum={sorted_checksum:016x} full_membership=true exact_equal=true baseline_ms={baseline_sort_ms:.3} worker_ms={sort_ms:.3}", order.len());
    println!("selection n={n} surviving={} known_bytes={} missing_metadata={} exact_membership_equal=true clone_prune_ms={clone_prune_ms:.3} mapping_exact=true", prepared.len(), summary.known_bytes, summary.missing_metadata);
    drop(expected);
    drop(prepared);
    let mid_scan = (n.div_ceil(256) / 2).max(3);
    cancellation_receipt("filter", n, mid_scan, |c| {
        filter_indices(&entries, &order, &PreparedQuery::new(""), true, &c)
    });
    cancellation_receipt("selection_clone_prune", n, mid_scan, |c| {
        selection.clone_retain_visible(&entries, &order, &c)
    });
    cancellation_receipt("selection_summary", n, mid_scan, |c| {
        selection.visible_summary(&entries, &order, &c)
    });
    cancellation_receipt("mapping_build", n, mid_scan, |c| {
        map_visible_rows(&entries, &order, &entries, &order, &c)
    });
    cancellation_receipt("mapping_lookup", n, n.div_ceil(256) + mid_scan, |c| {
        map_visible_rows(&entries, &order, &entries, &order, &c)
    });
    let names: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
    cancellation_receipt("select_all", n, mid_scan, |c| {
        selection.select_all_by_name_cancellable(&names, &c)
    });
    cancellation_receipt("invert", n, mid_scan, |c| {
        selection.invert_by_name_cancellable(&names, &c)
    });
    cancellation_receipt("band", n, mid_scan, |c| {
        selection.set_band_cancellable(&names[..100], &names[100..], &c)
    });
    // A missing anchor forces a full checked search before copying pins.
    selection.set_anchor(std::ffi::OsStr::new("missing-anchor"));
    cancellation_receipt("range_pin_clone", n, n.div_ceil(256) + mid_scan + 4, |c| {
        selection.select_range_by_name_cancellable(
            &names,
            n - 1,
            gust_core::selection::RangeMode::Replace,
            &c,
        )
    });
    drop(names);
    drop(selection);
    drop(mapping);
    // Trigger supersession immediately after a deterministic checkpoint sample
    // in merge work, so the following batch runs before observing it.
    // The tail includes disposal of rejected scratch, relevant to admission of
    // the newest CPU request. It is not the app scheduler's start latency.
    let calls = Cell::new(0);
    let cancel_at = n.div_ceil(256) * 2 + 10;
    let requested = Cell::new(None);
    let observed = Cell::new(None);
    let cancelled = || {
        calls.set(calls.get() + 1);
        if calls.get() == cancel_at {
            requested.set(Some(Instant::now()));
            token.cancel();
            // Simulate a request immediately after this checkpoint's sample:
            // one complete bounded batch must run before the next observation.
            return false;
        }
        let cancelled = token.is_cancelled();
        if cancelled && observed.get().is_none() {
            observed.set(Some(Instant::now()));
        }
        cancelled
    };
    assert_eq!(
        sort_indices(&entries, SortSpec::default(), &cancelled),
        Err(ProjectionError::Cancelled)
    );
    let returned = Instant::now();
    let requested = requested.get().unwrap();
    let observed = observed.get().unwrap();
    let t = Instant::now();
    let newest = filter_indices(
        &entries,
        &order,
        &PreparedQuery::new("f17"),
        false,
        &CancelToken::new(),
    )
    .unwrap();
    println!("cancel n={n} checkpoint={} observed_us={:.3} return_and_scratch_drop_us={:.3} next_direct_start_us={:.3} next_direct_work_ms={:.3} next_count={} app_scheduler_measured=false", calls.get(), observed.duration_since(requested).as_secs_f64()*1e6, returned.duration_since(requested).as_secs_f64()*1e6, t.duration_since(requested).as_secs_f64()*1e6, t.elapsed().as_secs_f64()*1e3, newest.len());
}

fn main() {
    println!("profile_debug_assertions={}", cfg!(debug_assertions));
    println!("scope=pure_cpu; checkpoints=256_rows_or_one_256_index_sort_run; no_OS_deadline; risks=allocation_hash_rehash_name_length_scratch_drop_and_app_admission_not_bounded_in_time");
    for n in [100_000, 1_000_000] {
        run(n);
    }
}
