//! Scalar-sort acceptance: full API, exact vectors, transient storage and cancellation.
//! Run timing, memory and cancel separately: allocation profiling is disabled
//! for timing. Legacy input clones are untimed setup and may change name locality.
use gust_core::entry::{sort_entries, Entry, EntryId, EntryKind, Metadata, SortField, SortSpec};
use gust_core::projection::{sort_indices, ProjectionError};
use gust_core::CancelToken;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

struct ProfileAlloc;
static ACTIVE: AtomicBool = AtomicBool::new(false);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);
static FREE_ORIGIN: OnceLock<Instant> = OnceLock::new();
static CANCEL_FREE: AtomicBool = AtomicBool::new(false);
static FREE_FIRST: AtomicUsize = AtomicUsize::new(0);
static FREE_LAST: AtomicUsize = AtomicUsize::new(0);
static FREE_SUM: AtomicUsize = AtomicUsize::new(0);
static FREE_BYTES: AtomicUsize = AtomicUsize::new(0);
#[global_allocator]
static ALLOCATOR: ProfileAlloc = ProfileAlloc;

fn added(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(live, Ordering::Relaxed);
    CALLS.fetch_add(1, Ordering::Relaxed);
}
// Profiling scopes below allocate/free only their own sort storage. Inputs
// and validation allocations stay outside the scope; no other thread runs.
unsafe impl GlobalAlloc for ProfileAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() && ACTIVE.load(Ordering::Relaxed) {
            added(layout.size());
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ACTIVE.load(Ordering::Relaxed) {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        let tracking = CANCEL_FREE.load(Ordering::Relaxed);
        let start = if tracking {
            let now = FREE_ORIGIN.get().unwrap().elapsed().as_nanos() as usize;
            let _ = FREE_FIRST.compare_exchange(0, now, Ordering::Relaxed, Ordering::Relaxed);
            FREE_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
            now
        } else {
            0
        };
        unsafe {
            System.dealloc(ptr, layout);
        }
        if tracking {
            let end = FREE_ORIGIN.get().unwrap().elapsed().as_nanos() as usize;
            FREE_LAST.store(end, Ordering::Relaxed);
            FREE_SUM.fetch_add(end - start, Ordering::Relaxed);
        }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let next = unsafe { System.realloc(ptr, layout, size) };
        if !next.is_null() && ACTIVE.load(Ordering::Relaxed) {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            added(size);
        }
        next
    }
}

