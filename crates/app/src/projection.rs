//! Pane-local bounded mailboxes over process-wide, fair CPU and I/O executors.
//!
//! The GUI owns an immutable displayed frame. Requests never mutate it. The
//! executor retains that frame until the GUI has handed its predecessor back,
//! so the last large allocation is released on an executor, including shutdown.
use gust_core::entry::{Completeness, DirectorySnapshot, Entry, Metadata, SortSpec};
use gust_core::lister::CancelToken;
use gust_core::location::LocationKey;
use gust_core::projection::{self, Cancellation};
use gust_core::search::PreparedQuery;
use gust_core::selection::{RangeMode, Selection, SelectionScratch};
use std::collections::VecDeque;
use std::ffi::OsString;
use std::ops::{Deref, Index};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Instant;

#[cfg(test)]
thread_local! { static JOB_PHASE: std::cell::Cell<&'static str> = const { std::cell::Cell::new("start") }; }
#[cfg(test)]
fn job_phase(phase: &'static str) {
    JOB_PHASE.with(|current| current.set(phase));
}
const MAX_GESTURES: usize = 256;

/// Names/kinds never change. Worker-prepared sidecars allocate fixed lazy pages;
/// metadata cells and thumbnails provide stable borrowed references.
/// Directory-size reads copy their scalar value; raw entries never mutate.
pub struct Entries {
    #[cfg(test)]
    drop_notice: Mutex<Option<Arc<Mutex<Vec<String>>>>>,
    raw: Arc<Vec<Entry>>,
    prepared: bool,
    pub name_index: Arc<crate::name_index::NameIndex>,
    pub thumbs: Arc<crate::row_cache::Cache<String>>,
    pub dir_sizes: Arc<crate::row_cache::SizeCache>,
    details: crate::lazy_pages::Pages<OnceLock<Entry>>,
    pub listing: u64,
    pub load_revision: u64,
    search: Option<SearchId>,
    pub key: LocationKey,
    pub generation: u64,
    pub complete: Completeness,
    pub settled: bool,
    pub ranked: bool,
    pub risky: bool,
    pub meta_known: AtomicU64,
    pub sort_metadata_ready: bool,
    preflight: Vec<(OsString, String)>,
    pub validated_thumbnail_count: usize,
}
impl Entries {
    fn new(snapshot: DirectorySnapshot, listing: u64) -> Self {
        let raw = Arc::new(snapshot.entries);
        let index = Arc::new(crate::name_index::NameIndex::empty(raw.clone()));
        let thumbs = Arc::new(crate::row_cache::Cache::new(index.clone()));
        let dir_sizes = Arc::new(crate::row_cache::SizeCache::new(index.clone()));
        let known = raw.iter().filter(|e| e.meta.is_some()).count() as u64;
        Self {
            #[cfg(test)]
            drop_notice: Mutex::new(None),
            prepared: raw.is_empty(),
            name_index: index,
            thumbs,
            dir_sizes,
            meta_known: AtomicU64::new(known),
            sort_metadata_ready: known == raw.len() as u64,
            preflight: Vec::new(),
            validated_thumbnail_count: 0,
            details: crate::lazy_pages::Pages::new(0),
            raw,
            ranked: false,
            risky: true,
            listing,
            load_revision: 0,
            search: None,
            key: snapshot.key,
            generation: snapshot.generation,
            settled: !matches!(snapshot.complete, Completeness::Partial { .. }),
            complete: snapshot.complete,
        }
    }
    fn ready(
        self: &Arc<Self>,
        retirement: &mut Retirement,
        cancel: &impl Cancellation,
    ) -> Result<Arc<Self>, projection::ProjectionError> {
        if self.prepared {
            return Ok(self.clone());
        }
        let index = match crate::name_index::NameIndex::try_new(self.raw.clone(), cancel) {
            Ok(index) => Arc::new(index),
            Err(index) => {
                retirement.objects[0] = Some(Box::new(index));
                return Err(projection::ProjectionError::Cancelled);
            }
        };
        let details =
            match crate::lazy_pages::Pages::try_new(self.raw.len(), &|| cancel.checkpoint()) {
                Ok(details) => details,
                Err(details) => {
                    retirement.objects[0] = Some(Box::new((index, details)));
                    return Err(projection::ProjectionError::Cancelled);
                }
            };
        let thumbs = match crate::row_cache::Cache::try_new(index.clone(), cancel) {
            Ok(cache) => Arc::new(cache),
            Err(cache) => {
                retirement.objects[0] = Some(Box::new(cache));
                retirement.objects[1] = Some(Box::new(details));
                return Err(projection::ProjectionError::Cancelled);
            }
        };
        let dir_sizes = match crate::row_cache::SizeCache::try_new(index.clone(), cancel) {
            Ok(cache) => Arc::new(cache),
            Err(cache) => {
                retirement.objects[0] = Some(Box::new(cache));
                retirement.objects[1] = Some(Box::new(details));
                retirement.objects[2] = Some(Box::new(thumbs));
                return Err(projection::ProjectionError::Cancelled);
            }
        };
        for (name, url) in &self.preflight {
            if cancel.checkpoint() {
                retirement.objects[2] = Some(Box::new((thumbs, dir_sizes, details)));
                return Err(projection::ProjectionError::Cancelled);
            }
            thumbs.insert(name.clone(), url.clone());
        }
        let validated_thumbnail_count = thumbs.len();
        Ok(Arc::new(Self {
            #[cfg(test)]
            drop_notice: Mutex::new(self.drop_notice.lock().unwrap().clone()),
            raw: self.raw.clone(),
            prepared: true,
            name_index: index,
            details,
            thumbs,
            dir_sizes,
            listing: self.listing,
            load_revision: self.load_revision,
            search: self.search,
            key: self.key.clone(),
            generation: self.generation,
            complete: self.complete.clone(),
            settled: self.settled,
            ranked: self.ranked,
            risky: self.risky,
            meta_known: AtomicU64::new(self.meta_known.load(Ordering::Relaxed)),
            sort_metadata_ready: self.sort_metadata_ready,
            preflight: Vec::new(),
            validated_thumbnail_count,
        }))
    }
    pub fn get(&self, index: usize) -> Option<&Entry> {
        self.details
            .get(index)
            .and_then(OnceLock::get)
            .or_else(|| self.raw.get(index))
    }
    pub fn iter(&self) -> impl Iterator<Item = &Entry> {
        (0..self.len()).map(|i| &self[i])
    }
    pub fn patch(&self, index: usize, name: &std::ffi::OsStr, meta: Option<Metadata>) -> bool {
        let Some(entry) = self
            .raw
            .get(index)
            .filter(|e| e.name == name && e.meta.is_none())
        else {
            return false;
        };
        let Some(meta) = meta else { return false };
        // One bounded name clone per admitted patch, never a listing clone.
        let mut entry = entry.clone();
        entry.meta = Some(meta);
        let Some(cell) = self.details.cell(index) else {
            return false;
        };
        let applied = cell.set(entry).is_ok();
        if applied {
            self.meta_known.fetch_add(1, Ordering::Relaxed);
        }
        applied
    }
}
#[cfg(test)]
impl Drop for Entries {
    fn drop(&mut self) {
        if self.prepared {
            if let Some(notice) = self.drop_notice.get_mut().unwrap().as_ref() {
                notice.lock().unwrap().push(
                    std::thread::current()
                        .name()
                        .unwrap_or("unnamed")
                        .to_string(),
                );
            }
        }
    }
}
impl Deref for Entries {
    type Target = [Entry];
    fn deref(&self) -> &[Entry] {
        &self.raw
    }
}
impl Index<usize> for Entries {
    type Output = Entry;
    fn index(&self, i: usize) -> &Entry {
        self.get(i).expect("validated backing index")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Publication {
    Rows,
    Selection,
    Marks,
}
pub struct Frame {
    pub all: Arc<Entries>,
    pub thumbs: Arc<crate::row_cache::Cache<String>>,
    pub dir_sizes: Arc<crate::row_cache::SizeCache>,
    pub row_of: Arc<Vec<i32>>,
    pub clipboard: Arc<std::collections::HashSet<std::path::PathBuf>>,
    pub marked: usize,
    pub jump_paths: Vec<std::path::PathBuf>,
    pub order: Arc<Vec<usize>>,
    pub visible: Arc<Vec<usize>>,
    pub selection: Arc<Selection>,
    pub request: u64,
    pub selection_revision: u64,
    pub display_revision: u64,
    pub source_display_revision: u64,
    pub query: String,
    pub sort: SortSpec,
    pub sort_revision: u64,
    pub hidden: bool,
    pub cursor: i32,
    pub summary: String,
    pub mapping: Vec<i32>,
    pub keep: i32,
    pub row_change: bool,
    pub publication: Publication,
    pub marks_revision: u64,
    pub validated_thumbnail_count: usize,
}
impl Frame {
    pub fn empty(key: LocationKey) -> Arc<Self> {
        let snapshot = DirectorySnapshot::empty(key, 0);
        let all = Arc::new(Entries::new(snapshot, 0));
        Arc::new(Self {
            thumbs: all.thumbs.clone(),
            dir_sizes: all.dir_sizes.clone(),
            row_of: Arc::default(),
            clipboard: Arc::default(),
            marked: 0,
            jump_paths: Vec::new(),
            all,
            order: Arc::default(),
            visible: Arc::default(),
            selection: Arc::new(Selection::new()),
            request: 0,
            selection_revision: 0,
            display_revision: 0,
            source_display_revision: 0,
            query: String::new(),
            sort: SortSpec::default(),
            sort_revision: 0,
            hidden: false,
            cursor: 0,
            summary: String::new(),
            mapping: Vec::new(),
            keep: 0,
            row_change: false,
            publication: Publication::Rows,
            marks_revision: 0,
            validated_thumbnail_count: 0,
        })
    }
}
pub struct BandBase {
    pub selection: Option<Arc<Selection>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BandRect {
    pub lo: usize,
    pub hi: usize,
    pub left: usize,
    pub right: usize,
    pub cols: usize,
}
#[derive(Clone)]
pub enum Gesture {
    Toggle(OsString),
    Only(OsString),
    Clear,
    All,
    Invert,
    Range {
        from: usize,
        to: usize,
    },
    Band {
        base: Arc<BandBase>,
        all: Arc<Entries>,
        visible: Arc<Vec<usize>>,
        rectangle: BandRect,
    },
}
#[derive(Clone)]
pub struct Request {
    pub generation: u64,
    pub request: u64,
    pub selection_revision: u64,
    pub query: String,
    pub sort: SortSpec,
    pub sort_revision: u64,
    pub hidden: bool,
    pub cursor: i32,
    pub focus: Option<OsString>,
    pub keep: i32,
    pub submitted: Instant,
    pub clipboard: Arc<std::collections::HashSet<std::path::PathBuf>>,
    pub marks_revision: u64,
}
/// Search identity is independent of filter/sort/clipboard presentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchId {
    generation: u64,
    load_revision: u64,
    revision: u64,
    sort_revision: u64,
}
pub struct Load {
    pub key: LocationKey,
    pub generation: u64,
    pub sort: SortSpec,
    pub reuse: bool,
    pub live: bool,
    pub thumbnail_names: Vec<OsString>,
}
#[derive(Default)]
pub struct Metrics {
    pub started: AtomicU64,
    pub superseded: AtomicU64,
    pub cancel_observed: AtomicU64,
    pub cancel_observed_us_max: AtomicU64,
    pub latest_start_us_max: AtomicU64,
    pub completed: AtomicU64,
    pub rejected: AtomicU64,
    pub retired: AtomicU64,
    pub retire_us_max: AtomicU64,
    pub retired_selection_names_max: AtomicU64,
    pub errors: AtomicU64,
    pub search_accepted: AtomicU64,
    pub search_rejected: AtomicU64,
    pub search_metadata_us_max: AtomicU64,
    pub search_metadata_entries: AtomicU64,
}
impl Metrics {
    fn record_start(&self, submitted: Instant, now: Instant) {
        self.started.fetch_add(1, Ordering::Relaxed);
        self.latest_start_us_max.fetch_max(
            now.saturating_duration_since(submitted).as_micros() as u64,
            Ordering::Relaxed,
        );
    }
    pub fn json(&self) -> serde_json::Value {
        let read = |n: &AtomicU64| n.load(Ordering::Relaxed);
        serde_json::json!({"started":read(&self.started),"superseded":read(&self.superseded),
            "cancel_observed":read(&self.cancel_observed),"cancel_observed_us_max":read(&self.cancel_observed_us_max),
            "latest_start_us_max":read(&self.latest_start_us_max),"completed":read(&self.completed),
            "rejected":read(&self.rejected),"retired":read(&self.retired),"retire_us_max":read(&self.retire_us_max),"retired_selection_names_max":read(&self.retired_selection_names_max),"errors":read(&self.errors),"search_accepted":read(&self.search_accepted),"search_rejected":read(&self.search_rejected),"search_metadata_us_max":read(&self.search_metadata_us_max),"search_metadata_entries":read(&self.search_metadata_entries)})
    }
}
struct Event {
    revision: u64,
    submitted: Instant,
    gesture: Gesture,
}
#[derive(Default)]
struct Retirement {
    frames: [Option<Arc<Frame>>; 2],
    backing: Option<Arc<Entries>>,
    selection: Option<Selection>,
    scratch: SelectionScratch,
    objects: [Option<Box<dyn Send>>; 4],
    names: Vec<OsString>,
    gesture: Option<Event>,
    remember: bool,
    shutdown_gestures: VecDeque<Event>,
}
impl Retirement {
    fn is_empty(&self) -> bool {
        self.frames.iter().all(Option::is_none)
            && self.backing.is_none()
            && self.selection.is_none()
            && self.scratch.is_vacant()
            && self.objects.iter().all(Option::is_none)
            && self.names.capacity() == 0
            && self.gesture.is_none()
            && self.shutdown_gestures.is_empty()
    }
}
struct State {
    base: Arc<Frame>,
    desired: Request,
    marks_submitted: Instant,
    gestures: VecDeque<Event>,
    running_selection: Option<u64>,
    seed: Option<Arc<Vec<OsString>>>,
    candidate: Option<Arc<Entries>>,
    load: Option<Load>,
    load_risk: Option<bool>,
    load_id: u64,
    search_revision: u64,
    active_search: Option<SearchId>,
    search_cancel: Option<CancelToken>,
    classify_queued: bool,
    classify_running: bool,
    awaiting_load: bool,
    io_running: bool,
    io_cancel: CancelToken,
    risky_running: bool,
    risky_cancel: CancelToken,
    risky_queued: bool,
    cpu_running: bool,
    cpu_cancel: Arc<AtomicBool>,
    cancelled_at: Option<Instant>,
    completion: Option<Arc<Frame>>,
    retiring: Option<Retirement>,
    retire_running: bool,
    retire_queued: bool,
    cpu_queued: bool,
    io_queued: bool,
    publishing: bool,
    closed: bool,
    closed_cleaned: bool,
    serial: u64,
    error: Option<String>,
    shutdown_aux: Option<Box<dyn Send>>,
}
impl State {
    fn admission_time(&self, event: Option<&Event>, marks_only: bool) -> Instant {
        if let Some(event) = event {
            event.submitted
        } else if marks_only {
            self.marks_submitted
        } else {
            self.desired.submitted
        }
    }
    fn current(&self, entries: &Entries) -> bool {
        entries.generation == self.desired.generation
            && entries.load_revision == self.load_id
            && entries
                .search
                .is_none_or(|id| self.active_search == Some(id))
    }
    fn search_current(&self, id: SearchId) -> bool {
        !self.closed
            && self.active_search == Some(id)
            && id.generation == self.desired.generation
            && id.load_revision == self.load_id
    }
    fn valid(&self, f: &Frame) -> bool {
        if f.source_display_revision != self.base.display_revision {
            return false;
        }
        if f.publication == Publication::Marks {
            return self.gestures.is_empty()
                && f.marks_revision == self.desired.marks_revision
                && Arc::ptr_eq(&f.all, &self.base.all)
                && Arc::ptr_eq(&f.selection, &self.base.selection);
        }
        if f.publication == Publication::Selection {
            return self
                .gestures
                .front()
                .is_some_and(|e| e.revision == f.selection_revision)
                && Arc::ptr_eq(&f.all, &self.base.all);
        }
        self.gestures.is_empty()
            && !self.awaiting_load
            && self.current(&f.all)
            && f.marks_revision == self.desired.marks_revision
            && f.request == self.desired.request
            && f.selection_revision == self.desired.selection_revision
            && self.candidate.as_ref().map_or_else(
                || Arc::ptr_eq(&f.all, &self.base.all),
                |e| e.listing == f.all.listing && Arc::ptr_eq(&f.all.raw, &e.raw),
            )
    }
    fn dirty(&self) -> bool {
        self.current(&self.base.all)
            && (self.base.request != self.desired.request
                || self.base.query != self.desired.query
                || self.base.sort != self.desired.sort
                || self.base.sort_revision != self.desired.sort_revision
                || self.base.hidden != self.desired.hidden)
    }
}
#[cfg(test)]
type CheckpointHook = Arc<dyn Fn(usize) + Send + Sync>;
#[cfg(test)]
#[derive(Default)]
struct Hooks {
    checkpoint: Mutex<Option<CheckpointHook>>,
    retire: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    io_start: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    search: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}
struct Pane {
    #[cfg(test)]
    hooks: Hooks,
    state: Mutex<State>,
    wake: Mutex<Option<Arc<dyn Fn() -> bool + Send + Sync>>>,
    wake_queued: AtomicBool,
    metrics: Arc<Metrics>,
}
#[derive(Clone, Copy)]
enum Kind {
    Cpu,
    Io,
    RiskyIo,
    Retire,
    Classify,
}
struct Pool {
    queue: Mutex<VecDeque<Arc<Pane>>>,
    ready: Condvar,
    kind: Kind,
}
impl Pool {
    fn new(kind: Kind, name: &str, workers: usize) -> Arc<Self> {
        let pool = Arc::new(Self {
            queue: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            kind,
        });
        for n in 0..workers {
            let pool = pool.clone();
            std::thread::Builder::new()
                .name(format!("{name}-{n}"))
                .spawn(move || loop {
                    let pane = {
                        let mut queue = pool.queue.lock().unwrap_or_else(|e| e.into_inner());
                        while queue.is_empty() {
                            queue = pool.ready.wait(queue).unwrap_or_else(|e| e.into_inner());
                        }
                        queue.pop_front().unwrap()
                    };
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            match pool.kind {
                                Kind::Cpu => pane.run_cpu(),
                                Kind::Io => pane.run_io(false),
                                Kind::RiskyIo => pane.run_io(true),
                                Kind::Retire => pane.run_retire(),
                                Kind::Classify => pane.run_classify(),
                            }
                        }));
                    if result.is_err() {
                        pane.failed(pool.kind);
                    }
                })
                .expect("projection executor creation");
        }
        pool
    }
    fn enqueue(&self, pane: Arc<Pane>) {
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(pane);
        self.ready.notify_one();
    }
}
fn classifier() -> &'static Arc<Pool> {
    static POOL: OnceLock<Arc<Pool>> = OnceLock::new();
    POOL.get_or_init(|| Pool::new(Kind::Classify, "gust-classify", 1))
}
fn cpu() -> &'static Arc<Pool> {
    static POOL: OnceLock<Arc<Pool>> = OnceLock::new();
    POOL.get_or_init(|| Pool::new(Kind::Cpu, "gust-project", 2))
}
fn io() -> &'static Arc<Pool> {
    static POOL: OnceLock<Arc<Pool>> = OnceLock::new();
    POOL.get_or_init(|| Pool::new(Kind::Io, "gust-list", 2))
}
fn risky_io() -> &'static Arc<Pool> {
    static POOL: OnceLock<Arc<Pool>> = OnceLock::new();
    POOL.get_or_init(|| Pool::new(Kind::RiskyIo, "gust-risky-list", 2))
}
fn retire() -> &'static Arc<Pool> {
    static POOL: OnceLock<Arc<Pool>> = OnceLock::new();
    POOL.get_or_init(|| Pool::new(Kind::Retire, "gust-retire", 1))
}

