//! Process-owned interactive preferences. Only the worker calls the backend.
//!
//! Admission is bounded and updates a shared optimistic snapshot. Pending
//! intents are keyed by field/location (last admitted value wins), rather than
//! snapshots from individual tabs. Acknowledgements live here, independently
//! of whether a Qt receiver survives delivery. No operation journals belong here.
use gust_core::{
    entry::SortField,
    location::LocationKey,
    session::Session,
    settings::{RendererChoice, Settings},
    sorts::SortMemory,
    views::{ViewMemory, ViewMode},
};
use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak},
    time::{Duration, Instant},
};

const MAX_PENDING: usize = 2048;
const MAX_SUBSCRIBERS: usize = 256;
const RENDERER_WAIT: Duration = Duration::from_millis(50);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Document {
    Settings,
    Sorts,
    Views,
    Session,
}
impl Document {
    fn limit(self) -> u64 {
        match self {
            Self::Settings | Self::Session => 64 * 1024,
            _ => 512 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Mutation {
    Setting(String, String),
    Sort(LocationKey, SortField, bool),
    View(LocationKey, ViewMode),
    Session(Session),
}
impl Mutation {
    fn key(&self) -> (Document, String) {
        match self {
            Self::Setting(key, _) => (Document::Settings, key.clone()),
            Self::Sort(key, ..) => (Document::Sorts, key.to_token()),
            Self::View(key, _) => (Document::Views, key.to_token()),
            Self::Session(_) => (Document::Session, String::new()),
        }
    }
    fn apply(&self, snapshot: &mut Snapshot) -> Result<(), String> {
        match self {
            Self::Setting(key, value) => snapshot.settings.set(key, value)?,
            Self::Sort(key, field, ascending) => snapshot.sorts.remember(key, *field, *ascending),
            Self::View(key, view) => snapshot.views.remember(key, *view),
            Self::Session(session) => snapshot.session = session.clone(),
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct Integration {
    pub is_default: bool,
    pub omarchy_keys: bool,
    pub file_dialogs: bool,
    pub is_omarchy: bool,
    pub pictures: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub ready: bool,
    pub revision: u64,
    pub saved_revision: u64,
    /// Worker attempts completed, including refusals/failures with identical text.
    pub completed_attempt: u64,
    pub pending: bool,
    pub error: String,
    /// Rejected choices were never admitted; flush cannot retry them.
    pub admission_error: String,
    pub settings: Settings,
    pub sorts: SortMemory,
    pub views: ViewMemory,
    /// Startup restore snapshot; disk existence checks happen on the worker.
    pub session: Session,
    pub integration: Integration,
}

type Callback = Arc<dyn Fn() -> bool + Send + Sync>;
type Intents = BTreeMap<(Document, String), (u64, Mutation)>;
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum AdmissionKey {
    Mutation((Document, String)),
    Subscription,
}
struct State {
    snapshot: Snapshot,
    startup_renderer: Option<RendererChoice>,
    intents: Intents,
    admission_errors: BTreeMap<AdmissionKey, String>,
    admission_overflow: bool,
    wake: u64,
    stop: bool,
    subscribers: BTreeMap<u64, Callback>,
    next_subscriber: u64,
}
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

/// Dropping a model removes its callback. Failed queues also retire receivers.
pub struct Subscription {
    shared: Weak<Shared>,
    id: u64,
}
impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.upgrade() {
            lock(&shared.state).subscribers.remove(&self.id);
        }
    }
}

pub struct Preferences {
    shared: Arc<Shared>,
}
impl Preferences {
    fn start(backend: Box<dyn Backend>) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                snapshot: Snapshot::default(),
                startup_renderer: None,
                intents: BTreeMap::new(),
                admission_errors: BTreeMap::new(),
                admission_overflow: false,
                wake: 1,
                stop: false,
                subscribers: BTreeMap::new(),
                next_subscriber: 0,
            }),
            changed: Condvar::new(),
        });
        let worker = shared.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("gust-preferences".into())
            .spawn(move || run(worker, backend))
        {
            let mut state = lock(&shared.state);
            state.snapshot.ready = true;
            state.startup_renderer = Some(RendererChoice::Auto);
            state.snapshot.error = format!("cannot start preferences worker: {error}");
            state.stop = true;
        }
        Self { shared }
    }

    pub fn snapshot(&self) -> Snapshot {
        let state = lock(&self.shared.state);
        let mut snapshot = state.snapshot.clone();
        if !snapshot.admission_error.is_empty() {
            if !snapshot.error.is_empty() {
                snapshot.error.push_str("; ");
            }
            snapshot.error.push_str(&snapshot.admission_error);
        }
        snapshot
    }

    /// Empty error from a bridge setter means *admitted*, never *saved*.
    pub fn admit(&self, mutation: Mutation) -> Result<u64, String> {
        let result = {
            let mut state = lock(&self.shared.state);
            let key = mutation.key();
            let result: Result<u64, String> = (|| {
                if state.stop {
                    return Err("preferences worker is stopped".into());
                }
                if !state.snapshot.ready {
                    return Err("preferences are still loading".into());
                }
                if !state.intents.contains_key(&key) && state.intents.len() >= MAX_PENDING {
                    return Err(
                        "preferences queue is full; wait for saving, then reselect this choice"
                            .into(),
                    );
                }
                if matches!(&mutation, Mutation::Session(_))
                    && !state.snapshot.settings.restore_tabs
                {
                    return Err("session restore is disabled".into());
                }
                let revision = state
                    .snapshot
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| "preferences revision exhausted".to_string())?;
                mutation.apply(&mut state.snapshot)?;
                state.snapshot.revision = revision;
                state.snapshot.pending = true;
                state.intents.insert(key.clone(), (revision, mutation));
                state.wake = state.wake.wrapping_add(1);
                Ok(revision)
            })();
            match &result {
                Ok(_) => {
                    state
                        .admission_errors
                        .remove(&AdmissionKey::Mutation(key.clone()));
                }
                Err(error) => record_rejection(&mut state, AdmissionKey::Mutation(key), error),
            }
            update_admission_error(&mut state);
            result
        };
        self.shared.changed.notify_all();
        notify(&self.shared);
        result
    }

    /// Coalesces repeated requests into one worker wake; never waits for I/O.
    /// Callers compare saved_revision with this target and enforce their own
    /// UI deadline. Failure keeps the target pending and exposes error.
    pub fn flush(&self) -> u64 {
        let target = {
            let mut state = lock(&self.shared.state);
            state.wake = state.wake.wrapping_add(1);
            state.snapshot.revision
        };
        self.shared.changed.notify_all();
        target
    }

    /// Explicit refresh also observes external edits while idle. No timer polls.
    pub fn refresh(&self) {
        self.flush();
    }

    pub fn subscribe(&self, callback: Callback) -> Result<Subscription, String> {
        let mut state = lock(&self.shared.state);
        if state.subscribers.len() >= MAX_SUBSCRIBERS {
            let error = "too many preferences receivers; retry subscription".to_string();
            record_rejection(&mut state, AdmissionKey::Subscription, &error);
            update_admission_error(&mut state);
            drop(state);
            notify(&self.shared);
            return Err(error);
        }
        state.next_subscriber = state
            .next_subscriber
            .checked_add(1)
            .ok_or_else(|| "preferences receiver IDs exhausted".to_string())?;
        let recovered = state
            .admission_errors
            .remove(&AdmissionKey::Subscription)
            .is_some();
        update_admission_error(&mut state);
        let id = state.next_subscriber;
        state.subscribers.insert(id, callback);
        drop(state);
        if recovered {
            notify(&self.shared);
        }
        Ok(Subscription {
            shared: Arc::downgrade(&self.shared),
            id,
        })
    }

    /// Before Qt construction only. An explicit environment override needs no
    /// settings read. Otherwise give the worker one bounded opportunity; the
    /// returned choice is fixed for this process even if settings arrive late.
    pub fn renderer_before_qt(&self) -> RendererChoice {
        self.renderer_with_env(|key| std::env::var(key).ok())
    }

    fn renderer_with_env(&self, env: impl Fn(&str) -> Option<String>) -> RendererChoice {
        let explicit = matches!(
            env("GUST_RENDERER").as_deref().map(str::trim),
            Some("software" | "gpu" | "hardware")
        ) || ["QT_QUICK_BACKEND", "QSG_RHI_BACKEND"]
            .iter()
            .any(|key| env(key).is_some_and(|value| !value.trim().is_empty()));
        let mut state = lock(&self.shared.state);
        if explicit {
            state.startup_renderer = Some(RendererChoice::Auto);
            return RendererChoice::Auto;
        }
        let deadline = Instant::now() + RENDERER_WAIT;
        while state.startup_renderer.is_none() {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            state = self
                .shared
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        *state.startup_renderer.get_or_insert(RendererChoice::Auto)
    }

    /// Last-chance process exit drain. QML should flush with its event loop
    /// alive first. This has a deadline and never joins a blocked I/O thread.
    pub fn shutdown(&self, timeout: Duration) -> bool {
        let target = self.flush();
        let deadline = Instant::now() + timeout;
        let mut state = lock(&self.shared.state);
        while !state.snapshot.ready || state.snapshot.saved_revision < target {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            let (next, result) = self
                .shared
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next;
            if result.timed_out() {
                break;
            }
        }
        let saved = state.snapshot.ready && state.snapshot.saved_revision >= target;
        state.stop = true;
        drop(state);
        self.shared.changed.notify_all();
        saved
    }
}