fn begin_profile(enabled: bool) {
    LIVE.store(0, Ordering::Relaxed);
    PEAK.store(0, Ordering::Relaxed);
    CALLS.store(0, Ordering::Relaxed);
    ACTIVE.store(enabled, Ordering::Relaxed);
}
fn end_profile() -> (usize, usize, usize) {
    ACTIVE.store(false, Ordering::Relaxed);
    (
        PEAK.load(Ordering::Relaxed),
        LIVE.load(Ordering::Relaxed),
        CALLS.load(Ordering::Relaxed),
    )
}
fn checksum<'a>(entries: impl Iterator<Item = &'a Entry>) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64;
    for e in entries {
        for b in e.id.0.to_le_bytes().iter().chain(e.name.as_bytes()) {
            h = (h ^ *b as u64).wrapping_mul(0x1000_0000_01b3);
        }
    }
    h
}
fn frozen_fixture(n: usize, case: &str) -> Vec<Entry> {
    let mut entries: Vec<_> = (0..n)
        .map(|i| {
            let number = (i * 524_287) % n;
            let mut bytes = if case == "long_stem" {
                format!("a shared filename stem well beyond sixteen bytes {number:08}.txt")
                    .into_bytes()
            } else if case == "adversarial" {
                match i % 10 {
                    0 => b"exact-tie".to_vec(),
                    1 => b"A00b".to_vec(),
                    2 => b"a0B".to_vec(),
                    3 => format!("\u{e9}\u{1f980}{}", number % 37).into_bytes(),
                    4 => [
                        b"\xff\xfe".as_slice(),
                        format!("{}", number % 53).as_bytes(),
                    ]
                    .concat(),
                    5 => vec![b'9'; 255],
                    6 => vec![b'9'; 256],
                    7 => [vec![b'0'; 300], b"9.txt".to_vec()].concat(),
                    _ => format!(
                        "a shared filename stem beyond sixteen bytes {}",
                        number % 101
                    )
                    .into_bytes(),
                }
            } else {
                format!(
                    "{}File{number:08}.txt",
                    if number.is_multiple_of(11) { "." } else { "" }
                )
                .into_bytes()
            };
            if number.is_multiple_of(29) {
                bytes.push(0xff);
            }
            let mut entry = Entry::new(
                OsString::from_vec(bytes),
                if number.is_multiple_of(7) {
                    EntryKind::Dir
                } else {
                    EntryKind::File
                },
            );
            entry.id = EntryId(i as u64);
            if !i.is_multiple_of(4) {
                entry.meta = Some(Metadata {
                    size: (number % 17) as u64,
                    mtime: (number % 19) as i64 - 9,
                    ..Metadata::default()
                });
            }
            entry
        })
        .collect();
    if case == "random" {
        let mut seed = 0x1234_5678_90ab_cdef_u64;
        for i in (1..n).rev() {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            entries.swap(i, seed as usize % (i + 1));
        }
    } else if case == "ordered" {
        sort_entries(&mut entries, SortSpec::default());
    }
    entries
}
// The receipt adapter replaces only this function body with the byte-identical
// frozen Stage 2 projection module. A normal Cargo run uses the current source
// as its control and makes no claim to measure retained Stage 2.
fn retained_indices(entries: &[Entry], spec: SortSpec) -> Vec<usize> {
    sort_indices(entries, spec, &CancelToken::new()).unwrap()
}

fn fixture(n: usize, case: &str, spec: SortSpec) -> Vec<Entry> {
    let base = if matches!(case, "long_stem" | "adversarial") {
        case
    } else {
        "random"
    };
    let mut entries = frozen_fixture(n, base);
    if case == "high" {
        for (i, e) in entries.iter_mut().enumerate() {
            e.meta = Some(Metadata {
                size: (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15),
                mtime: (i as i64).wrapping_mul(-7046029254386353131),
                ..Metadata::default()
            });
        }
    } else if case == "equal" {
        for e in &mut entries {
            e.meta = Some(Metadata::default());
        }
    } else if case == "raw" {
        for (i, e) in entries.iter_mut().enumerate() {
            let mut name = vec![(i % 256) as u8, 0, 1, 127, 128, 255];
            name.extend(format!("file{}", i % 211).as_bytes());
            e.name = OsString::from_vec(name);
            e.meta = if i % 7 == 0 {
                None
            } else {
                Some(Metadata {
                    size: [0, 1, u64::MAX, u64::MAX - 1][i % 4],
                    mtime: [0, -1, 1, i64::MIN, i64::MAX][i % 5],
                    ..Metadata::default()
                })
            };
        }
    }
    if case == "ordered" || case == "reversed" {
        sort_entries(&mut entries, spec);
        if case == "reversed" {
            entries.reverse();
        }
    }
    entries
}

fn expected(entries: &[Entry], spec: SortSpec) -> Vec<usize> {
    let mut order: Vec<_> = (0..entries.len()).collect();
    order.sort_by(|&a, &b| gust_core::entry::compare_entries(&entries[a], &entries[b], spec));
    order
}

