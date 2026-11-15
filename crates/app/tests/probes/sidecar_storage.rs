//! Isolated component allocator probe, no Qt/event loop or native memory claim.
//! Run one case per process. Input raw entries/names are outside the profile;
//! owned name table, sidecars, cache values and page construction are inside.
#[path = "../../src/lazy_pages.rs"]
mod lazy_pages;
#[path = "../../src/name_index.rs"]
mod name_index;
#[path = "../../src/row_cache.rs"]
mod row_cache;
use gust_core::{Entry, EntryKind, Metadata};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;
struct Profile;
#[global_allocator]
static ALLOC: Profile = Profile;
static ACTIVE: AtomicBool = AtomicBool::new(false);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);
fn add(n: usize) {
    let live = LIVE.fetch_add(n, Ordering::Relaxed) + n;
    PEAK.fetch_max(live, Ordering::Relaxed);
    CALLS.fetch_add(1, Ordering::Relaxed);
}
// Measurement only: all profiled allocations and frees stay inside the scope;
// raw input remains owned outside it. Production storage contains no unsafe.
unsafe impl GlobalAlloc for Profile {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() && ACTIVE.load(Ordering::Relaxed) {
            add(l.size())
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if ACTIVE.load(Ordering::Relaxed) {
            LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        }
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, n) };
        if !q.is_null() && ACTIVE.load(Ordering::Relaxed) {
            LIVE.fetch_sub(l.size(), Ordering::Relaxed);
            add(n)
        }
        q
    }
}
struct Slot<T> {
    value: OnceLock<T>,
    invalid: AtomicBool,
}
impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self {
            value: OnceLock::new(),
            invalid: AtomicBool::new(false),
        }
    }
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let variant = &args[1];
    let case = &args[2];
    let n: usize = args[3].parse().unwrap();
    let raw: Arc<Vec<Entry>> = Arc::new(
        (0..n)
            .map(|i| Entry::new(format!("file-{i:08}.png").into(), EntryKind::File))
            .collect(),
    );
    let touched = match case.as_str() {
        "empty" | "cancel" => 0,
        "sparse" | "renewal-sparse" => 60,
        "dense" | "cache-heavy" | "renewal-dense" => n,
        _ => panic!("case"),
    };
    ACTIVE.store(true, Ordering::Relaxed);
    let started = Instant::now();
    let mut cancel_return_ns = 0;
    let mut cancelled_names = 0;
    let retained: Box<dyn Send> = if case == "cancel" {
        let calls = AtomicUsize::new(0);
        let observed = std::cell::Cell::new(None);
        let partial = name_index::NameIndex::try_new(raw.clone(), &|| {
            let cancelled = calls.fetch_add(1, Ordering::Relaxed) > n * 3 / 4 / 128;
            if cancelled {
                observed.set(Some(Instant::now()));
            }
            cancelled
        })
        .err()
        .expect("partial ownership");
        cancel_return_ns = observed.get().unwrap().elapsed().as_nanos();
        cancelled_names = partial.len();
        assert!(partial.len() > n / 2);
        Box::new(partial)
    } else if variant == "legacy" {
        let map: std::collections::HashMap<_, _> = raw
            .iter()
            .enumerate()
            .map(|(i, e)| (e.name.clone(), i))
            .collect();
        let details: Vec<OnceLock<Entry>> = (0..n).map(|_| OnceLock::new()).collect();
        let thumbs: Vec<Slot<String>> = (0..n).map(|_| Slot::default()).collect();
        let sizes: Vec<Slot<gust_core::dirsize::Sized>> = (0..n).map(|_| Slot::default()).collect();
        for i in 0..touched {
            let mut e = raw[i].clone();
            e.meta = Some(Metadata::default());
            details[i].set(e).unwrap();
            if case == "cache-heavy" {
                thumbs[i]
                    .value
                    .set(format!("file:///cache/{i:08}?gust=version"))
                    .unwrap();
                sizes[i]
                    .value
                    .set(gust_core::dirsize::Sized::default())
                    .unwrap();
            }
        }
        Box::new((map, details, thumbs, sizes))
    } else {
        let index = Arc::new(
            name_index::NameIndex::try_new(raw.clone(), &|| false)
                .unwrap_or_else(|_| panic!("cancel")),
        );
        let details = lazy_pages::Pages::<OnceLock<Entry>>::new(n);
        let thumbs = row_cache::Cache::<String>::new(index.clone());
        let sizes = row_cache::Cache::<gust_core::dirsize::Sized>::new(index.clone());
        for i in 0..touched {
            let mut e = raw[i].clone();
            e.meta = Some(Metadata::default());
            details.cell(i).unwrap().set(e).unwrap();
            if matches!(
                case.as_str(),
                "cache-heavy" | "renewal-sparse" | "renewal-dense"
            ) {
                thumbs.insert(
                    raw[i].name.clone(),
                    format!("file:///cache/{i:08}?gust=version"),
                );
                sizes.insert(raw[i].name.clone(), gust_core::dirsize::Sized::default());
            }
        }
        if case.starts_with("renewal-") {
            for i in 0..touched {
                assert_eq!(
                    thumbs.insert(
                        raw[i].name.clone(),
                        format!("file:///cache/{i:08}?gust=renewal")
                    ),
                    row_cache::InsertResult::Changed
                );
                assert_eq!(
                    sizes.insert(
                        raw[i].name.clone(),
                        gust_core::dirsize::Sized {
                            bytes: 1,
                            files: 1,
                            complete: true
                        }
                    ),
                    row_cache::InsertResult::Changed
                );
            }
        }
        assert_eq!(
            thumbs.len(),
            if matches!(
                case.as_str(),
                "cache-heavy" | "renewal-sparse" | "renewal-dense"
            ) {
                touched
            } else {
                0
            }
        );
        Box::new((index, details, thumbs, sizes))
    };
    ACTIVE.store(false, Ordering::Relaxed);
    let peak = PEAK.load(Ordering::Relaxed);
    let live = LIVE.load(Ordering::Relaxed);
    let calls = CALLS.load(Ordering::Relaxed);
    let setup = started.elapsed().as_micros();
    let drop_us = std::thread::Builder::new()
        .name("storage-probe-recipient".into())
        .spawn(move || {
            let at = Instant::now();
            drop(retained);
            at.elapsed().as_micros()
        })
        .unwrap()
        .join()
        .unwrap();
    println!("{{\"variant\":\"{variant}\",\"case\":\"{case}\",\"rows\":{n},\"peak_requested_bytes\":{peak},\"retained_requested_bytes\":{live},\"allocations\":{calls},\"prepare_us\":{setup},\"cancel_return_ns\":{cancel_return_ns},\"cancelled_indices\":{cancelled_names},\"recipient_drop_us\":{drop_us},\"drop_thread\":\"storage-probe-recipient\"}}");
}