fn record_rejection(state: &mut State, key: AdmissionKey, error: &str) {
    if state.admission_errors.contains_key(&key) || state.admission_errors.len() < MAX_PENDING {
        state.admission_errors.insert(
            key,
            format!("Choice not accepted: {error}; reselect it to save"),
        );
    } else {
        // Do not grow an unbounded error ledger or report overflowed choices
        // as saved. This exceptional latch requires restarting the process.
        state.admission_overflow = true;
    }
}
fn update_admission_error(state: &mut State) {
    state.snapshot.admission_error = state
        .admission_errors
        .values()
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    if state.admission_overflow {
        state
            .snapshot
            .admission_error
            .push_str("; too many rejected choices; restart and reselect unsaved choices");
    }
}

pub fn process() -> &'static Preferences {
    static OWNER: OnceLock<Preferences> = OnceLock::new();
    OWNER.get_or_init(|| Preferences::start(Box::new(Disk::default())))
}

fn notify(shared: &Shared) {
    let subscribers: Vec<_> = lock(&shared.state)
        .subscribers
        .iter()
        .map(|(id, callback)| (*id, callback.clone()))
        .collect();
    for (id, callback) in subscribers {
        if !callback() {
            lock(&shared.state).subscribers.remove(&id);
        }
    }
    shared.changed.notify_all();
}

