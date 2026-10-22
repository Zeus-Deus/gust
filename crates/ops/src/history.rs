//! Durable, typed, worker-owned multi-step undo and redo.
use crate::{
    anchor::{self, DirHandle, FileIdentity},
    journal::{Job, JobKind, JobState, RecordedIdentity},
};
use gust_core::lister::CancelToken;
use rustix::fs::{Mode, OFlags};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::File,
    io::{self, Read, Write},
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryKind {
    Copy,
    Move,
    Trash,
    Rename,
    Create,
    Extract,
    Compress,
    Attributes,
    Barrier,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryStatus {
    Applied,
    Undone,
    Retired,
    Interrupted,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub label: String,
    pub kind: HistoryKind,
    pub status: HistoryStatus,
    pub created_unix: i64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryError {
    Empty,
    Cancelled,
    Occupied(PathBuf),
    Changed(PathBuf),
    NewWork(PathBuf),
    IdentityUnknown(PathBuf),
    Barrier(String),
    Unsupported(String),
    Io(String),
}
impl std::fmt::Display for HistoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("There is no history step in that direction."),
            Self::Cancelled => f.write_str("Recovery was cancelled."),
            Self::Occupied(p) => write!(f, "{} is occupied; all work was preserved", p.display()),
            Self::Changed(p) => write!(f, "{} changed since; all work was preserved", p.display()),
            Self::NewWork(p) => write!(f, "{} is new work; all work was preserved", p.display()),
            Self::IdentityUnknown(p) => {
                write!(f, "Cannot establish the identity of {}", p.display())
            }
            Self::Barrier(id) => write!(
                f,
                "Recovery is fenced by {id}; inspect the recorded intent before continuing."
            ),
            Self::Unsupported(s) | Self::Io(s) => f.write_str(s),
        }
    }
}
impl std::error::Error for HistoryError {}
impl From<rustix::io::Errno> for HistoryError {
    fn from(e: rustix::io::Errno) -> Self {
        io::Error::from(e).into()
    }
}
impl From<io::Error> for HistoryError {
    fn from(e: io::Error) -> Self {
        Self::Io(e.to_string())
    }
}
#[derive(Debug, Default)]
pub struct HistoryOutcome {
    pub completed: Vec<HistoryEntry>,
    pub refusal: Option<HistoryError>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SavedPath(Vec<u8>);
impl SavedPath {
    fn new(path: &Path) -> Self {
        Self(path.as_os_str().as_bytes().to_vec())
    }
    fn path(&self) -> PathBuf {
        OsString::from_vec(self.0.clone()).into()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TreeNode {
    relative: SavedPath,
    identity: RecordedIdentity,
    mode: u32,
}
type Tree = Vec<TreeNode>;
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Parent {
    path: SavedPath,
    dev: u64,
    ino: u64,
}
impl Parent {
    fn of(path: &Path) -> Result<Self, HistoryError> {
        let dir = crate::history_tree::open_dir(path)?;
        let s = dir.stat_self()?;
        Ok(Self {
            path: SavedPath::new(path),
            dev: s.st_dev,
            ino: s.st_ino,
        })
    }
    fn open(&self) -> Result<DirHandle, HistoryError> {
        let dir = crate::history_tree::open_dir(&self.path.path())?;
        let s = dir.stat_self()?;
        if (s.st_dev, s.st_ino) != (self.dev, self.ino) {
            return Err(HistoryError::Changed(self.path.path()));
        }
        Ok(dir)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
enum Attr {
    Mode(u32),
    Owner(u32, u32),
    Tags(Option<Vec<u8>>),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Step {
    Move {
        present: SavedPath,
        restore: Option<SavedPath>,
        present_parent: Parent,
        restore_parent: Option<Parent>,
        vault: Option<Parent>,
        tree: Tree,
    },
    Attr {
        path: SavedPath,
        parent: Parent,
        identity: RecordedIdentity,
        before: Attr,
        after: Attr,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    entry: HistoryEntry,
    steps: Vec<Step>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Pending {
    id: String,
    undo: bool,
    step: usize,
    from: SavedPath,
    to: SavedPath,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Store {
    version: u32,
    records: Vec<Record>,
    retired: Vec<Record>,
    cursor: usize,
    pending: Option<Pending>,
}
/// Open and drop this owner around one worker command, so other processes
/// can acquire it between commands. No filesystem I/O belongs on a GUI poll.
pub struct History {
    directory: DirHandle,
    name: OsString,
    _lock: File,
    store: Store,
    state_identity: Option<FileIdentity>,
}
fn checkpoint(cancel: &CancelToken) -> Result<(), HistoryError> {
    if cancel.is_cancelled() {
        Err(HistoryError::Cancelled)
    } else {
        Ok(())
    }
}
fn unique() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}
fn owned_regular(file: &File) -> io::Result<()> {
    let s = rustix::fs::fstat(file)?;
    if s.st_mode & 0o170000 != 0o100000
        || s.st_uid != rustix::process::geteuid().as_raw()
        || s.st_mode & 0o077 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "History must be an owned private regular file.",
        ));
    }
    Ok(())
}
impl History {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("History needs an absolute parent."))?;
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::other("History needs a file name."))?
            .to_os_string();
        let directory = crate::history_tree::ensure_dir(parent)?;
        let parent_stat = directory.stat_self()?;
        if parent_stat.st_uid != rustix::process::geteuid().as_raw()
            || parent_stat.st_mode & 0o022 != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "History folder must be owned and not writable by others.",
            ));
        }
        let mut lock_name = name.clone();
        lock_name.push(".lock");
        let lock = File::from(rustix::fs::openat(
            directory.as_fd(),
            &lock_name,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::from_bits_truncate(0o600),
        )?);
        owned_regular(&lock)?;
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)?;
        let (store, state_identity) = match rustix::fs::openat(
            directory.as_fd(),
            &name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => {
                let file = File::from(fd);
                owned_regular(&file)?;
                let identity = FileIdentity::from_stat(&rustix::fs::fstat(&file)?);
                let mut bytes = Vec::new();
                file.take(32 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
                if bytes.len() > 32 * 1024 * 1024 {
                    return Err(io::Error::other("History exceeds its storage bound."));
                }
                let store: Store = serde_json::from_slice(&bytes)?;
                if store.version != 1 || store.cursor > store.records.len() {
                    return Err(io::Error::other("Unsupported or damaged history."));
                }
                (store, Some(identity))
            }
            Err(rustix::io::Errno::NOENT) => (
                Store {
                    version: 1,
                    ..Default::default()
                },
                None,
            ),
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            directory,
            name,
            _lock: lock,
            store,
            state_identity,
        })
    }
    fn save(&mut self) -> Result<(), HistoryError> {
        match (self.state_identity, self.directory.stat_child(&self.name)) {
            (Some(old), Ok(st)) if old.matches(&FileIdentity::from_stat(&st)) => {}
            (None, Err(e)) if e.kind() == io::ErrorKind::NotFound => {}
            _ => return Err(HistoryError::Changed(PathBuf::from(&self.name))),
        }
        let bytes = serde_json::to_vec(&self.store).map_err(io::Error::from)?;
        if bytes.len() > 32 * 1024 * 1024 {
            return Err(HistoryError::Unsupported(
                "History storage bound reached; preserved objects were retained.".into(),
            ));
        }
        let temp = OsString::from(format!(".history-next-{}", unique()));
        let result = (|| -> Result<FileIdentity, HistoryError> {
            let mut file = File::from(rustix::fs::openat(
                self.directory.as_fd(),
                &temp,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )?);
            file.write_all(&bytes)?;
            file.sync_all()?;
            let id = FileIdentity::from_stat(&rustix::fs::fstat(&file)?);
            rustix::fs::renameat(
                self.directory.as_fd(),
                &temp,
                self.directory.as_fd(),
                &self.name,
            )?;
            rustix::fs::fsync(self.directory.as_fd())?;
            Ok(id)
        })();
        match result {
            Ok(id) => {
                self.state_identity = Some(id);
                Ok(())
            }
            Err(e) => {
                let _ = rustix::fs::unlinkat(
                    self.directory.as_fd(),
                    &temp,
                    rustix::fs::AtFlags::empty(),
                );
                Err(e)
            }
        }
    }
    pub fn entries(&self) -> Vec<HistoryEntry> {
        let mut out: Vec<_> = self
            .store
            .records
            .iter()
            .chain(&self.store.retired)
            .map(|r| r.entry.clone())
            .collect();
        if let Some(p) = &self.store.pending {
            if let Some(entry) = out.iter_mut().find(|e| e.id == p.id) {
                entry.status = HistoryStatus::Interrupted;
            }
        }
        out
    }
    fn fence(&self) -> Result<(), HistoryError> {
        if let Some(p) = &self.store.pending {
            Err(HistoryError::Barrier(p.id.clone()))
        } else {
            Ok(())
        }
    }
    fn admit(
        &mut self,
        label: &str,
        kind: HistoryKind,
        steps: Vec<Step>,
    ) -> Result<HistoryEntry, HistoryError> {
        self.fence()?;
        if label.is_empty()
            || label.len() > 4096
            || self.store.records.len() + self.store.retired.len() >= 1000
        {
            return Err(HistoryError::Unsupported(
                "History label/record limit reached.".into(),
            ));
        }
        if steps.is_empty() && kind != HistoryKind::Barrier {
            return Err(HistoryError::Empty);
        }
        let old = self.store.clone();
        let retired = self.store.records.split_off(self.store.cursor);
        self.store.retired.extend(retired.into_iter().map(|mut r| {
            r.entry.status = HistoryStatus::Retired;
            r
        }));
        let entry = HistoryEntry {
            id: unique(),
            label: label.into(),
            kind,
            status: HistoryStatus::Applied,
            created_unix: crate::journal::now_unix(),
        };
        self.store.records.push(Record {
            entry: entry.clone(),
            steps,
        });
        self.store.cursor = self.store.records.len();
        if let Err(e) = self.save() {
            self.store = old;
            return Err(e);
        }
        Ok(entry)
    }
    pub fn record_barrier(&mut self, label: &str) -> Result<HistoryEntry, HistoryError> {
        self.admit(label, HistoryKind::Barrier, Vec::new())
    }
    pub fn record_created(
        &mut self,
        label: &str,
        created: &[crate::create::Created],
    ) -> Result<HistoryEntry, HistoryError> {
        let mut steps = Vec::new();
        for c in created {
            let id = FileIdentity::of_path(&c.path)?;
            verify_ident(&c.path, c.ident, &id)?;
            if id.size != c.len || id.mode & 0o170000 != 0o100000 {
                return Err(HistoryError::Changed(c.path.clone()));
            }
            steps.push(move_step(
                &c.path,
                None,
                tree(&c.path, &CancelToken::new())?,
            )?);
        }
        self.admit(label, HistoryKind::Create, steps)
    }
    pub fn record_links(
        &mut self,
        label: &str,
        links: &[crate::attrs::LinkMade],
    ) -> Result<HistoryEntry, HistoryError> {
        let mut steps = Vec::new();
        for c in links {
            let id = FileIdentity::of_path(&c.link)?;
            verify_ident(&c.link, c.ident, &id)?;
            let (p, n) = anchor::resolve_parent(&c.link)?;
            if id.mode & 0o170000 != 0o120000
                || rustix::fs::readlinkat(p.as_fd(), n, Vec::new())?.to_bytes()
                    != c.target.as_os_str().as_bytes()
            {
                return Err(HistoryError::Changed(c.link.clone()));
            }
            steps.push(move_step(
                &c.link,
                None,
                tree(&c.link, &CancelToken::new())?,
            )?);
        }
        self.admit(label, HistoryKind::Create, steps)
    }
    pub fn record_renames(
        &mut self,
        label: &str,
        renames: &[crate::attrs::RenameMade],
    ) -> Result<HistoryEntry, HistoryError> {
        let mut steps = Vec::new();
        for c in renames {
            let expected: Tree = c
                .subtree
                .iter()
                .map(|(p, id)| {
                    Ok(TreeNode {
                        relative: SavedPath::new(
                            p.strip_prefix(&c.renamed)
                                .map_err(|_| HistoryError::IdentityUnknown(p.clone()))?,
                        ),
                        identity: (*id).into(),
                        mode: id.mode,
                    })
                })
                .collect::<Result<_, HistoryError>>()?;
            verify_tree(&c.renamed, &expected, &CancelToken::new())?;
            steps.push(move_step(&c.renamed, Some(&c.original), expected)?);
        }
        self.admit(label, HistoryKind::Rename, steps)
    }
    pub fn record_job(&mut self, label: &str, job: &Job) -> Result<HistoryEntry, HistoryError> {
        if job.state != JobState::Confirmed {
            return Err(HistoryError::Unsupported(
                "Only confirmed jobs enter reversible history.".into(),
            ));
        }
        if !job.removed_empty_dirs.is_empty() || job.entries.iter().any(|e| e.preserved.is_some()) {
            return Err(HistoryError::Unsupported(
                "This merge/replacement needs its retained legacy recovery record.".into(),
            ));
        }
        let kind = match job.kind {
            JobKind::Copy => HistoryKind::Copy,
            JobKind::Move => HistoryKind::Move,
            JobKind::Trash => {
                return Err(HistoryError::Unsupported(
                    "Trash requires its typed data/info receipt.".into(),
                ))
            }
            JobKind::Rename => HistoryKind::Rename,
            JobKind::NewFolder => HistoryKind::Create,
            JobKind::Extract => HistoryKind::Extract,
            JobKind::Compress => HistoryKind::Compress,
            JobKind::DeletePermanent => return self.record_barrier(label),
        };
        let mut steps = Vec::new();
        let preexisting: std::collections::BTreeSet<_> = job.preexisting_at_dest.iter().collect();
        for e in job.entries.iter().filter(|e| e.done) {
            let Some(path) = &e.destination else {
                return Err(HistoryError::IdentityUnknown(e.source.clone()));
            };
            if preexisting.contains(path) {
                continue;
            }
            let mut nodes = Vec::new();
            for (p, id) in job
                .subtree_identities
                .iter()
                .filter(|(p, _)| p.starts_with(path) && !preexisting.contains(p))
            {
                let now = FileIdentity::of_path(p)?;
                if !id.matches(&now) {
                    return Err(HistoryError::Changed(p.clone()));
                }
                nodes.push(TreeNode {
                    relative: SavedPath::new(
                        p.strip_prefix(path)
                            .map_err(|_| HistoryError::IdentityUnknown(p.clone()))?,
                    ),
                    identity: *id,
                    mode: now.mode,
                });
            }
            if nodes.is_empty() {
                return Err(HistoryError::IdentityUnknown(path.clone()));
            }
            verify_tree(path, &nodes, &CancelToken::new())?;
            steps.push(move_step(
                path,
                if matches!(job.kind, JobKind::Move | JobKind::Rename) {
                    Some(&e.source)
                } else {
                    None
                },
                nodes,
            )?);
        }
        self.admit(label, kind, steps)
    }
    pub fn record_modes(
        &mut self,
        label: &str,
        changes: &[crate::attrs::ModeChanged],
    ) -> Result<HistoryEntry, HistoryError> {
        let mut steps = Vec::new();
        for c in changes {
            steps.push(attr_step(
                &c.path,
                c.ident,
                Attr::Mode(c.old),
                Attr::Mode(c.new),
            )?);
        }
        self.admit(label, HistoryKind::Attributes, steps)
    }
    pub fn record_owners(
        &mut self,
        label: &str,
        changes: &[crate::attrs::OwnerChanged],
    ) -> Result<HistoryEntry, HistoryError> {
        let mut steps = Vec::new();
        for c in changes {
            steps.push(attr_step(
                &c.path,
                c.ident,
                Attr::Owner(c.old.0, c.old.1),
                Attr::Owner(c.new.0, c.new.1),
            )?);
        }
        self.admit(label, HistoryKind::Attributes, steps)
    }
    pub fn record_tags(
        &mut self,
        label: &str,
        changes: &[crate::tags::TagChanged],
    ) -> Result<HistoryEntry, HistoryError> {
        let mut steps = Vec::new();
        for c in changes {
            steps.push(attr_step(
                &c.path,
                c.ident,
                Attr::Tags(c.before.clone()),
                Attr::Tags(c.after.clone()),
            )?);
        }
        self.admit(label, HistoryKind::Attributes, steps)
    }
    pub fn undo(&mut self, cancel: &CancelToken) -> Result<HistoryEntry, HistoryError> {
        self.transition(true, cancel)
    }
    pub fn redo(&mut self, cancel: &CancelToken) -> Result<HistoryEntry, HistoryError> {
        self.transition(false, cancel)
    }
    pub fn undo_many(&mut self, count: usize, cancel: &CancelToken) -> HistoryOutcome {
        self.many(count, true, cancel)
    }
    pub fn redo_many(&mut self, count: usize, cancel: &CancelToken) -> HistoryOutcome {
        self.many(count, false, cancel)
    }
    fn many(&mut self, count: usize, undo: bool, cancel: &CancelToken) -> HistoryOutcome {
        let mut out = HistoryOutcome::default();
        for _ in 0..count {
            match self.transition(undo, cancel) {
                Ok(e) => out.completed.push(e),
                Err(e) => {
                    out.refusal = Some(e);
                    break;
                }
            }
        }
        out
    }
    fn transition(
        &mut self,
        undo: bool,
        cancel: &CancelToken,
    ) -> Result<HistoryEntry, HistoryError> {
        self.fence()?;
        checkpoint(cancel)?;
        let index = if undo {
            self.store
                .cursor
                .checked_sub(1)
                .ok_or(HistoryError::Empty)?
        } else {
            if self.store.cursor >= self.store.records.len() {
                return Err(HistoryError::Empty);
            }
            self.store.cursor
        };
        let record = self.store.records[index].clone();
        if record.entry.kind == HistoryKind::Barrier {
            return Err(HistoryError::Barrier(record.entry.id));
        }
        let indices: Vec<_> = if undo {
            (0..record.steps.len()).rev().collect()
        } else {
            (0..record.steps.len()).collect()
        };
        // Check all identities and occupancy before any inverse effect. Names
        // overlapping another step are refused, retaining the whole receipt.
        let mut occupied = std::collections::BTreeSet::new();
        for &i in &indices {
            match &record.steps[i] {
                Step::Move {
                    present,
                    restore,
                    present_parent,
                    restore_parent,
                    vault,
                    tree,
                } => {
                    let (from, to, parent) = if undo {
                        (
                            present.path(),
                            restore
                                .as_ref()
                                .map(SavedPath::path)
                                .or_else(|| vault.as_ref().map(|v| v.path.path().join("item"))),
                            present_parent,
                        )
                    } else {
                        (
                            restore
                                .as_ref()
                                .map(SavedPath::path)
                                .or_else(|| vault.as_ref().map(|v| v.path.path().join("item")))
                                .ok_or_else(|| HistoryError::IdentityUnknown(present.path()))?,
                            Some(present.path()),
                            restore_parent
                                .as_ref()
                                .or(vault.as_ref())
                                .ok_or_else(|| HistoryError::IdentityUnknown(present.path()))?,
                        )
                    };
                    parent.open()?;
                    verify_tree(&from, tree, cancel)?;
                    occupied.insert(from.clone());
                    if let Some(to) = to {
                        let (p, n) = anchor::resolve_parent(&to)?;
                        if p.stat_child(&n).is_ok() {
                            return Err(HistoryError::Occupied(to));
                        }
                        let target = if undo {
                            restore_parent.as_ref().or(vault.as_ref())
                        } else {
                            Some(present_parent)
                        };
                        target
                            .ok_or_else(|| HistoryError::IdentityUnknown(to.clone()))?
                            .open()?;
                    }
                }
                Step::Attr {
                    path,
                    parent,
                    identity,
                    before,
                    after,
                } => {
                    parent.open()?;
                    let file = open_attr(&path.path(), identity)?;
                    if read_attr(&file, if undo { after } else { before })?
                        != *(if undo { after } else { before })
                    {
                        return Err(HistoryError::Changed(path.path()));
                    }
                }
            }
        }
        let mut touched = Vec::new();
        for i in indices {
            checkpoint(cancel)?;
            let mut step = self.store.records[index].steps[i].clone();
            match &mut step {
                Step::Move {
                    present,
                    restore,
                    present_parent,
                    restore_parent,
                    vault,
                    tree,
                } => {
                    let from = if undo {
                        present.path()
                    } else {
                        restore
                            .as_ref()
                            .map(SavedPath::path)
                            .or_else(|| vault.as_ref().map(|v| v.path.path().join("item")))
                            .ok_or_else(|| HistoryError::IdentityUnknown(present.path()))?
                    };
                    let to = if undo {
                        if let Some(path) = restore.as_ref() {
                            path.path()
                        } else {
                            if vault.is_none() {
                                let base = self.vault_base(&from)?;
                                let planned = base.join(format!(".gust-undo-{}", unique()));
                                self.store.pending = Some(Pending {
                                    id: record.entry.id.clone(),
                                    undo,
                                    step: i,
                                    from: SavedPath::new(&from),
                                    to: SavedPath::new(&planned.join("item")),
                                });
                                self.save()?;
                                let (parent, name) = anchor::resolve_parent(&planned)?;
                                rustix::fs::mkdirat(
                                    parent.as_fd(),
                                    &name,
                                    Mode::from_bits_truncate(0o700),
                                )?;
                                rustix::fs::fsync(parent.as_fd())?;
                                *vault = Some(Parent::of(&planned)?);
                                if let Step::Move {
                                    vault: saved_vault, ..
                                } = &mut self.store.records[index].steps[i]
                                {
                                    *saved_vault = vault.clone();
                                }
                                self.save()?;
                            }
                            vault
                                .as_ref()
                                .ok_or_else(|| HistoryError::IdentityUnknown(from.clone()))?
                                .path
                                .path()
                                .join("item")
                        }
                    } else {
                        present.path()
                    };
                    if occupied.contains(&to) {
                        return Err(HistoryError::Occupied(to));
                    }
                    self.store.pending = Some(Pending {
                        id: record.entry.id.clone(),
                        undo,
                        step: i,
                        from: SavedPath::new(&from),
                        to: SavedPath::new(&to),
                    });
                    self.save()?;
                    let source_parent = if undo {
                        &*present_parent
                    } else {
                        restore_parent
                            .as_ref()
                            .or(vault.as_ref())
                            .ok_or_else(|| HistoryError::IdentityUnknown(from.clone()))?
                    };
                    let target_parent = if undo {
                        restore_parent
                            .as_ref()
                            .or(vault.as_ref())
                            .ok_or_else(|| HistoryError::IdentityUnknown(to.clone()))?
                    } else {
                        &*present_parent
                    };
                    let source = source_parent.open()?;
                    let target = target_parent.open()?;
                    verify_tree(&from, tree, cancel)?;
                    anchor::rename_noreplace(
                        &source,
                        from.file_name()
                            .ok_or_else(|| HistoryError::IdentityUnknown(from.clone()))?,
                        &target,
                        to.file_name()
                            .ok_or_else(|| HistoryError::IdentityUnknown(to.clone()))?,
                    )
                    .map_err(|e| {
                        if e.kind() == io::ErrorKind::AlreadyExists {
                            HistoryError::Occupied(to.clone())
                        } else {
                            e.into()
                        }
                    })?;
                    verify_tree(&to, tree, cancel)?;
                    rustix::fs::fsync(source.as_fd())?;
                    rustix::fs::fsync(target.as_fd())?;
                    touched.extend([from, to]);
                }
                Step::Attr {
                    path,
                    parent,
                    identity,
                    before,
                    after,
                } => {
                    self.store.pending = Some(Pending {
                        id: record.entry.id.clone(),
                        undo,
                        step: i,
                        from: path.clone(),
                        to: path.clone(),
                    });
                    self.save()?;
                    parent.open()?;
                    let file = open_attr(&path.path(), identity)?;
                    let (expected, value) = if undo {
                        (after, before)
                    } else {
                        (before, after)
                    };
                    if read_attr(&file, expected)? != *expected {
                        return Err(HistoryError::Changed(path.path()));
                    }
                    write_attr(&file, value)?;
                    file.sync_all()?;
                    if read_attr(&file, value)? != *value {
                        return Err(HistoryError::Changed(path.path()));
                    }
                }
            }
            self.store.records[index].steps[i] = step;
            self.save()?;
        }
        self.store.cursor = if undo { index } else { index + 1 };
        self.store.records[index].entry.status = if undo {
            HistoryStatus::Undone
        } else {
            HistoryStatus::Applied
        };
        self.refresh_known_ancestors(&touched, cancel);
        self.store.pending = None;
        self.save()?;
        Ok(self.store.records[index].entry.clone())
    }
    fn vault_base(&self, from: &Path) -> Result<PathBuf, HistoryError> {
        let dev = FileIdentity::of_path(from)?.dev;
        let roots: Vec<_> = self
            .store
            .records
            .iter()
            .flat_map(|r| r.steps.iter())
            .filter_map(|s| {
                if let Step::Move { present, .. } = s {
                    Some(present.path())
                } else {
                    None
                }
            })
            .collect();
        for parent in from.ancestors().skip(1) {
            let dir = crate::history_tree::open_dir(parent)?;
            let s = dir.stat_self()?;
            if s.st_dev != dev {
                break;
            }
            if !roots.iter().any(|r| parent.starts_with(r)) {
                return Ok(parent.to_path_buf());
            }
        }
        Err(HistoryError::Unsupported(
            "No safe same-volume undo vault parent is available.".into(),
        ))
    }
    fn refresh_known_ancestors(&mut self, touched: &[PathBuf], cancel: &CancelToken) {
        for record in &mut self.store.records {
            for step in &mut record.steps {
                if let Step::Move {
                    present,
                    restore,
                    vault,
                    tree,
                    ..
                } = step
                {
                    let root = if record.entry.status == HistoryStatus::Applied {
                        present.path()
                    } else {
                        match restore
                            .as_ref()
                            .map(SavedPath::path)
                            .or_else(|| vault.as_ref().map(|v| v.path.path().join("item")))
                        {
                            Some(p) => p,
                            None => continue,
                        }
                    };
                    if !touched.iter().any(|p| p != &root && p.starts_with(&root)) {
                        continue;
                    }
                    let Ok(now) = tree_at(&root, cancel) else {
                        continue;
                    };
                    if now.len() != tree.len() {
                        continue;
                    }
                    let consistent = tree.iter().zip(&now).all(|(a, b)| {
                        a.relative == b.relative
                            && a.identity.dev == b.identity.dev
                            && a.identity.ino == b.identity.ino
                            && a.mode == b.mode
                            && (a.mode & 0o170000 == 0o040000 || a.identity == b.identity)
                    });
                    if consistent {
                        for (a, b) in tree.iter_mut().zip(now) {
                            let path = root.join(a.relative.path());
                            if a.mode & 0o170000 == 0o040000
                                && touched
                                    .iter()
                                    .any(|p| p.parent().is_some_and(|parent| parent == path))
                            {
                                a.identity = b.identity;
                            }
                        }
                    }
                }
            }
        }
    }
}
fn verify_ident(
    path: &Path,
    id: crate::attrs::Ident,
    now: &FileIdentity,
) -> Result<(), HistoryError> {
    if (id.dev, id.ino, id.mtime, id.mtime_nsec) != (now.dev, now.ino, now.mtime, now.mtime_nsec) {
        Err(HistoryError::Changed(path.to_path_buf()))
    } else {
        Ok(())
    }
}
fn tree_at(root: &Path, cancel: &CancelToken) -> Result<Tree, HistoryError> {
    let snapshot = crate::history_tree::snapshot(root, cancel).map_err(|_| {
        if cancel.is_cancelled() {
            HistoryError::Cancelled
        } else {
            HistoryError::IdentityUnknown(root.to_path_buf())
        }
    })?;
    snapshot
        .into_iter()
        .map(|(path, id)| {
            Ok(TreeNode {
                relative: SavedPath::new(
                    path.strip_prefix(root)
                        .map_err(|_| HistoryError::IdentityUnknown(path.clone()))?,
                ),
                identity: id.into(),
                mode: id.mode,
            })
        })
        .collect()
}
fn tree(root: &Path, cancel: &CancelToken) -> Result<Tree, HistoryError> {
    tree_at(root, cancel)
}
fn verify_tree(root: &Path, expected: &Tree, cancel: &CancelToken) -> Result<(), HistoryError> {
    let current = tree(root, cancel)?;
    let map: BTreeMap<_, _> = expected.iter().map(|n| (&n.relative.0, n)).collect();
    if expected.is_empty() {
        return Err(HistoryError::IdentityUnknown(root.to_path_buf()));
    }
    for now in &current {
        if !map.contains_key(&now.relative.0) {
            return Err(HistoryError::NewWork(root.join(now.relative.path())));
        }
    }
    for now in &current {
        let old = map
            .get(&now.relative.0)
            .ok_or_else(|| HistoryError::IdentityUnknown(root.to_path_buf()))?;
        if old.identity != now.identity || old.mode != now.mode {
            return Err(HistoryError::Changed(root.join(now.relative.path())));
        }
    }
    if current.len() != expected.len() {
        return Err(HistoryError::Changed(root.to_path_buf()));
    }
    Ok(())
}
fn move_step(present: &Path, restore: Option<&Path>, tree: Tree) -> Result<Step, HistoryError> {
    if let Some(restore) = restore {
        let source_dev = FileIdentity::of_path(present)?.dev;
        let target = Parent::of(
            restore
                .parent()
                .ok_or_else(|| HistoryError::IdentityUnknown(restore.to_path_buf()))?,
        )?;
        if target.dev != source_dev {
            return Err(HistoryError::Unsupported(
                "This inverse crosses volumes and needs a copy receipt; no item was moved.".into(),
            ));
        }
    }
    Ok(Step::Move {
        present: SavedPath::new(present),
        restore: restore.map(SavedPath::new),
        present_parent: Parent::of(
            present
                .parent()
                .ok_or_else(|| HistoryError::IdentityUnknown(present.to_path_buf()))?,
        )?,
        restore_parent: restore
            .map(|p| Parent::of(p.parent().unwrap_or(Path::new("/"))))
            .transpose()?,
        vault: None,
        tree,
    })
}
fn attr_step(
    path: &Path,
    id: crate::attrs::Ident,
    before: Attr,
    after: Attr,
) -> Result<Step, HistoryError> {
    let now = FileIdentity::of_path(path)?;
    verify_ident(path, id, &now)?;
    let file = open_attr(path, &now.into())?;
    if read_attr(&file, &after)? != after {
        return Err(HistoryError::Changed(path.to_path_buf()));
    }
    Ok(Step::Attr {
        path: SavedPath::new(path),
        parent: Parent::of(
            path.parent()
                .ok_or_else(|| HistoryError::IdentityUnknown(path.to_path_buf()))?,
        )?,
        identity: now.into(),
        before,
        after,
    })
}
fn open_attr(path: &Path, expected: &RecordedIdentity) -> Result<File, HistoryError> {
    let (parent, name) = anchor::resolve_parent(path)?;
    let file = File::from(rustix::fs::openat(
        parent.as_fd(),
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let now = FileIdentity::from_stat(&rustix::fs::fstat(&file)?);
    if !expected.matches(&now) || !matches!(now.mode & 0o170000, 0o100000 | 0o040000) {
        return Err(HistoryError::Changed(path.to_path_buf()));
    }
    Ok(file)
}
fn read_attr(file: &File, kind: &Attr) -> Result<Attr, HistoryError> {
    let stat = rustix::fs::fstat(file)?;
    Ok(match kind {
        Attr::Mode(_) => Attr::Mode(stat.st_mode & 0o7777),
        Attr::Owner(..) => Attr::Owner(stat.st_uid, stat.st_gid),
        Attr::Tags(_) => {
            let mut bytes = vec![0; gust_core::tags::MAX_VALUE];
            let value = match rustix::fs::fgetxattr(file, gust_core::tags::ATTR, &mut bytes[..]) {
                Ok(n) => {
                    bytes.truncate(n);
                    Some(bytes)
                }
                Err(rustix::io::Errno::NODATA) => None,
                Err(e) => return Err(io::Error::from(e).into()),
            };
            Attr::Tags(value)
        }
    })
}
fn write_attr(file: &File, value: &Attr) -> Result<(), HistoryError> {
    match value {
        Attr::Mode(mode) => rustix::fs::fchmod(file, Mode::from_bits_truncate(*mode))?,
        Attr::Owner(uid, gid) => {
            if rustix::fs::fstat(file)?.st_mode & 0o6000 != 0 {
                return Err(HistoryError::Unsupported(
                    "Privileged owner changes are refused.".into(),
                ));
            }
            rustix::fs::fchown(
                file,
                Some(rustix::process::Uid::from_raw(*uid)),
                Some(rustix::process::Gid::from_raw(*gid)),
            )?;
        }
        Attr::Tags(Some(bytes)) => rustix::fs::fsetxattr(
            file,
            gust_core::tags::ATTR,
            bytes,
            rustix::fs::XattrFlags::empty(),
        )?,
        Attr::Tags(None) => match rustix::fs::fremovexattr(file, gust_core::tags::ATTR) {
            Ok(()) | Err(rustix::io::Errno::NODATA) => {}
            Err(e) => return Err(io::Error::from(e).into()),
        },
    }
    Ok(())
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn interrupted_inverse_is_fenced_without_replay_or_cleanup() {
        let root = std::env::temp_dir().join(format!("gust-history-pending-{}", unique()));
        std::fs::create_dir_all(root.join("work")).unwrap();
        let created = crate::create::create_from_bytes(
            &root.join("work"),
            std::ffi::OsStr::new("file"),
            b"preserve",
        )
        .unwrap();
        let store = root.join("state/history");
        let mut history = History::open(&store).unwrap();
        let entry = history.record_created("Created file", &[created]).unwrap();
        let to = root.join("work/retained");
        history.store.pending = Some(Pending {
            id: entry.id.clone(),
            undo: true,
            step: 0,
            from: SavedPath::new(&root.join("work/file")),
            to: SavedPath::new(&to),
        });
        history.save().unwrap();
        crate::anchor::rename_noreplace_paths(&root.join("work/file"), &to).unwrap();
        drop(history);
        let mut history = History::open(&store).unwrap();
        assert_eq!(history.entries()[0].status, HistoryStatus::Interrupted);
        assert_eq!(
            history.undo(&CancelToken::new()),
            Err(HistoryError::Barrier(entry.id.clone()))
        );
        assert_eq!(
            history.redo(&CancelToken::new()),
            Err(HistoryError::Barrier(entry.id))
        );
        assert_eq!(std::fs::read(&to).unwrap(), b"preserve");
        assert!(!root.join("work/file").exists());
        drop(history);
        std::fs::remove_dir_all(root).unwrap();
    }
}
