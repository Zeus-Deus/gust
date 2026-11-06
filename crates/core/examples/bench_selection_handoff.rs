//! Selection cancellation/commit returns before bounded recipient cleanup.
//! One recipient thread and a synchronous one-owner channel model ownership
//! transfer only. This is not the app retirement scheduler or newest-start QA.
use gust_core::projection::{Cancellation, ProjectionError};
use gust_core::selection::{RangeMode, Selection, SelectionScratch};
use gust_core::{Entry, EntryKind};
use std::cell::Cell;
use std::ffi::OsString;
use std::sync::mpsc::{self, SyncSender};
use std::time::Instant;

fn cancelled_receipt(
    label: &str,
    cancel_at: usize,
    recipient: &SyncSender<SelectionScratch>,
    cleaned: &mpsc::Receiver<(usize, f64)>,
    run: impl FnOnce(&mut SelectionScratch, &dyn Fn() -> bool) -> Result<(), ProjectionError>,
) {
    let requested = Cell::new(None);
    let observed = Cell::new(None);
    let calls = Cell::new(0);
    let cancel = || {
        calls.set(calls.get() + 1);
        if calls.get() == cancel_at {
            requested.set(Some(Instant::now()));
            return false;
        }
        if calls.get() > cancel_at {
            observed.set(Some(Instant::now()));
            true
        } else {
            false
        }
    };
    let mut scratch = SelectionScratch::default();
    assert!(matches!(
        run(&mut scratch, &cancel),
        Err(ProjectionError::Cancelled)
    ));
    let returned = Instant::now();
    let retained = scratch.tracked_names();
    assert!(retained > 0);
    // This send moves the caller-owned scratch; no core destructor ran.
    recipient.send(scratch).unwrap();
    let (recipient_count, cleanup_ms) = cleaned.recv().unwrap();
    assert_eq!(retained, recipient_count);
    println!("cancel phase={label} n=1000000 observed_us={:.3} return_before_recipient_cleanup_us={:.3} retained_names={retained} recipient_cleanup_ms={cleanup_ms:.3} next_app_start_measured=false",
        observed.get().unwrap().duration_since(requested.get().unwrap()).as_secs_f64()*1e6,
        returned.duration_since(requested.get().unwrap()).as_secs_f64()*1e6);
}

fn committed_receipt(
    label: &str,
    recipient: &SyncSender<SelectionScratch>,
    cleaned: &mpsc::Receiver<(usize, f64)>,
    run: impl FnOnce(&mut SelectionScratch) -> Result<(), ProjectionError>,
) {
    let mut scratch = SelectionScratch::default();
    let start = Instant::now();
    run(&mut scratch).unwrap();
    let return_ms = start.elapsed().as_secs_f64() * 1e3;
    let retained = scratch.tracked_names();
    assert!(retained > 0);
    recipient.send(scratch).unwrap();
    let (recipient_count, cleanup_ms) = cleaned.recv().unwrap();
    assert_eq!(retained, recipient_count);
    println!("commit phase={label} n=1000000 return_before_recipient_cleanup_ms={return_ms:.3} retained_names={retained} recipient_cleanup_ms={cleanup_ms:.3} next_app_start_measured=false");
}