#[derive(Clone)]
pub struct Producer {
    pane: Arc<Pane>,
}
impl Producer {
    /// Called on the existing Find worker, before sending its lightweight wake.
    pub fn find(
        &self,
        paths: Vec<std::path::PathBuf>,
        root: Option<std::path::PathBuf>,
        search: SearchId,
        cancel: &CancelToken,
    ) -> bool {
        let reject = || {
            self.pane
                .metrics
                .search_rejected
                .fetch_add(1, Ordering::Relaxed);
            false
        };
        #[cfg(test)]
        let search_hook = self.pane.hooks.search.lock().unwrap().clone();
        #[cfg(test)]
        if let Some(hook) = search_hook {
            hook();
        }
        if cancel.is_cancelled() || !self.pane.state.lock().unwrap().search_current(search) {
            return reject();
        }
        let mut snapshot = DirectorySnapshot::empty(
            root.as_ref()
                .map(LocationKey::local)
                .unwrap_or_else(|| LocationKey::local("/")),
            search.generation,
        );
        snapshot.complete = Completeness::Complete;
        for (index, path) in paths.into_iter().enumerate() {
            if cancel.is_cancelled()
                || (index % 128 == 0 && !self.pane.state.lock().unwrap().search_current(search))
            {
                return reject();
            }
            let label = root
                .as_ref()
                .and_then(|root| path.strip_prefix(root).ok())
                .unwrap_or(&path)
                .as_os_str()
                .to_os_string();
            let kind = if path.is_dir() {
                gust_core::entry::EntryKind::Dir
            } else {
                gust_core::entry::EntryKind::File
            };
            snapshot.entries.push(Entry::new(label, kind));
        }
        // The existing search worker provides immutable metadata-sort input;
        // a later size/mtime request must not reload the origin directory.
        let metadata_start = Instant::now();
        gust_core::lister::fill_metadata_unordered(&mut snapshot, cancel);
        self.pane.metrics.search_metadata_us_max.fetch_max(
            metadata_start.elapsed().as_micros() as u64,
            Ordering::Relaxed,
        );
        self.pane
            .metrics
            .search_metadata_entries
            .fetch_add(snapshot.entries.len() as u64, Ordering::Relaxed);
        let serial = loop {
            let mut state = self.pane.state.lock().unwrap();
            if cancel.is_cancelled() || !state.search_current(search) {
                return reject();
            }
            if state.candidate.is_none()
                && !state.io_running
                && state.retiring.is_none()
                && !state.retire_running
                && !state.publishing
            {
                state.serial = state.serial.wrapping_add(1);
                break state.serial;
            }
            drop(state);
            std::thread::park_timeout(std::time::Duration::from_millis(2));
        };
        let mut entries = Entries::new(snapshot, serial);
        entries.ranked = true;
        entries.search = Some(search);
        entries.load_revision = search.load_revision;
        let mut state = self.pane.state.lock().unwrap();
        if cancel.is_cancelled() || !state.search_current(search) {
            return reject();
        }
        debug_assert!(state.candidate.is_none());
        state.candidate = Some(Arc::new(entries));
        self.pane
            .metrics
            .search_accepted
            .fetch_add(1, Ordering::Relaxed);
        drop(state);
        self.pane.schedule();
        true
    }
}
pub struct Runtime {
    pane: Arc<Pane>,
}
impl Runtime {
    pub fn new(base: Arc<Frame>) -> Self {
        let desired = Request {
            generation: 0,
            request: 0,
            selection_revision: 0,
            query: String::new(),
            sort: SortSpec::default(),
            sort_revision: 0,
            hidden: false,
            cursor: 0,
            focus: None,
            keep: 0,
            submitted: Instant::now(),
            clipboard: Arc::default(),
            marks_revision: 0,
        };
        Self {
            pane: Arc::new(Pane {
                #[cfg(test)]
                hooks: Hooks::default(),
                state: Mutex::new(State {
                    base,
                    marks_submitted: desired.submitted,
                    desired,
                    gestures: VecDeque::new(),
                    running_selection: None,
                    seed: None,
                    candidate: None,
                    load: None,
                    load_risk: None,
                    load_id: 0,
                    search_revision: 0,
                    active_search: None,
                    search_cancel: None,
                    classify_queued: false,
                    classify_running: false,
                    awaiting_load: false,
                    io_running: false,
                    io_cancel: CancelToken::new(),
                    risky_running: false,
                    risky_cancel: CancelToken::new(),
                    risky_queued: false,
                    cpu_running: false,
                    cpu_cancel: Arc::new(AtomicBool::new(false)),
                    cancelled_at: None,
                    completion: None,
                    retiring: None,
                    retire_running: false,
                    retire_queued: false,
                    cpu_queued: false,
                    io_queued: false,
                    publishing: false,
                    closed: false,
                    closed_cleaned: false,
                    serial: 0,
                    error: None,
                    shutdown_aux: None,
                }),
                wake: Mutex::new(None),
                wake_queued: AtomicBool::new(false),
                metrics: Arc::default(),
            }),
        }
    }
    pub fn seed_selection(&self, names: Vec<OsString>) {
        self.pane.state.lock().unwrap().seed = Some(Arc::new(names));
    }
    pub fn subscribe(&self, wake: Arc<dyn Fn() -> bool + Send + Sync>) {
        *self.pane.wake.lock().unwrap() = Some(wake);
    }
    pub fn begin_search(&self, generation: u64, cancel: CancelToken) -> SearchId {
        let mut state = self.pane.state.lock().unwrap();
        if let Some(previous) = state.search_cancel.replace(cancel) {
            previous.cancel();
        }
        state.search_revision = state.search_revision.wrapping_add(1);
        let id = SearchId {
            generation,
            load_revision: state.load_id,
            revision: state.search_revision,
            sort_revision: state.desired.sort_revision,
        };
        state.active_search = Some(id);
        if state.cpu_running && state.gestures.is_empty() {
            self.pane.cancel(&mut state);
        }
        drop(state);
        self.pane.schedule();
        id
    }
    pub fn search_current(&self, id: SearchId) -> bool {
        self.pane.state.lock().unwrap().search_current(id)
    }
    pub fn has_current_search(&self, generation: u64) -> bool {
        let state = self.pane.state.lock().unwrap();
        state
            .active_search
            .is_some_and(|id| id.generation == generation && state.search_current(id))
    }