fn pairs(n: usize, case: &str, spec: SortSpec, rounds: usize) {
    let entries = fixture(n, case, spec);
    let expected = expected(&entries, spec);
    let sum = checksum(expected.iter().map(|&i| &entries[i]));
    for round in 0..rounds {
        // Each adjacent round is A/B/C then C/B/A. Both candidate/control and
        // candidate/legacy therefore receive alternating order (ABBA).
        for worker in if round % 2 == 0 {
            ["legacy", "stage2", "candidate"]
        } else {
            ["candidate", "stage2", "legacy"]
        } {
            let mut legacy = if worker == "legacy" {
                entries.clone()
            } else {
                Vec::new()
            };
            let start = Instant::now();
            let order = match worker {
                "legacy" => {
                    sort_entries(&mut legacy, spec);
                    Vec::new()
                }
                "stage2" => retained_indices(&entries, spec),
                _ => sort_indices(&entries, spec, &CancelToken::new()).unwrap(),
            };
            let ms = start.elapsed().as_secs_f64() * 1000.;
            if worker == "legacy" {
                assert_eq!(legacy.len(), expected.len());
                for (e, &i) in legacy.iter().zip(&expected) {
                    assert_eq!(e.id, entries[i].id);
                    assert_eq!(e.name, entries[i].name);
                    assert_eq!(e.kind, entries[i].kind);
                    assert_eq!(e.meta, entries[i].meta);
                }
            } else {
                assert_eq!(order, expected);
            }
            println!("pair n={n} case={case} field={:?} asc={} dirs={} round={round} which={worker} ms={ms:.6} checksum={sum:016x} exact_vector=true profile=false",spec.field,spec.ascending,spec.dirs_first);
        }
    }
}

fn memory(n: usize, case: &str, spec: SortSpec) {
    let entries = fixture(n, case, spec);
    let expected = expected(&entries, spec);
    let calls = Cell::new(0usize);
    drop(
        sort_indices(&entries, spec, &|| {
            calls.set(calls.get() + 1);
            false
        })
        .unwrap(),
    );
    let total = calls.get();
    let count = Cell::new(0usize);
    let conversion_peak = Cell::new(0usize);
    let ordered = expected.iter().copied().eq(0..n);
    begin_profile(true);
    let order = sort_indices(&entries, spec, &|| {
        count.set(count.get() + 1);
        if !ordered && count.get() >= total - n.div_ceil(256) && count.get() < total {
            conversion_peak.set(conversion_peak.get().max(LIVE.load(Ordering::Relaxed)));
        }
        false
    })
    .unwrap();
    let peak = PEAK.load(Ordering::Relaxed);
    let live = LIVE.load(Ordering::Relaxed);
    assert_eq!(live, n * std::mem::size_of::<usize>());
    drop(order);
    let (_, after_drop, allocs) = end_profile();
    assert_eq!(after_drop, 0);
    assert!(peak <= n * 48);
    if ordered {
        assert_eq!(peak, n * 8);
    } else if peak == n * 48 {
        assert_eq!(conversion_peak.get(), n * 32);
    } else {
        assert_eq!(peak, n * 16); // Conservative admission kept the index path.
    }
    let order = sort_indices(&entries, spec, &|| false).unwrap();
    assert_eq!(order, expected);
    println!("memory n={n} case={case} field={:?} asc={} dirs={} peak_bytes={peak} live_output_bytes={live} after_drop_bytes={after_drop} allocs={allocs} bound_bytes={} exact_vector=true conversion_included=true conversion_peak_bytes={} ordered_no_key={ordered}", spec.field,spec.ascending,spec.dirs_first,n*48,conversion_peak.get());
}