fn main() {
    println!("profile_debug_assertions={} scratch_max=2_name_sets_plus_anchor retirement_channel_capacity=1 production_core_threads=0 app_scheduler_measured=false", cfg!(debug_assertions));
    let n: usize = 1_000_000;
    let entries: Vec<_> = (0..n)
        .map(|i| Entry::new(format!("file{i:08}").into(), EntryKind::File))
        .collect();
    let names: Vec<OsString> = entries.iter().map(|e| e.name.clone()).collect();
    let visible: Vec<_> = (0..n).collect();
    let mut source = Selection::new();
    for name in &names {
        source.toggle(name);
    }
    // The fixed recipient exists before any CPU work. There is only one
    // in-flight scratch owner; each receipt acknowledges cleanup before reuse.
    let (send, recv) = mpsc::sync_channel::<SelectionScratch>(1);
    let (done, cleaned) = mpsc::sync_channel(1);
    let cpu_thread = std::thread::current().id();
    let recipient = std::thread::spawn(move || {
        while let Ok(scratch) = recv.recv() {
            assert_ne!(std::thread::current().id(), cpu_thread);
            let retained = scratch.tracked_names();
            let start = Instant::now();
            drop(scratch);
            done.send((retained, start.elapsed().as_secs_f64() * 1e3))
                .unwrap();
        }
    });
    let mid = n.div_ceil(256) / 2;
    cancelled_receipt("clone_prune", mid, &send, &cleaned, |scratch, cancel| {
        source
            .clone_retain_visible_with_scratch(&entries, &visible, scratch, &cancel)
            .map(|_| ())
    });
    cancelled_receipt("select_all", mid, &send, &cleaned, |scratch, cancel| {
        source.select_all_by_name_cancellable_with_scratch(&names, scratch, &cancel)
    });
    let mut inverted = Selection::new();
    cancelled_receipt("invert", mid, &send, &cleaned, |scratch, cancel| {
        inverted.invert_by_name_cancellable_with_scratch(&names, scratch, &cancel)
    });
    assert!(inverted.is_untouched());
    cancelled_receipt("band", mid, &send, &cleaned, |scratch, cancel| {
        source.set_band_cancellable_with_scratch(&names[..100], &names[100..], scratch, &cancel)
    });
    source.set_anchor(std::ffi::OsStr::new("missing-anchor"));
    cancelled_receipt(
        "range_pins",
        n.div_ceil(256) + mid + 4,
        &send,
        &cleaned,
        |scratch, cancel| {
            source.select_range_by_name_cancellable_with_scratch(
                &names,
                n - 1,
                RangeMode::Replace,
                scratch,
                &cancel,
            )
        },
    );
    cancelled_receipt("extend", mid, &send, &cleaned, |scratch, cancel| {
        source.extend_by_name_cancellable_with_scratch(
            &names,
            0,
            n - 1,
            RangeMode::Add,
            scratch,
            &cancel,
        )
    });
    // Cancel extend during copied-base work as well as anchor search.
    cancelled_receipt(
        "extend_copy",
        n.div_ceil(256) + mid + 4,
        &send,
        &cleaned,
        |scratch, cancel| {
            source.extend_by_name_cancellable_with_scratch(
                &names,
                usize::MAX,
                n - 1,
                RangeMode::Add,
                scratch,
                &cancel,
            )
        },
    );
    assert_eq!(source.len(), n);
    for kind in 0..5 {
        // Set up a full old selection outside the timed gesture so each
        // successful commit has a directory-sized replaced owner to retire.
        let mut current = source.clone();
        committed_receipt(
            ["select_all", "invert", "band", "range", "extend"][kind],
            &send,
            &cleaned,
            |scratch| match kind {
                0 => {
                    current.select_all_by_name_cancellable_with_scratch(&names, scratch, &|| false)
                }
                1 => current.invert_by_name_cancellable_with_scratch(&names, scratch, &|| false),
                2 => {
                    current
                        .set_band_cancellable_with_scratch(&names[..100], &[], scratch, &|| false)
                }
                3 => current.select_range_by_name_cancellable_with_scratch(
                    &names,
                    0,
                    RangeMode::Replace,
                    scratch,
                    &|| false,
                ),
                _ => current.extend_by_name_cancellable_with_scratch(
                    &names,
                    0,
                    0,
                    RangeMode::Add,
                    scratch,
                    &|| false,
                ),
            },
        );
    }
    drop(send);
    recipient.join().unwrap();
    // Exercise the same seam directly so this probe cannot silently switch
    // to a non-cancellation adapter.
    assert!(Cancellation::checkpoint(&|| true));
}