    #[cfg(test)]
    pub fn set_search_hook(&self, hook: Option<Arc<dyn Fn() + Send + Sync>>) {
        *self.pane.hooks.search.lock().unwrap() = hook;
    }
    pub fn producer(&self) -> Producer {
        Producer {
            pane: self.pane.clone(),
        }
    }
    pub fn metrics(&self) -> &Metrics {
        &self.pane.metrics
    }
    pub fn request(&self, request: Request) {
        let mut s = self.pane.state.lock().unwrap();
        if s.closed {
            return;
        }
        if s.cpu_running && s.gestures.is_empty() {
            self.pane.cancel(&mut s);
        }
        s.desired = request;
        drop(s);
        self.pane.schedule();
    }
    pub fn marks(&self, clipboard: Arc<std::collections::HashSet<std::path::PathBuf>>) {
        let mut state = self.pane.state.lock().unwrap();
        state.marks_submitted = Instant::now();
        state.desired.clipboard = clipboard;
        state.desired.marks_revision = state.desired.marks_revision.wrapping_add(1);
        drop(state);
        self.pane.schedule();
    }
    pub fn marks_revision(&self) -> u64 {
        self.pane.state.lock().unwrap().desired.marks_revision
    }
    pub fn load(&self, load: Load) {
        let mut s = self.pane.state.lock().unwrap();
        s.io_cancel.cancel();
        s.risky_cancel.cancel();
        if s.cpu_running && s.gestures.is_empty() {
            self.pane.cancel(&mut s);
        }
        if let Some(cancel) = s.search_cancel.take() {
            cancel.cancel();
        }
        s.active_search = None;
        s.load = Some(load);
        s.load_risk = None;
        s.load_id = s.load_id.wrapping_add(1);
        s.awaiting_load = true;
        drop(s);
        self.pane.schedule();
    }
    pub fn gesture(&self, gesture: Gesture, mut request: Request) -> Result<(), &'static str> {
        let mut s = self.pane.state.lock().unwrap();
        if s.closed {
            return Err("This pane is closing.");
        }
        if let Gesture::Band { all, visible, .. } = &gesture {
            if !Arc::ptr_eq(all, &s.base.all) || !Arc::ptr_eq(visible, &s.base.visible) {
                return Err("The displayed rows changed; start the rectangle again.");
            }
        }
        let replace_band = match (&gesture, s.gestures.back()) {
            (
                Gesture::Band {
                    base, all, visible, ..
                },
                Some(event),
            ) if s.running_selection != Some(event.revision) => {
                if !Arc::ptr_eq(all, &s.base.all) || !Arc::ptr_eq(visible, &s.base.visible) {
                    return Err("The displayed rows changed; start the rectangle again.");
                }
                matches!(&event.gesture, Gesture::Band {base: previous, ..} if Arc::ptr_eq(base,previous))
            }
            _ => false,
        };
        if !replace_band && s.gestures.len() >= MAX_GESTURES {
            return Err("Selection is busy; wait for the pending gestures to finish.");
        }
        if s.gestures.is_empty() && s.cpu_running {
            self.pane.cancel(&mut s);
        }
        let event = Event {
            revision: request.selection_revision,
            submitted: Instant::now(),
            gesture,
        };
        if replace_band {
            // Shared backing/maps are still owned by the displayed frame.
            *s.gestures.back_mut().unwrap() = event;
        } else {
            s.gestures.push_back(event);
        }
        // A selection event may update cursor/focus intent for these rows,
        // but does not re-admit the same outstanding row request.
        if request.generation == s.desired.generation && request.request == s.desired.request {
            request.submitted = s.desired.submitted;
        }
        s.desired = request;
        drop(s);
        self.pane.schedule();
        Ok(())
    }
    pub fn pending_selection(&self) -> bool {
        !self.pane.state.lock().unwrap().gestures.is_empty()
    }
    pub fn take(&self, held: bool) -> Option<Arc<Frame>> {
        self.pane.wake_queued.store(false, Ordering::Release);
        let mut s = self.pane.state.lock().unwrap();
        let valid = s
            .completion
            .as_ref()
            .is_some_and(|f| s.valid(f) && (!held || !f.row_change));
        let result = if valid && s.retiring.is_none() && !s.retire_running && !s.publishing {
            s.publishing = true;
            s.completion.take()
        } else {
            None
        };
        drop(s);
        self.pane.schedule();
        result
    }
    pub fn installed(&self, frame: Arc<Frame>, old: Arc<Frame>) {
        let mut s = self.pane.state.lock().unwrap();
        debug_assert!(s.retiring.is_none() && !s.retire_running && s.publishing);
        let gesture = if frame.publication == Publication::Selection {
            s.gestures.pop_front()
        } else {
            None
        };
        if s.candidate
            .as_ref()
            .is_some_and(|e| e.listing == frame.all.listing && Arc::ptr_eq(&e.raw, &frame.all.raw))
        {
            s.candidate = None;
        }
        let remember = frame.row_change
            && frame.keep != 2
            && !frame.all.ranked
            && frame.all.listing != old.all.listing;
        let remember_owner = remember.then(|| frame.clone());
        s.base = frame;
        s.publishing = false;
        s.retiring = Some(Retirement {
            frames: [Some(old), remember_owner],
            gesture,
            remember,
            ..Retirement::default()
        });
        drop(s);
        self.pane.schedule();
    }
    pub fn take_error(&self) -> Option<String> {
        self.pane.state.lock().unwrap().error.take()
    }
    pub fn slots(&self) -> serde_json::Value {
        let s = self.pane.state.lock().unwrap();
        serde_json::json!({"running":s.cpu_running,
        "pending":s.dirty()||s.awaiting_load,"completion":s.completion.is_some(),"candidate":s.candidate.is_some()||s.io_running||s.risky_running,"risky_running":s.risky_running,"local_running":s.io_running,
        "retiring":s.retiring.is_some()||s.retire_running,"selection_intents":s.gestures.len(),"load_revision":s.load_id,"search_revision":s.active_search.map(|id|id.revision)})
    }
    pub fn retire_selection<T: Send + Sync + 'static>(
        &self,
        old: Option<Arc<T>>,
    ) -> Result<(), Option<Arc<T>>> {
        let Some(old) = old else {
            return Ok(());
        };
        let mut state = self.pane.state.lock().unwrap();
        if state.cpu_running || state.retiring.is_some() || state.retire_running || state.publishing
        {
            return Err(Some(old));
        }
        let mut retired = Retirement::default();
        retired.objects[3] = Some(Box::new(old));
        state.retiring = Some(retired);
        drop(state);
        self.pane.schedule();
        Ok(())
    }
    pub fn close_with_aux<T: Send + 'static>(&self, aux: T) {
        self.pane.state.lock().unwrap().shutdown_aux = Some(Box::new(aux));
        self.close();
    }
    pub fn close(&self) {
        let mut s = self.pane.state.lock().unwrap();
        s.closed = true;
        s.io_cancel.cancel();
        s.risky_cancel.cancel();
        if let Some(cancel) = s.search_cancel.take() {
            cancel.cancel();
        }
        s.active_search = None;
        s.cpu_cancel.store(true, Ordering::Release);
        drop(s);
        self.pane.schedule();
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.close();
    }
}
impl Pane {
    fn cancel(&self, s: &mut State) {
        if !s.cpu_cancel.swap(true, Ordering::AcqRel) {
            s.cancelled_at = Some(Instant::now());
            self.metrics.superseded.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn schedule(self: &Arc<Self>) {
        let mut s = self.state.lock().unwrap();
        if s.retiring.is_some() && !s.retire_running && !s.retire_queued {
            s.retire_queued = true;
            retire().enqueue(self.clone());
        }
        let cleanup_free = s.retiring.is_none() && !s.retire_running;
        let stale = s.completion.as_ref().is_some_and(|f| !s.valid(f))
            || s.candidate.as_ref().is_some_and(|e| !s.current(e));
        let work = s.closed && !s.closed_cleaned
            || stale
            || s.completion.is_none()
                && (!s.gestures.is_empty()
                    || s.base.marks_revision != s.desired.marks_revision
                    || !s.awaiting_load && (s.candidate.is_some() || s.dirty()));
        if cleanup_free && !s.cpu_running && !s.cpu_queued && !s.publishing && work {
            s.cpu_queued = true;
            cpu().enqueue(self.clone());
        }
        // Local I/O retains independent capacity when both risky workers stall.
        if !s.closed
            && s.load.is_some()
            && s.candidate.is_none()
            && cleanup_free
            && s.completion.is_none()
            && !s.cpu_running
        {
            let Some(risky) = s.load_risk else {
                if !s.classify_running && !s.classify_queued {
                    s.classify_queued = true;
                    classifier().enqueue(self.clone());
                }
                return;
            };
            if risky {
                if !s.risky_running && !s.risky_queued {
                    s.risky_queued = true;
                    risky_io().enqueue(self.clone());
                }
            } else if !s.io_running && !s.io_queued {
                s.io_queued = true;
                io().enqueue(self.clone());
            }
        }
    }
    fn notify(&self) {
        if !self.wake_queued.swap(true, Ordering::AcqRel) {
            if let Some(wake) = self.wake.lock().unwrap().as_ref() {
                if !wake() {
                    self.wake_queued.store(false, Ordering::Release);
                }
            }
        }
    }
    fn run_retire(self: &Arc<Self>) {
        let value = {
            let mut s = self.state.lock().unwrap();
            s.retire_queued = false;
            let Some(value) = s.retiring.take() else {
                return;
            };
            s.retire_running = true;
            value
        };
        #[cfg(test)]
        if let Some(hook) = self.hooks.retire.lock().unwrap().clone() {
            hook();
        }
        let started = Instant::now();
        if value.remember {
            if let Some(frame) = &value.frames[1] {
                crate::folder_jump::remember_paths(&frame.jump_paths);
            }
        }
        drop(value);
        self.metrics
            .retire_us_max
            .fetch_max(started.elapsed().as_micros() as u64, Ordering::Relaxed);
        self.metrics.retired.fetch_add(1, Ordering::Relaxed);
        let mut s = self.state.lock().unwrap();
        s.retire_running = false;
        drop(s);
        self.schedule();
        self.notify();
    }
    fn run_classify(self: &Arc<Self>) {
        let (key, id) = {
            let mut state = self.state.lock().unwrap();
            state.classify_queued = false;
            if state.closed || state.classify_running {
                return;
            }
            let Some(load) = state.load.as_ref() else {
                return;
            };
            let key = load.key.clone();
            state.classify_running = true;
            (key, state.load_id)
        };
        let risky = key.as_path().is_some_and(gust_core::stall::needs_probe);
        let mut state = self.state.lock().unwrap();
        state.classify_running = false;
        if state.load_id == id && state.load.is_some() {
            state.load_risk = Some(risky);
        }
        drop(state);
        self.schedule();
    }
    fn run_io(self: &Arc<Self>, risky: bool) {
        let (load, cancel, serial, load_id) = {
            let mut s = self.state.lock().unwrap();
            if risky {
                s.risky_queued = false;
            } else {
                s.io_queued = false;
            }
            if s.closed
                || s.candidate.is_some()
                || s.completion.is_some()
                || s.publishing
                || s.cpu_running
                || s.retiring.is_some()
                || s.retire_running
            {
                return;
            }
            let matches = s.load_risk == Some(risky) && s.load.is_some();
            if !matches {
                drop(s);
                self.schedule();
                return;
            }
            if if risky { s.risky_running } else { s.io_running } {
                return;
            }
            let load = s.load.take().unwrap();
            s.serial += 1;
            let cancel = CancelToken::new();
            if risky {
                s.risky_running = true;
                s.risky_cancel = cancel.clone();
            } else {
                s.io_running = true;
                s.io_cancel = cancel.clone();
            }
            (load, cancel, s.serial, s.load_id)
        };
        #[cfg(test)]
        if let Some(hook) = self.hooks.io_start.lock().unwrap().clone() {
            hook();
        }
        let classified = load
            .key
            .as_path()
            .is_some_and(gust_core::stall::needs_probe);
        if classified != risky {
            let mut state = self.state.lock().unwrap();
            if risky {
                state.risky_running = false;
            } else {
                state.io_running = false;
            }
            if state.load_id == load_id && !state.closed && !cancel.is_cancelled() {
                state.load = Some(load);
                state.load_risk = Some(classified);
            }
            drop(state);
            self.schedule();
            return;
        }
        let mut snapshot = if let Some(path) = load.key.as_path() {
            let reach = gust_core::stall::probe_if_risky(
                path,
                std::time::Duration::from_millis(gust_core::stall::DEFAULT_PROBE_MS),
            );
            if !reach.is_responsive() {
                let mut snapshot = DirectorySnapshot::empty(load.key.clone(), load.generation);
                snapshot.complete = Completeness::Failed {
                    error: reach.message(&gust_core::stall::place_name_for(path)),
                };
                snapshot.settled = true;
                snapshot
            } else {
                let _ = gust_ops::engine::sweep_stale_staging(path);
                crate::directory_cache::list_unordered(
                    &load.key,
                    load.generation,
                    &cancel,
                    load.reuse,
                )
            }
        } else {
            crate::directory_cache::list_unordered(&load.key, load.generation, &cancel, load.reuse)
        };
        snapshot.sort = load.sort;
        if snapshot.sort_needs_metadata() {
            gust_core::lister::fill_metadata_unordered(&mut snapshot, &cancel);
        } else if load.live {
            // Preserve live folder-total invalidation without sorting on an
            // I/O worker. Viewport details use the revision-safe sidecar pass.
            if let Some(root) = snapshot.key.as_path() {
                let mut read = 0;
                for entry in &mut snapshot.entries {
                    if cancel.is_cancelled() {
                        break;
                    }
                    if entry.kind == gust_core::entry::EntryKind::Dir {
                        entry.meta = gust_core::lister::stat_entry(root, &entry.name, entry.kind);
                        read += 1;
                        if read == 2000 {
                            break;
                        }
                    }
                }
            }
        }
        let mut preflight = Vec::new();
        if load.live {
            if let Some(root) = snapshot.key.as_path() {
                for name in load.thumbnail_names.iter().take(256) {
                    if cancel.is_cancelled() {
                        break;
                    }
                    if let Some(thumbnail) = gust_thumbs::render::cached_thumbnail(
                        &root.join(name),
                        gust_thumbs::ThumbSize::Normal,
                    ) {
                        preflight.push((name.clone(), thumbnail.url()));
                    }
                }
            }
        }
        let mut s = self.state.lock().unwrap();
        if risky {
            s.risky_running = false;
        } else {
            s.io_running = false;
        }
        let accept = !s.closed
            && !cancel.is_cancelled()
            && snapshot.generation == s.desired.generation
            && load_id == s.load_id
            && s.candidate.is_none();
        drop(s);
        if !accept {
            drop(snapshot);
            self.schedule();
            self.notify();
            return;
        }
        let mut entries = Entries::new(snapshot, serial);
        entries.risky = risky;
        entries.preflight = preflight;
        entries.load_revision = load_id;
        let entries = Arc::new(entries);
        let mut s = self.state.lock().unwrap();
        if !s.closed && !cancel.is_cancelled() && s.current(&entries) && s.candidate.is_none() {
            s.candidate = Some(entries);
            s.awaiting_load = false;
        }
        drop(s);
        self.schedule();
        self.notify();
    }
    fn run_cpu(self: &Arc<Self>) {
        let (base, entries, mut request, event, cancel, seed, marks_only, submitted) = {
            let mut s = self.state.lock().unwrap();
            s.cpu_queued = false;
            if s.cpu_running || s.publishing || s.retiring.is_some() || s.retire_running {
                return;
            }
            if s.closed {
                let key = s.base.all.key.clone();
                let base = std::mem::replace(&mut s.base, Frame::empty(key));
                let completion = s.completion.take();
                let backing = s.candidate.take();
                let events = std::mem::take(&mut s.gestures);
                let seed = s.seed.take();
                let aux = s.shutdown_aux.take();
                s.load = None;
                s.closed_cleaned = true;
                let mut retirement = Retirement {
                    frames: [Some(base), completion],
                    backing,
                    shutdown_gestures: events,
                    ..Retirement::default()
                };
                retirement.objects[2] = aux;
                retirement.objects[3] = seed.map(|seed| Box::new(seed) as Box<dyn Send>);
                s.retiring = Some(retirement);
                drop(s);
                self.schedule();
                return;
            }
            let old_completion = s.completion.as_ref().is_some_and(|frame| !s.valid(frame));
            let old_candidate = s
                .candidate
                .as_ref()
                .is_some_and(|entries| !s.current(entries));
            if old_completion || old_candidate {
                let frame = if old_completion {
                    s.completion.take()
                } else {
                    None
                };
                let backing = if old_candidate {
                    s.candidate.take()
                } else {
                    None
                };
                s.retiring = Some(Retirement {
                    frames: [frame, None],
                    backing,
                    ..Retirement::default()
                });
                self.metrics.rejected.fetch_add(1, Ordering::Relaxed);
                drop(s);
                self.schedule();
                return;
            }
            if s.completion.is_some() {
                return;
            }
            let event = s.gestures.front().map(|e| Event {
                revision: e.revision,
                submitted: e.submitted,
                gesture: e.gesture.clone(),
            });
            let marks_only = event.is_none() && s.base.marks_revision != s.desired.marks_revision;
            let entries = if event.is_some() || marks_only {
                s.base.all.clone()
            } else {
                if s.awaiting_load {
                    return;
                }
                s.candidate.clone().unwrap_or_else(|| s.base.all.clone())
            };
            if event.is_none()
                && !marks_only
                && (entries.generation != s.desired.generation
                    || (s.candidate.is_none() && !s.dirty()))
            {
                return;
            }
            let submitted = s.admission_time(event.as_ref(), marks_only);
            s.running_selection = event.as_ref().map(|event| event.revision);
            s.cpu_running = true;
            s.cpu_cancel = Arc::new(AtomicBool::new(false));
            s.cancelled_at = None;
            (
                s.base.clone(),
                entries,
                s.desired.clone(),
                event,
                s.cpu_cancel.clone(),
                s.seed.clone(),
                marks_only,
                submitted,
            )
        };
        let selection_only = event.is_some();
        if let Some(event) = &event {
            request.selection_revision = event.revision;
        }
        self.metrics.record_start(submitted, Instant::now());
        let observed = AtomicBool::new(false);
        let observed_delay = AtomicU64::new(0);
        #[cfg(test)]
        let checkpoints = std::sync::atomic::AtomicUsize::new(0);
        #[cfg(test)]
        let hook = self.hooks.checkpoint.lock().unwrap().clone();
        let checkpoint = || {
            #[cfg(test)]
            {
                if let Some(hook) = &hook {
                    hook(checkpoints.fetch_add(1, Ordering::Relaxed));
                }
            }
            let cancelled = cancel.load(Ordering::Acquire);
            if cancelled && !observed.swap(true, Ordering::Relaxed) {
                let s = self.state.lock().unwrap();
                if let Some(at) = s.cancelled_at {
                    observed_delay.store(at.elapsed().as_micros() as u64, Ordering::Relaxed);
                }
            }
            cancelled
        };
        // This job pre-admitted the pane's sole retirement envelope before any
        // clone. Core hands partial/old HashSets back instead of freeing them.
        let mut retirement = Retirement::default();
        let result = if marks_only {
            prepare_marks(&base, &request, &checkpoint)
        } else {
            prepare(
                &base,
                entries,
                &request,
                event.as_ref().map(|e| &e.gesture),
                seed.as_deref().map(Vec::as_slice),
                &mut retirement,
                &checkpoint,
            )
        };
        drop(base);
        drop(seed);
        retirement.gesture = event;
        let mut s = self.state.lock().unwrap();
        s.cpu_running = false;
        s.running_selection = None;
        if observed.load(Ordering::Relaxed) {
            self.metrics.cancel_observed.fetch_add(1, Ordering::Relaxed);
            self.metrics
                .cancel_observed_us_max
                .fetch_max(observed_delay.load(Ordering::Relaxed), Ordering::Relaxed);
        }
        match result {
            Ok(frame) => {
                if !s.closed && !cancel.load(Ordering::Acquire) && s.valid(&frame) {
                    if !selection_only && !marks_only {
                        if let Some(seed) = s.seed.take() {
                            retirement.objects[3] = Some(Box::new(seed));
                        }
                    }
                    s.completion = Some(Arc::new(frame));
                    self.metrics.completed.fetch_add(1, Ordering::Relaxed);
                } else {
                    retirement.frames[0] = Some(Arc::new(frame));
                    self.metrics.rejected.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(projection::ProjectionError::Cancelled) => {
                self.metrics.rejected.fetch_add(1, Ordering::Relaxed);
            }
            Err(error) => {
                s.error = Some(format!("Could not prepare listing: {error:?}"));
                self.metrics.errors.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.metrics
            .retired_selection_names_max
            .fetch_max(retirement.scratch.tracked_names() as u64, Ordering::Relaxed);
        if !retirement.is_empty() {
            s.retiring = Some(retirement);
        }
        drop(s);
        self.schedule();
        self.notify();
    }
    fn failed(self: &Arc<Self>, kind: Kind) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match kind {
            Kind::Cpu => s.cpu_running = false,
            Kind::Io => {
                s.io_running = false;
                s.awaiting_load = false;
            }
            Kind::RiskyIo => {
                s.risky_running = false;
                s.awaiting_load = false;
            }
            Kind::Retire => s.retire_running = false,
            Kind::Classify => s.classify_running = false,
        }
        s.error = Some("The listing worker stopped unexpectedly; reload to try again.".into());
        // Suppress automatic panic retry. A new user request remains possible.
        s.desired.request = s.base.request;
        s.desired.query = s.base.query.clone();
        s.desired.sort = s.base.sort;
        s.desired.hidden = s.base.hidden;
        self.metrics.errors.fetch_add(1, Ordering::Relaxed);
        drop(s);
        self.notify();
    }
}
fn prepare(
    base: &Frame,
    entries: Arc<Entries>,
    request: &Request,
    gesture: Option<&Gesture>,
    seed: Option<&[OsString]>,
    retirement: &mut Retirement,
    cancel: &impl Cancellation,
) -> Result<Frame, projection::ProjectionError> {
    #[cfg(test)]
    job_phase("ready");
    let entries = entries.ready(retirement, cancel)?;
    if !Arc::ptr_eq(&base.all, &entries) {
        retirement.backing = Some(entries.clone());
    }
    let selection_only = gesture.is_some();
    let same = Arc::ptr_eq(&base.all, &entries);
    #[cfg(test)]
    job_phase("sort");
    let order = if selection_only
        || same && base.sort == request.sort && base.sort_revision == request.sort_revision
    {
        base.order.clone()
    } else if entries.ranked
        && !same
        && entries
            .search
            .is_none_or(|id| id.sort_revision == request.sort_revision)
    {
        let mut order = Vec::with_capacity(entries.len());
        for i in 0..entries.len() {
            if i % 128 == 0 && cancel.checkpoint() {
                return Err(projection::ProjectionError::Cancelled);
            }
            order.push(i);
        }
        Arc::new(order)
    } else {
        Arc::new(projection::sort_indices(&entries, request.sort, cancel)?)
    };
    #[cfg(test)]
    job_phase("filter");
    let visible = if selection_only {
        base.visible.clone()
    } else {
        Arc::new(projection::filter_indices(
            &entries,
            &order,
            &PreparedQuery::new(&request.query),
            request.hidden,
            cancel,
        )?)
    };
    if !same && entries.key == base.all.key {
        for (row, entry) in entries.iter().enumerate() {
            if row % 128 == 0 && cancel.checkpoint() {
                return Err(projection::ProjectionError::Cancelled);
            }
            if let Some(&old) = base.all.name_index.get(&entry.name) {
                let previous = &base.all[old];
                if entry.meta.is_some() && entry.meta == previous.meta {
                    // Thumbnail stamp validation includes facts beyond listing
                    // metadata. A new backing starts empty; the cache-first
                    // thumbnail worker revalidates its version before reuse.
                    if let Some(size) = base.dir_sizes.get(&entry.name) {
                        entries.dir_sizes.insert(entry.name.clone(), size);
                    }
                }
            }
        }
    }
    let row_of = if selection_only {
        base.row_of.clone()
    } else {
        let mut row_of = vec![-1; entries.len()];
        for (row, &index) in visible.iter().enumerate() {
            if row % 128 == 0 && cancel.checkpoint() {
                return Err(projection::ProjectionError::Cancelled);
            }
            row_of[index] = i32::try_from(row)
                .map_err(|_| projection::ProjectionError::TooManyRows { len: visible.len() })?;
        }
        Arc::new(row_of)
    };
    let mut jump_paths = Vec::new();
    if !same && !entries.ranked {
        if let Some(root) = entries.key.as_path() {
            jump_paths.push(root.to_path_buf());
            for (row, &index) in order.iter().enumerate() {
                if row % 128 == 0 && cancel.checkpoint() {
                    return Err(projection::ProjectionError::Cancelled);
                }
                let entry = &entries[index];
                if entry.kind == gust_core::entry::EntryKind::Dir && !entry.is_hidden() {
                    jump_paths.push(root.join(&entry.name));
                    if jump_paths.len() == 257 {
                        break;
                    }
                }
            }
        }
    }
    let marked = if selection_only {
        base.marked
    } else {
        count_marks(&entries, &request.clipboard, cancel)?
    };
    #[cfg(test)]
    job_phase("mapping");
    let mapping = if selection_only {
        Vec::new()
    } else {
        projection::map_visible_rows(&base.all, &base.visible, &entries, &visible, cancel)?
    };
    let cursor = if selection_only {
        request.cursor
    } else {
        let mut focus = None;
        if let Some(name) = &request.focus {
            for (row, &i) in visible.iter().enumerate() {
                if row % 128 == 0 && cancel.checkpoint() {
                    return Err(projection::ProjectionError::Cancelled);
                }
                if entries[i].name == *name {
                    focus = Some(row as i32);
                    break;
                }
            }
        }
        focus
            .or_else(|| {
                if request.keep != 0 {
                    usize::try_from(request.cursor)
                        .ok()
                        .and_then(|i| mapping.get(i).copied())
                        .filter(|i| *i >= 0)
                } else {
                    None
                }
            })
            .unwrap_or_else(|| {
                if request.keep == 0 {
                    0
                } else {
                    request.cursor.min((visible.len() as i32 - 1).max(0))
                }
            })
    };
    let fresh = base.selection.is_untouched()
        || entries.key != base.all.key
        || (entries.ranked && !same)
        || matches!(
            gesture,
            Some(Gesture::Clear | Gesture::Only(_) | Gesture::All | Gesture::Band { .. })
        );
    #[cfg(test)]
    job_phase("selection");
    let selection = if fresh {
        Selection::new()
    } else {
        base.selection.clone_retain_visible_with_scratch(
            &entries,
            &visible,
            &mut retirement.scratch,
            cancel,
        )?
    };
    let mut selection = Some(selection);
    // On any later cancellation preserve the prepared Selection too. No large
    // error-path destructor can hide between observation and newest admission.
    let mut bounded_summary = None;
    let result = (|| {
        let current = selection.as_mut().unwrap();
        if let Some(names) = seed {
            current.select_all_by_name_cancellable_with_scratch(
                names,
                &mut retirement.scratch,
                cancel,
            )?;
        }
        if let Some(gesture) = gesture {
            match gesture {
                Gesture::Toggle(name) => current.toggle(name),
                Gesture::Only(name) => current.select_only(name),
                Gesture::Clear => {}
                Gesture::Band {
                    base: band,
                    all,
                    visible,
                    rectangle,
                } => {
                    if let Some(band) = &band.selection {
                        for (row, &i) in base.visible.iter().enumerate() {
                            if row % 128 == 0 && cancel.checkpoint() {
                                return Err(projection::ProjectionError::Cancelled);
                            }
                            let name = &base.all[i].name;
                            if band.contains(name) {
                                retirement.names.push(name.clone());
                            }
                        }
                    }
                    let mut picked = Vec::new();
                    let bounded = band
                        .selection
                        .as_ref()
                        .is_none_or(|selection| selection.is_empty());
                    let (mut known, mut bytes) = (true, 0u64);
                    for row in rectangle.lo..=rectangle.hi {
                        if cancel.checkpoint() {
                            retirement.objects[3] = Some(Box::new(picked));
                            return Err(projection::ProjectionError::Cancelled);
                        }
                        for col in rectangle.left..=rectangle.right {
                            if let Some(&index) =
                                visible.get(row.saturating_mul(rectangle.cols).saturating_add(col))
                            {
                                picked.push(all[index].name.clone());
                                if bounded {
                                    if let Some(meta) = all[index].meta {
                                        bytes = bytes.saturating_add(meta.size)
                                    } else {
                                        known = false
                                    }
                                }
                            }
                        }
                    }
                    let applied = current.set_band_cancellable_with_scratch(
                        &retirement.names,
                        &picked,
                        &mut retirement.scratch,
                        cancel,
                    );
                    retirement.objects[3] = Some(Box::new(picked));
                    applied?;
                    if bounded {
                        bounded_summary = Some((known, bytes));
                    }
                }
                _ => {
                    retirement.names.reserve(base.visible.len());
                    for (row, &i) in base.visible.iter().enumerate() {
                        if row % 128 == 0 && cancel.checkpoint() {
                            return Err(projection::ProjectionError::Cancelled);
                        }
                        retirement.names.push(base.all[i].name.clone());
                    }
                    match gesture {
                        Gesture::All => current.select_all_by_name_cancellable_with_scratch(
                            &retirement.names,
                            &mut retirement.scratch,
                            cancel,
                        )?,
                        Gesture::Invert => current.invert_by_name_cancellable_with_scratch(
                            &retirement.names,
                            &mut retirement.scratch,
                            cancel,
                        )?,
                        Gesture::Range { from, to } => current
                            .extend_by_name_cancellable_with_scratch(
                                &retirement.names,
                                *from,
                                *to,
                                RangeMode::Replace,
                                &mut retirement.scratch,
                                cancel,
                            )?,
                        _ => unreachable!(),
                    }
                }
            }
        }
        #[cfg(test)]
        job_phase("summary");
        let mut known = true;
        let mut bytes = 0u64;
        if let Some((prepared_known, prepared_bytes)) = bounded_summary {
            known = prepared_known;
            bytes = prepared_bytes;
        } else if let Some(Gesture::Only(name)) = gesture {
            if let Some(&i) = entries.name_index.get(name) {
                if let Some(meta) = entries[i].meta {
                    bytes = meta.size;
                } else {
                    known = false;
                }
            }
        } else if !current.is_empty() {
            for (row, &i) in visible.iter().enumerate() {
                if row % 128 == 0 && cancel.checkpoint() {
                    return Err(projection::ProjectionError::Cancelled);
                }
                if current.contains(&entries[i].name) {
                    if let Some(meta) = entries[i].meta {
                        bytes = bytes.saturating_add(meta.size);
                    } else {
                        known = false;
                    }
                }
            }
        }
        let summary = current.summary(if known { bytes } else { 0 });
        Ok(Frame {
            thumbs: entries.thumbs.clone(),
            dir_sizes: entries.dir_sizes.clone(),
            row_of,
            validated_thumbnail_count: entries.validated_thumbnail_count,
            clipboard: if selection_only {
                base.clipboard.clone()
            } else {
                request.clipboard.clone()
            },
            marked,
            jump_paths,
            all: entries,
            order,
            visible,
            selection: Arc::new(selection.take().unwrap()),
            request: if selection_only {
                base.request
            } else {
                request.request
            },
            selection_revision: request.selection_revision,
            source_display_revision: base.display_revision,
            display_revision: base.display_revision + 1,
            query: if selection_only {
                base.query.clone()
            } else {
                request.query.clone()
            },
            sort: if selection_only {
                base.sort
            } else {
                request.sort
            },
            sort_revision: if selection_only {
                base.sort_revision
            } else {
                request.sort_revision
            },
            hidden: if selection_only {
                base.hidden
            } else {
                request.hidden
            },
            cursor,
            summary,
            mapping,
            keep: if selection_only {
                base.keep
            } else {
                request.keep
            },
            row_change: !selection_only,
            publication: if selection_only {
                Publication::Selection
            } else {
                Publication::Rows
            },
            marks_revision: if selection_only {
                base.marks_revision
            } else {
                request.marks_revision
            },
        })
    })();
    if let Some(selection) = selection {
        retirement.selection = Some(selection);
    }
    result
}

fn count_marks(
    entries: &Entries,
    clipboard: &std::collections::HashSet<std::path::PathBuf>,
    cancel: &impl Cancellation,
) -> Result<usize, projection::ProjectionError> {
    let Some(root) = entries.key.as_path() else {
        return Ok(0);
    };
    let mut count = 0;
    for (i, path) in clipboard.iter().enumerate() {
        if i % 128 == 0 && cancel.checkpoint() {
            return Err(projection::ProjectionError::Cancelled);
        }
        if path
            .strip_prefix(root)
            .ok()
            .is_some_and(|name| entries.name_index.contains_key(name.as_os_str()))
        {
            count += 1;
        }
    }
    Ok(count)
}
fn prepare_marks(
    base: &Frame,
    request: &Request,
    cancel: &impl Cancellation,
) -> Result<Frame, projection::ProjectionError> {
    Ok(Frame {
        all: base.all.clone(),
        thumbs: base.thumbs.clone(),
        dir_sizes: base.dir_sizes.clone(),
        row_of: base.row_of.clone(),
        clipboard: request.clipboard.clone(),
        marked: count_marks(&base.all, &request.clipboard, cancel)?,
        jump_paths: Vec::new(),
        order: base.order.clone(),
        visible: base.visible.clone(),
        selection: base.selection.clone(),
        request: base.request,
        selection_revision: base.selection_revision,
        display_revision: base.display_revision + 1,
        source_display_revision: base.display_revision,
        query: base.query.clone(),
        sort: base.sort,
        sort_revision: base.sort_revision,
        hidden: base.hidden,
        cursor: base.cursor,
        validated_thumbnail_count: base.validated_thumbnail_count,
        summary: base.summary.clone(),
        mapping: Vec::new(),
        keep: base.keep,
        row_change: false,
        publication: Publication::Marks,
        marks_revision: request.marks_revision,
    })
}

#[cfg(test)]
#[path = "projection_tests.rs"]
mod tests;