trait Backend: Send {
    /// Worker-only nonblocking admission for the entire read/reconcile/publish
    /// batch. Read-only refreshes never call this or create a lock file.
    fn begin_write(&mut self, _documents: &[Document]) -> Result<(), String> {
        Ok(())
    }
    fn end_write(&mut self) {}
    fn read(&mut self, document: Document) -> Result<Option<String>, String>;
    /// Must refuse publication if the source changed since read.
    fn publish(
        &mut self,
        document: Document,
        previous: Option<&str>,
        text: &str,
    ) -> Result<(), String>;
    fn integration(&mut self) -> Integration {
        Integration::default()
    }
    fn restore(&mut self, session: Session) -> Session {
        session
    }
}

fn decode(texts: &BTreeMap<Document, Option<String>>) -> Snapshot {
    let text = |document| {
        texts
            .get(&document)
            .and_then(|s| s.as_deref())
            .unwrap_or("")
    };
    Snapshot {
        settings: Settings::from_text(text(Document::Settings)),
        sorts: SortMemory::from_text(text(Document::Sorts)),
        views: ViewMemory::from_text(text(Document::Views)),
        session: Session::from_text(&session_body(text(Document::Session))),
        ..Snapshot::default()
    }
}

/// Remove only comments emitted by Session itself. Older writers appended
/// another header on every save, eventually consuming the core parser's line
/// budget. Raw reads remain bounded before this normalization; unknown comments
/// and fields retain their order and bytes (apart from normalized line endings).
fn session_body(text: &str) -> String {
    let canonical = Session::default().to_text();
    let headers: Vec<_> = canonical
        .lines()
        .take_while(|line| line.starts_with('#'))
        .collect();
    text.lines()
        .filter(|line| !headers.contains(&line.trim()))
        .map(|line| format!("{line}\n"))
        .collect()
}

