//! Release acceptance for exact ordering, throughput, allocation and cancellation.
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
use std::time::Instant;

struct ProfileAlloc;
static ACTIVE: AtomicBool = AtomicBool::new(false);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);
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
        unsafe {
            System.dealloc(ptr, layout);
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
fn fixture(n: usize, case: &str) -> Vec<Entry> {
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
fn pairs(n: usize, case: &str, spec: SortSpec, profile: bool) {
    let entries = fixture(n, case);
    let mut expected = entries.clone();
    sort_entries(&mut expected, spec);
    let sum = checksum(expected.iter());
    for pair in 0usize..if profile { 1 } else { 4 } {
        let mut legacy = entries.clone();
        for worker in if pair.is_multiple_of(2) {
            [false, true]
        } else {
            [true, false]
        } {
            begin_profile(profile);
            let start = Instant::now();
            if worker {
                let indices = sort_indices(&entries, spec, &CancelToken::new()).unwrap();
                let ms = start.elapsed().as_secs_f64() * 1e3;
                let (peak, live, allocs) = end_profile();
                assert_eq!(indices.len(), n);
                let mut seen = vec![false; n];
                for (&i, expected) in indices.iter().zip(&expected) {
                    assert!(!seen[i]);
                    seen[i] = true;
                    assert_eq!(entries[i].id, expected.id);
                    assert_eq!(entries[i].name, expected.name);
                }
                assert!(seen.into_iter().all(|v| v));
                assert_eq!(checksum(indices.iter().map(|&i| &entries[i])), sum);
                if profile {
                    assert_eq!(live, n * std::mem::size_of::<usize>());
                }
                println!("pair n={n} case={case} field={:?} asc={} dirs={} round={pair} first={} which=index ms={ms:.3} profile={profile} peak_bytes={peak} live_output_bytes={live} alloc_calls={allocs} checksum={sum:016x} exact_membership_order=true",spec.field,spec.ascending,spec.dirs_first,if pair.is_multiple_of(2){"legacy"}else{"index"});
            } else {
                sort_entries(&mut legacy, spec);
                let ms = start.elapsed().as_secs_f64() * 1e3;
                let (peak, live, allocs) = end_profile();
                for (actual, expected) in legacy.iter().zip(&expected) {
                    assert_eq!(actual.id, expected.id);
                    assert_eq!(actual.name, expected.name);
                }
                assert_eq!(checksum(legacy.iter()), sum);
                if profile {
                    assert_eq!(live, 0);
                }
                println!("pair n={n} case={case} field={:?} asc={} dirs={} round={pair} first={} which=legacy ms={ms:.3} profile={profile} peak_bytes={peak} live_output_bytes={live} alloc_calls={allocs} checksum={sum:016x} exact_membership_order=true",spec.field,spec.ascending,spec.dirs_first,if pair.is_multiple_of(2){"legacy"}else{"index"});
            }
        }
    }
}
fn cancellation(n: usize, case: &str) {
    let entries = fixture(n, case);
    let spec = SortSpec::default();
    let calls = Cell::new(0);
    let baseline = sort_indices(&entries, spec, &|| {
        calls.set(calls.get() + 1);
        false
    })
    .unwrap();
    let total = calls.get();
    assert_eq!(baseline.len(), n);
    drop(baseline);
    let rows = n.div_ceil(256);
    for (label, budget) in [
        ("prepare", rows / 2),
        ("runs", rows + rows / 2 + 4),
        ("merge", rows * 3 + 128),
        (
            if case == "long_stem" {
                "final_merge"
            } else {
                "output"
            },
            total - rows / 2 - 1,
        ),
    ] {
        let count = Cell::new(0);
        let requested = Cell::new(None);
        let observed = Cell::new(None);
        let token = CancelToken::new();
        let cancel = || {
            count.set(count.get() + 1);
            if count.get() == budget {
                requested.set(Some(Instant::now()));
                token.cancel();
                return false;
            }
            let cancelled = token.checkpoint();
            if cancelled {
                observed.set(Some(Instant::now()));
            }
            cancelled
        };
        begin_profile(true);
        assert_eq!(
            sort_indices(&entries, spec, &cancel),
            Err(ProjectionError::Cancelled)
        );
        let returned = Instant::now();
        let (peak, live, allocs) = end_profile();
        assert_eq!(live, 0);
        let requested = requested.get().unwrap();
        let observed = observed.get().unwrap();
        println!("cancel n={n} case={case} requested_phase={label} checkpoints={} total_complete_checkpoints={total} observation_us={:.3} return_after_free_us={:.3} peak_bytes={peak} live_after_return_bytes={live} alloc_calls={allocs} profile=true deterministic_after_sample=true app_start_measured=false",count.get(),observed.duration_since(requested).as_secs_f64()*1e6,returned.duration_since(requested).as_secs_f64()*1e6);
    }
}
fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "timing".into());
    println!("mode={mode} debug_assertions={} allocator_profile_separate=true baseline_clone_setup_untimed=true app_latency_measured=false",cfg!(debug_assertions));
    match mode.as_str() {
        "timing" | "memory" => {
            for n in [100_000, 1_000_000] {
                for case in ["permutation", "random", "ordered", "long_stem"] {
                    pairs(n, case, SortSpec::default(), mode == "memory");
                }
            }
        }
        "adversarial" => {
            let filters: Vec<_> = std::env::args().skip(2).collect();
            for n in [100_000, 1_000_000] {
                if filters.first().is_some_and(|v| v != &n.to_string()) {
                    continue;
                }
                for field in [
                    SortField::Name,
                    SortField::Size,
                    SortField::Modified,
                    SortField::Kind,
                ] {
                    if filters.get(1).is_some_and(|v| v != &format!("{field:?}")) {
                        continue;
                    }
                    for ascending in [false, true] {
                        if filters.get(2).is_some_and(|v| v != &ascending.to_string()) {
                            continue;
                        }
                        for dirs_first in [false, true] {
                            if filters.get(3).is_some_and(|v| v != &dirs_first.to_string()) {
                                continue;
                            }
                            pairs(
                                n,
                                "adversarial",
                                SortSpec {
                                    field,
                                    ascending,
                                    dirs_first,
                                },
                                false,
                            );
                        }
                    }
                }
            }
        }
        "cancel" => {
            for case in ["permutation", "random", "long_stem"] {
                cancellation(1_000_000, case);
            }
        }
        _ => panic!("mode must be timing, memory, adversarial or cancel"),
    }
}