fn cancellation(n: usize, spec: SortSpec) {
    // Short plain names ensure no digit-scan checkpoints. First pair is
    // deliberately disordered; exact phase boundaries follow the chunk loops.
    let mut entries: Vec<_> = (0..n)
        .rev()
        .map(|i| {
            let mut e = Entry::new(format!("file{i}").into(), EntryKind::File);
            e.id = EntryId(i as u64);
            e
        })
        .collect();
    if !spec.ascending {
        entries.reverse();
    }
    let original = checksum(entries.iter());
    let pointers: Vec<_> = entries.iter().map(|e| e.name.as_bytes().as_ptr()).collect();
    let calls = Cell::new(0usize);
    let order = sort_indices(&entries, spec, &|| {
        calls.set(calls.get() + 1);
        false
    })
    .unwrap();
    let total = calls.get();
    let chunks = n.div_ceil(256);
    let levels = (0..usize::BITS)
        .take_while(|&i| 256usize.saturating_mul(1usize << i) < n)
        .count();
    assert_eq!(
        total,
        2 + 6 + chunks * 2 + levels * (chunks + 1) + 1 + chunks + 1
    );
    assert_eq!(order.len(), n);
    drop(order);
    for (phase, budget) in [
        ("prepare", 9 + chunks / 2),
        ("run", 9 + chunks + chunks / 2),
        ("merge", 10 + chunks * 2 + chunks / 2),
        ("conversion", total - chunks / 2),
    ] {
        for repeat in 0..3 {
            let count = Cell::new(0);
            let requested = Cell::new(None);
            let observed = Cell::new(None);
            FREE_ORIGIN.get_or_init(Instant::now);
            FREE_FIRST.store(0, Ordering::Relaxed);
            FREE_LAST.store(0, Ordering::Relaxed);
            FREE_SUM.store(0, Ordering::Relaxed);
            FREE_BYTES.store(0, Ordering::Relaxed);
            begin_profile(true);
            let result = sort_indices(&entries, spec, &|| {
                count.set(count.get() + 1);
                if count.get() == budget {
                    requested.set(Some(Instant::now()));
                    false
                } else if count.get() == budget + 1 {
                    observed.set(Some(Instant::now()));
                    CANCEL_FREE.store(true, Ordering::Relaxed);
                    true
                } else {
                    false
                }
            });
            let returned = Instant::now();
            CANCEL_FREE.store(false, Ordering::Relaxed);
            let free_sum = FREE_SUM.load(Ordering::Relaxed);
            let free_span = FREE_LAST.load(Ordering::Relaxed) - FREE_FIRST.load(Ordering::Relaxed);
            let free_bytes = FREE_BYTES.load(Ordering::Relaxed);
            let (peak, live, allocs) = end_profile();
            assert_eq!(result, Err(ProjectionError::Cancelled));
            assert_eq!(count.get(), budget + 1);
            assert_eq!(live, 0);
            // This is a sequential core continuation, not the application
            // scheduler's newest-request start (qualified separately by parent).
            let continuation = Instant::now();
            assert_eq!(checksum(entries.iter()), original);
            assert!(entries
                .iter()
                .zip(&pointers)
                .all(|(e, &p)| e.name.as_bytes().as_ptr() == p));
            println!("cancel n={n} field={:?} phase={phase} repeat={repeat} budget={budget} observed_checkpoint={} total={total} request_to_observe_us={:.3} observe_to_return_including_flat_free_us={:.3} return_to_core_continuation_us={:.3} peak_bytes={peak} live_after_return_bytes={live} allocs={allocs} backing_unchanged=true flat_free_bytes={free_bytes} flat_free_sum_us={:.3} flat_free_span_us={:.3} app_scheduler_measured=false",spec.field,count.get(),observed.get().unwrap().duration_since(requested.get().unwrap()).as_secs_f64()*1e6,returned.duration_since(observed.get().unwrap()).as_secs_f64()*1e6,continuation.duration_since(returned).as_secs_f64()*1e6,free_sum as f64/1000.,free_span as f64/1000.);
        }
    }
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("timing");
    let n = args.get(2).map_or(100_000, |s| s.parse().unwrap());
    let field = match args.get(3).map(String::as_str).unwrap_or("Size") {
        "Size" => SortField::Size,
        "Modified" => SortField::Modified,
        "Kind" => SortField::Kind,
        _ => panic!("field"),
    };
    let case = args.get(4).map(String::as_str).unwrap_or("random");
    let spec = SortSpec {
        field,
        ascending: args.get(5).is_none_or(|s| s == "true"),
        dirs_first: args.get(6).is_none_or(|s| s == "true"),
    };
    println!("mode={mode} debug_assertions={} allocator_profile_separate=true setup_validation_untimed=true app_latency_measured=false retained_adapter_required=true",cfg!(debug_assertions));
    match mode {
        "timing" => pairs(n, case, spec, args.get(7).map_or(4, |s| s.parse().unwrap())),
        "oracle" => pairs(n, case, spec, 1),
        "memory" => memory(n, case, spec),
        "cancel" => cancellation(n, spec),
        _ => panic!("mode must be timing/oracle/memory/cancel"),
    }
}