/// Replace just one recognized field, retaining comments/unknown fields and
/// external choices elsewhere. Duplicate occurrences of the touched key are
/// replaced together so an old trailing value cannot override the intent.
fn patch(text: &str, key: &str, value: Option<&str>) -> String {
    let mut out = String::new();
    for line in text.lines() {
        if line.split_once('=').is_some_and(|(k, _)| k.trim() == key) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if let Some(value) = value {
        out.push_str(&format!("{key} = {value}\n"));
    }
    out
}

fn reconcile(
    texts: &mut BTreeMap<Document, Option<String>>,
    intents: &Intents,
) -> Result<Snapshot, String> {
    let mut snapshot = decode(texts);
    let mut ordered: Vec<_> = intents.values().collect();
    ordered.sort_unstable_by_key(|(revision, _)| *revision);
    for (_, mutation) in ordered {
        mutation.apply(&mut snapshot)?;
        let (document, key) = mutation.key();
        let old = texts
            .get(&document)
            .and_then(|s| s.as_deref())
            .unwrap_or("");
        let text = match mutation {
            Mutation::Setting(_, _) => {
                let canonical = snapshot.settings.to_text();
                let value = canonical.lines().find_map(|l| {
                    l.split_once('=')
                        .filter(|(k, _)| k.trim() == key)
                        .map(|(_, v)| v.trim())
                });
                patch(old, &key, value)
            }
            Mutation::Sort(..) => canonical_prefix(old, &snapshot.sorts.to_text(), |line| {
                replaced_location_record(line, &key, document)
            }),
            Mutation::View(..) => canonical_prefix(old, &snapshot.views.to_text(), |line| {
                replaced_location_record(line, &key, document)
            }),
            Mutation::Session(session) => canonical_prefix(old, &session.to_text(), |line| {
                line.split_once('=')
                    .is_some_and(|(key, _)| matches!(key.trim(), "active" | "tab"))
            }),
        };
        if text.len() as u64 > document.limit() {
            return Err("preferences conflict: reconciled document exceeds its size limit".into());
        }
        texts.insert(document, Some(text));
    }
    // Check the complete batch once per touched document, after all replay
    // and eviction. Never acknowledge bytes that the unchanged core decoders
    // would silently interpret as a different set of known choices.
    for document in [
        Document::Settings,
        Document::Sorts,
        Document::Views,
        Document::Session,
    ] {
        if intents.keys().any(|(doc, _)| *doc == document) {
            let text = texts
                .get(&document)
                .and_then(|text| text.as_deref())
                .unwrap_or("");
            validate_serialized(document, text, &snapshot)?;
        }
    }
    Ok(snapshot)
}

/// Every line-bounded format writes its retained recognized state first, then
/// preserves unknown content in relative order. Derive owned headers from the
/// canonical writer so repeated saves cannot accumulate them. Callers remove
/// old recognized records, including evicted entries and overwritten unknown
/// values at an explicitly touched key; ignored content never gains authority.
fn canonical_prefix(old: &str, canonical: &str, replaced: impl Fn(&str) -> bool) -> String {
    let headers: Vec<_> = canonical
        .lines()
        .take_while(|line| line.starts_with('#'))
        .collect();
    let mut out = canonical.to_string();
    for line in old.lines() {
        if !headers.contains(&line.trim()) && !replaced(line) {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn replaced_location_record(line: &str, touched: &str, document: Document) -> bool {
    line.split_once('=').is_some_and(|(key, value)| {
        if key.trim() == touched {
            return true;
        }
        // Match core parsing, not PathToken admission: these tokens remember
        // preferences and do not authorize filesystem operations. Invalid or
        // future token/value syntax remains preserved unknown content.
        let recognized = match document {
            Document::Sorts => {
                gust_core::sorts::parse_sort(value.trim().trim_matches('"')).is_some()
            }
            Document::Views => ViewMode::parse(value.trim().trim_matches('"')).is_some(),
            _ => false,
        };
        recognized && LocationKey::from_token(key.trim()).is_some()
    })
}

fn validate_serialized(document: Document, text: &str, expected: &Snapshot) -> Result<(), String> {
    let consistent = match document {
        Document::Settings => Settings::from_text(text) == expected.settings,
        Document::Sorts => SortMemory::from_text(text).to_text() == expected.sorts.to_text(),
        Document::Views => ViewMemory::from_text(text).to_text() == expected.views.to_text(),
        Document::Session => Session::from_text(text) == expected.session,
    };
    if consistent {
        Ok(())
    } else {
        Err(format!("preferences conflict: {document:?} serialization does not preserve known choices; retry flush"))
    }
}

fn run(shared: Arc<Shared>, mut backend: Box<dyn Backend>) {
    let mut observed = 0;
    // Only successful reads/publications replace a document's source cache.
    // None is a successful observation of deletion, distinct from an I/O error.
    let mut last_good = BTreeMap::new();
    loop {
        let (wake, intents, target, attempt) = {
            let mut state = lock(&shared.state);
            while !state.stop && state.wake == observed {
                state = shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            if state.stop {
                return;
            }
            (
                state.wake,
                state.intents.clone(),
                state.snapshot.revision,
                state.snapshot.completed_attempt.saturating_add(1),
            )
        };
        observed = wake;
        if !intents.is_empty() {
            let documents: Vec<_> = intents.keys().map(|(doc, _)| *doc).collect();
            if let Err(error) = backend.begin_write(&documents) {
                {
                    let mut state = lock(&shared.state);
                    state.snapshot.error = error;
                    state.snapshot.completed_attempt = attempt;
                }
                notify(&shared);
                continue;
            }
        }
        let mut texts = last_good.clone();
        let mut errors = Vec::new();
        let mut session_read_failed = false;
        for document in [
            Document::Settings,
            Document::Sorts,
            Document::Views,
            Document::Session,
        ] {
            match backend.read(document) {
                Ok(text) => {
                    last_good.insert(document, text.clone());
                    texts.insert(document, text);
                }
                Err(error) => {
                    session_read_failed |= document == Document::Session;
                    errors.push(format!("{document:?}: {error}"));
                }
            }
            if document == Document::Settings {
                let mut state = lock(&shared.state);
                if state.startup_renderer.is_none() {
                    let text = texts
                        .get(&document)
                        .and_then(|t| t.as_deref())
                        .unwrap_or("");
                    state.startup_renderer = Some(Settings::from_text(text).renderer);
                    shared.changed.notify_all();
                }
            }
        }
        let previous = texts.clone();
        let mut next = if errors.is_empty() {
            match reconcile(&mut texts, &intents) {
                Ok(next) => next,
                Err(error) => {
                    errors.push(error);
                    decode(&previous)
                }
            }
        } else {
            decode(&previous)
        };
        // Settings publish first: a session cannot be acknowledged durable
        // ahead of the restore-tabs intent that authorized keeping it.
        if errors.is_empty() {
            for document in [
                Document::Settings,
                Document::Sorts,
                Document::Views,
                Document::Session,
            ] {
                if !intents.keys().any(|(doc, _)| *doc == document) {
                    continue;
                }
                if document == Document::Session && !next.settings.restore_tabs {
                    continue;
                }
                let old = previous.get(&document).and_then(|s| s.as_deref());
                let text = texts
                    .get(&document)
                    .and_then(|s| s.as_deref())
                    .unwrap_or("");
                if let Err(error) = backend.publish(document, old, text) {
                    errors.push(format!("{document:?}: {error}"));
                    break;
                }
                last_good.insert(document, Some(text.to_string()));
            }
        }
        // Drop advisory ownership before slow integration/session probes.
        backend.end_write();
        next.integration = backend.integration();
        next.session = if session_read_failed {
            // Keep the previously restored public session, including accepted
            // choices, rather than reviving paths pruned by an earlier restore.
            lock(&shared.state).snapshot.session.clone()
        } else {
            backend.restore(next.session)
        };
        {
            let mut state = lock(&shared.state);
            if errors.is_empty() {
                state.intents.retain(|_, (revision, _)| *revision > target);
                next.saved_revision = target;
            } else {
                next.saved_revision = state.snapshot.saved_revision;
            }
            // Replay *all* remaining intents on failure, including a newer
            // value replacing an in-flight key. No stale copy reaches disk.
            let mut remaining: Vec<_> = state.intents.values().collect();
            remaining.sort_unstable_by_key(|(revision, _)| *revision);
            for (_, mutation) in remaining {
                let _ = mutation.apply(&mut next);
            }
            next.completed_attempt = attempt;
            next.ready = true;
            next.revision = state.snapshot.revision;
            next.pending = !state.intents.is_empty();
            next.error = errors.join("; ");
            next.admission_error = state.snapshot.admission_error.clone();
            state.snapshot = next;
        }
        notify(&shared);
    }
}

#[derive(Default)]
struct Disk {
    paths: Option<BTreeMap<Document, PathBuf>>,
    batch: Option<DiskBatch>,
}
impl Disk {
    fn paths(&mut self) -> &BTreeMap<Document, PathBuf> {
        self.paths.get_or_insert_with(|| {
            [
                (Document::Settings, Settings::default_path()),
                (Document::Sorts, SortMemory::default_path()),
                (Document::Views, ViewMemory::default_path()),
                (Document::Session, Session::default_path()),
            ]
            .into_iter()
            .filter_map(|(doc, path)| path.map(|path| (doc, path)))
            .collect()
        })
    }
}

/// Each held parent and lock inode survives through source reads and rename.
/// Locks are permanent coordination files: unlinking one would split owners.
struct DiskBatch {
    parents: BTreeMap<Document, (std::fs::File, std::ffi::OsString)>,
    _locks: Vec<std::fs::File>,
}
const LOCK_NAME: &str = ".gust-preferences.lock";
fn owner_lock(dir: &std::fs::File) -> Result<std::fs::File, String> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let uid = rustix::process::geteuid().as_raw();
    let parent = dir.metadata().map_err(|e| e.to_string())?;
    if parent.uid() != uid || parent.mode() & 0o022 != 0 {
        return Err("unsafe preferences lock parent ownership or permissions".into());
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW | rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open(descriptor_path(dir).join(LOCK_NAME))
        .map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file()
        || metadata.uid() != uid
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
    {
        return Err("unsafe preferences lock file ownership, type or permissions".into());
    }
    rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive).map_err(
        |error| {
            if error == rustix::io::Errno::WOULDBLOCK {
                "preferences busy in another Gust process; retry flush".to_string()
            } else {
                format!("cannot lock preferences: {error}")
            }
        },
    )?;
    Ok(file)
}

// Holding each parent descriptor avoids following a swapped/symlink ancestor.
const O_NOFOLLOW: i32 = rustix::fs::OFlags::NOFOLLOW.bits() as i32;
const O_DIRECTORY: i32 = rustix::fs::OFlags::DIRECTORY.bits() as i32;
fn descriptor_path(file: &std::fs::File) -> PathBuf {
    use std::os::fd::AsRawFd;
    PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
}
fn parent(path: &Path, create: bool) -> io::Result<(std::fs::File, std::ffi::OsString)> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("missing preference filename"))?
        .to_os_string();
    let mut dir = std::fs::File::open("/")?;
    for component in path
        .parent()
        .ok_or_else(|| io::Error::other("missing preference parent"))?
        .components()
    {
        match component {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(name) => {
                let child = descriptor_path(&dir).join(name);
                if create {
                    match std::fs::DirBuilder::new().mode(0o700).create(&child) {
                        Ok(()) => (),
                        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
                        Err(error) => return Err(error),
                    }
                }
                dir = std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(O_NOFOLLOW | O_DIRECTORY)
                    .open(child)?;
            }
            _ => return Err(io::Error::other("unsafe preference ancestor")),
        }
    }
    Ok((dir, name))
}
fn read_at(path: &Path, limit: u64) -> io::Result<Option<String>> {
    use std::os::unix::fs::OpenOptionsExt;
    // O_NONBLOCK prevents an attacker-controlled FIFO from blocking before
    // the regular-file check. The worker may block on ordinary disk I/O.
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW | rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(io::Error::other("unsafe or oversized preference file"));
    }
    let mut text = String::new();
    file.take(limit + 1).read_to_string(&mut text)?;
    if text.len() as u64 > limit {
        return Err(io::Error::other("oversized preference file"));
    }
    Ok(Some(text))
}
impl Backend for Disk {
    fn begin_write(&mut self, documents: &[Document]) -> Result<(), String> {
        use std::os::unix::fs::MetadataExt;
        self.batch = None;
        let mut documents: std::collections::BTreeSet<_> = documents.iter().copied().collect();
        // Session publication depends on the reconciled restore-tabs setting.
        if documents.contains(&Document::Session) {
            documents.insert(Document::Settings);
        }
        let mut parents = BTreeMap::new();
        let mut locked = std::collections::BTreeSet::new();
        let mut locks = Vec::new();
        for document in documents {
            let path = self
                .paths()
                .get(&document)
                .ok_or("no home folder to keep preferences in")?;
            let (dir, name) = parent(path, true).map_err(|e| e.to_string())?;
            let metadata = dir.metadata().map_err(|e| e.to_string())?;
            if locked.insert((metadata.dev(), metadata.ino())) {
                locks.push(owner_lock(&dir)?);
            }
            parents.insert(document, (dir, name));
        }
        self.batch = Some(DiskBatch {
            parents,
            _locks: locks,
        });
        Ok(())
    }
    fn end_write(&mut self) {
        self.batch = None;
    }
    fn read(&mut self, document: Document) -> Result<Option<String>, String> {
        if let Some((dir, name)) = self.batch.as_ref().and_then(|b| b.parents.get(&document)) {
            return read_at(&descriptor_path(dir).join(name), document.limit())
                .map_err(|e| e.to_string());
        }
        let Some(path) = self.paths().get(&document) else {
            return Ok(None);
        };
        let (dir, name) = match parent(path, false) {
            Ok(parent) => parent,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        read_at(&descriptor_path(&dir).join(name), document.limit()).map_err(|e| e.to_string())
    }
    fn publish(
        &mut self,
        document: Document,
        previous: Option<&str>,
        text: &str,
    ) -> Result<(), String> {
        let (dir, name) = self
            .batch
            .as_ref()
            .and_then(|b| b.parents.get(&document))
            .ok_or("preference publication requires worker batch ownership")?;
        publish_at(dir, name, document, previous, text, std::fs::File::sync_all)
    }
    fn integration(&mut self) -> Integration {
        let bindings = gust_core::omarchy_keys::bindings_path();
        Integration {
            is_default: gust_core::defaults::mimeapps_path()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .map(|text| gust_core::defaults::MimeApps::parse(&text))
                .and_then(|m| m.get("inode/directory"))
                .as_deref()
                == Some(gust_core::defaults::GUST_DESKTOP),
            omarchy_keys: bindings
                .as_ref()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .is_some_and(|text| gust_core::omarchy_keys::has_block(&text)),
            file_dialogs: gust_core::portal_conf::paths()
                .and_then(|(p, _)| std::fs::read_to_string(p).ok())
                .is_some_and(|text| gust_core::portal_conf::is_claimed(&text)),
            is_omarchy: bindings.is_some_and(|p| p.exists()),
            pictures: pictures_directory(),
        }
    }
    fn restore(&mut self, session: Session) -> Session {
        session.still_there()
    }
}
#[cfg(test)]
fn publish_file(
    path: &Path,
    document: Document,
    previous: Option<&str>,
    text: &str,
    sync: impl FnOnce(&std::fs::File) -> io::Result<()>,
) -> Result<(), String> {
    let (dir, name) = parent(path, true).map_err(|e| e.to_string())?;
    publish_at(&dir, &name, document, previous, text, sync)
}
fn publish_at(
    dir: &std::fs::File,
    name: &std::ffi::OsStr,
    document: Document,
    previous: Option<&str>,
    text: &str,
    sync: impl FnOnce(&std::fs::File) -> io::Result<()>,
) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    static STAGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let target = descriptor_path(dir).join(name);
    let stage = descriptor_path(dir).join(format!(
        ".gust-prefs-{}-{}.tmp",
        std::process::id(),
        STAGE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW)
        .open(&stage)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        sync(&file).map_err(|e| e.to_string())?;
        if read_at(&target, document.limit())
            .map_err(|e| e.to_string())?
            .as_deref()
            != previous
        {
            return Err(
                "preferences conflict: file changed during publication; retry flush".into(),
            );
        }
        std::fs::rename(&stage, &target).map_err(|e| e.to_string())
    })();
    drop(file);
    if result.is_err() {
        let _ = std::fs::remove_file(stage);
    }
    // The staged file is synced; we make no parent-directory fsync claim.
    result
}

fn pictures_directory() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_PICTURES_DIR").filter(|p| !p.is_empty()) {
        return Some(path.into());
    }
    let home = PathBuf::from(std::env::var_os("HOME").filter(|p| !p.is_empty())?);
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let text = read_at(&config.join("user-dirs.dirs"), 64 * 1024).ok()??;
    for line in text.lines() {
        if let Some(value) = line.trim().strip_prefix("XDG_PICTURES_DIR=") {
            let value = value.trim().trim_matches('"');
            return Some(
                value
                    .strip_prefix("$HOME/")
                    .map_or_else(|| PathBuf::from(value), |tail| home.join(tail)),
            );
        }
    }
    let conventional = home.join("Pictures");
    conventional.is_dir().then_some(conventional)
}

#[cfg(test)]
#[path = "preferences_tests.rs"]
mod tests;
