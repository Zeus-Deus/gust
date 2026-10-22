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
    Partial {
        from: PathBuf,
        at: PathBuf,
        cause: String,
    },
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
            Self::Partial { from, at, cause } => write!(
                f,
                "Recovery is incomplete; inspect {} and {}: {cause}",
                from.display(),
                at.display()
            ),
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
        #[serde(default)]
        lineage: Option<Tree>,
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
        let steps = normalize_attribute_aliases(steps)?;
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
            #[cfg(test)]
            run_hook(&BEFORE_ADMISSION_INVENTORY);
            steps.push(move_step(
                &c.path,
                None,
                verified_receipt_root(&c.path, c.ident, c.len, id.mode)?,
            )?);
        }
        self.admit(label, HistoryKind::Create, steps)
    }
    pub fn record_folders(
        &mut self,
        label: &str,
        folders: &[crate::create::FolderCreated],
    ) -> Result<HistoryEntry, HistoryError> {
        let mut steps = Vec::new();
        for folder in folders {
            let expected = vec![TreeNode {
                relative: SavedPath::new(Path::new("")),
                identity: folder.identity.into(),
                mode: folder.identity.mode,
            }];
            verify_tree(&folder.path, &expected, &CancelToken::new())?;
            steps.push(move_step(&folder.path, None, expected)?);
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
            #[cfg(test)]
            run_hook(&BEFORE_ADMISSION_INVENTORY);
            steps.push(move_step(
                &c.link,
                None,
                verified_receipt_root(
                    &c.link,
                    c.ident,
                    c.target.as_os_str().as_bytes().len() as u64,
                    id.mode,
                )?,
            )?);
        }
        self.admit(label, HistoryKind::Create, steps)
    }
    /// Admit a completed typed Trash move. The original information inode
    /// participates in both directions, rather than deleting/recreating it.
    pub fn record_trash(
        &mut self,
        label: &str,
        receipts: &[crate::trash::TrashReceipt],
    ) -> Result<HistoryEntry, HistoryError> {
        let mut steps = Vec::new();
        let mut names = std::collections::BTreeSet::new();
        for receipt in receipts {
            receipt.verify(&CancelToken::new())?;
            receipt.verify_original_parent()?;
            let data = receipt.data_path();
            let info = receipt.info_path();
            for path in [&data, &info, receipt.original_path()] {
                if !names.insert(path.to_path_buf()) {
                    return Err(HistoryError::Unsupported(
                        "Overlapping Trash receipts need a barrier".into(),
                    ));
                }
            }
            steps.push(move_step(
                &info,
                None,
                vec![TreeNode {
                    relative: SavedPath::new(Path::new("")),
                    identity: receipt.info.into(),
                    mode: receipt.info.mode,
                }],
            )?);
            steps.push(move_step(
                &data,
                Some(receipt.original_path()),
                receipt_tree(&data, &receipt.tree)?,
            )?);
            receipt.verify(&CancelToken::new())?;
            receipt.verify_original_parent()?;
        }
        self.admit(label, HistoryKind::Trash, steps)
    }
    pub fn record_restored(
        &mut self,
        label: &str,
        receipts: &[crate::trash::RestoredReceipt],
    ) -> Result<HistoryEntry, HistoryError> {
        let mut steps = Vec::new();
        let mut names = std::collections::BTreeSet::new();
        for receipt in receipts {
            if !receipt.history_supported() {
                return Err(HistoryError::Unsupported(
                    "Restoration created missing parents; preserve a history barrier".into(),
                ));
            }
            receipt.verify(&CancelToken::new())?;
            let data = receipt.trash.data_path();
            let info = receipt.trash.info_path();
            for path in [&data, &info, &receipt.original, receipt.info_path()] {
                if !names.insert(path.to_path_buf()) {
                    return Err(HistoryError::Unsupported(
                        "Overlapping restoration receipts need a barrier".into(),
                    ));
                }
            }
            steps.push(move_step(
                &receipt.original,
                Some(&data),
                receipt_tree(&data, &receipt.trash.tree)?,
            )?);
            steps.push(move_step(
                receipt.info_path(),
                Some(&info),
                vec![TreeNode {
                    relative: SavedPath::new(Path::new("")),
                    identity: receipt.trash.info.into(),
                    mode: receipt.trash.info.mode,
                }],
            )?);
        }
        for receipt in receipts {
            receipt.verify(&CancelToken::new())?;
        }
        self.admit(label, HistoryKind::Trash, steps)
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
                    let held_parent = parent.open()?;
                    verify_tree_at(&held_parent, &from, tree, cancel)?;
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
                    lineage,
                } => {
                    parent.open()?;
                    let file = open_attr(&path.path(), identity)?;
                    if let Some(lineage) = lineage {
                        if !deferred_mode_lineage(&file, if undo { before } else { after })? {
                            verify_tree(&path.path(), lineage, cancel)?;
                        }
                    }
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
                    verify_tree_at(&source, &from, tree, cancel)?;
                    #[cfg(test)]
                    run_hook(&BEFORE_INVERSE_RENAME);
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
                    let moved = target
                        .stat_child(
                            to.file_name()
                                .ok_or_else(|| HistoryError::IdentityUnknown(to.clone()))?,
                        )
                        .map(|stat| FileIdentity::from_stat(&stat))
                        .map_err(|error| HistoryError::Partial {
                            from: from.clone(),
                            at: to.clone(),
                            cause: error.to_string(),
                        })?;
                    if let Err(cause) = verify_tree_at(&target, &to, tree, cancel) {
                        match restore_mismatching_move(&target, &to, &source, &from, &moved) {
                            Ok(()) => {
                                rustix::fs::fsync(source.as_fd())?;
                                rustix::fs::fsync(target.as_fd())?;
                                return Err(HistoryError::Changed(from));
                            }
                            Err(recovery) => {
                                return Err(HistoryError::Partial {
                                    from,
                                    at: to,
                                    cause: format!("{cause}; {recovery}"),
                                })
                            }
                        }
                    }
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
                    lineage,
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
                    let deferred = lineage.is_some() && deferred_mode_lineage(&file, value)?;
                    let mut next_lineage = lineage.clone();
                    let validation = (|| -> Result<(), HistoryError> {
                        write_attr(&file, value)?;
                        sync_attribute(&file, parent)?;
                        if read_attr(&file, value)? != *value {
                            return Err(HistoryError::Changed(path.path()));
                        }
                        let current = FileIdentity::from_stat(&rustix::fs::fstat(&file)?);
                        if !identity.matches(&current) {
                            return Err(HistoryError::Changed(path.path()));
                        }
                        if let Some(lineage) = &mut next_lineage {
                            if let Some(root) =
                                lineage.iter_mut().find(|node| node.relative.0.is_empty())
                            {
                                root.mode = current.mode;
                            }
                            if deferred {
                                verify_tree(&path.path(), lineage, cancel)?;
                            }
                        }
                        Ok(())
                    })();
                    if let Err(cause) = validation {
                        if deferred {
                            let rollback = (|| -> Result<(), HistoryError> {
                                let current = FileIdentity::from_stat(&rustix::fs::fstat(&file)?);
                                if !identity.matches(&current) || read_attr(&file, value)? != *value
                                {
                                    return Err(HistoryError::Changed(path.path()));
                                }
                                write_attr(&file, expected)?;
                                sync_attribute(&file, parent)?;
                                if read_attr(&file, expected)? != *expected {
                                    return Err(HistoryError::Changed(path.path()));
                                }
                                Ok(())
                            })();
                            let recovery = match rollback {
                                Ok(()) => "the prior mode was restored".into(),
                                Err(error) => format!("mode recovery refused: {error}"),
                            };
                            return Err(HistoryError::Partial {
                                from: path.path(),
                                at: path.path(),
                                cause: format!("{cause}; {recovery}"),
                            });
                        }
                        return Err(cause);
                    }
                    *lineage = next_lineage;
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
            .map(|s| match s {
                Step::Move { present, .. } => present.path(),
                Step::Attr { path, .. } => path.path(),
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
                match step {
                    Step::Move {
                        present,
                        restore,
                        vault,
                        tree,
                        ..
                    } => {
                        let root = if record.entry.status == HistoryStatus::Applied {
                            present.path()
                        } else {
                            let Some(root) = restore
                                .as_ref()
                                .map(SavedPath::path)
                                .or_else(|| vault.as_ref().map(|v| v.path.path().join("item")))
                            else {
                                continue;
                            };
                            root
                        };
                        refresh_lineage(tree, &root, touched, cancel);
                    }
                    Step::Attr {
                        path,
                        identity,
                        lineage: Some(lineage),
                        ..
                    } => {
                        if refresh_lineage(lineage, &path.path(), touched, cancel) {
                            if let Some(root) =
                                lineage.iter().find(|node| node.relative.0.is_empty())
                            {
                                *identity = root.identity;
                            }
                        }
                    }
                    _ => {}
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
    compare_tree(root, expected, tree(root, cancel)?)
}
fn compare_tree(root: &Path, expected: &Tree, current: Tree) -> Result<(), HistoryError> {
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
fn receipt_tree(root: &Path, tree: &BTreeMap<PathBuf, FileIdentity>) -> Result<Tree, HistoryError> {
    tree.iter()
        .map(|(path, id)| {
            Ok(TreeNode {
                relative: SavedPath::new(
                    path.strip_prefix(root)
                        .map_err(|_| HistoryError::IdentityUnknown(path.clone()))?,
                ),
                identity: (*id).into(),
                mode: id.mode,
            })
        })
        .collect()
}
fn move_step(present: &Path, restore: Option<&Path>, mut tree: Tree) -> Result<Step, HistoryError> {
    tree.sort_by(|a, b| a.relative.0.cmp(&b.relative.0));
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
    let lineage = if now.mode & 0o170000 == 0o040000 && !metadata_only(&file)? {
        let observed = tree(path, &CancelToken::new())?;
        let root = observed
            .iter()
            .find(|node| node.relative.0.is_empty())
            .ok_or_else(|| HistoryError::IdentityUnknown(path.to_path_buf()))?;
        if !root.identity.matches(&now) || root.mode != now.mode {
            return Err(HistoryError::Changed(path.to_path_buf()));
        }
        Some(observed)
    } else {
        None
    };
    Ok(Step::Attr {
        path: SavedPath::new(path),
        parent: Parent::of(
            path.parent()
                .ok_or_else(|| HistoryError::IdentityUnknown(path.to_path_buf()))?,
        )?,
        identity: now.into(),
        before,
        after,
        lineage,
    })
}
fn open_attr(path: &Path, expected: &RecordedIdentity) -> Result<File, HistoryError> {
    let (parent, name) = anchor::resolve_parent(path)?;
    let flags = OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let fd = match rustix::fs::openat(parent.as_fd(), &name, OFlags::RDONLY | flags, Mode::empty())
    {
        Ok(fd) => fd,
        Err(rustix::io::Errno::ACCESS) => {
            rustix::fs::openat(parent.as_fd(), &name, OFlags::PATH | flags, Mode::empty())?
        }
        Err(error) => return Err(error.into()),
    };
    let file = File::from(fd);
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
        Attr::Mode(mode) => {
            if metadata_only(file)? {
                use std::os::fd::AsRawFd;
                // This kernel-owned magic link addresses our still-held O_PATH
                // descriptor. It grants no read access and cannot adopt a new
                // object at the original name. rustix's chmodat implementation
                // does not yet support fchmodat2/AT_EMPTY_PATH.
                rustix::fs::chmodat(
                    rustix::fs::CWD,
                    format!("/proc/self/fd/{}", file.as_raw_fd()),
                    Mode::from_bits_truncate(*mode),
                    rustix::fs::AtFlags::empty(),
                )?;
            } else {
                rustix::fs::fchmod(file, Mode::from_bits_truncate(*mode))?;
            }
        }
        Attr::Owner(uid, gid) => {
            if rustix::fs::fstat(file)?.st_mode & 0o6000 != 0 {
                return Err(HistoryError::Unsupported(
                    "Privileged owner changes are refused.".into(),
                ));
            }
            let uid = Some(rustix::process::Uid::from_raw(*uid));
            let gid = Some(rustix::process::Gid::from_raw(*gid));
            if metadata_only(file)? {
                rustix::fs::chownat(file, "", uid, gid, rustix::fs::AtFlags::EMPTY_PATH)?;
            } else {
                rustix::fs::fchown(file, uid, gid)?;
            }
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

fn verified_receipt_root(
    path: &Path,
    ident: crate::attrs::Ident,
    size: u64,
    mode: u32,
) -> Result<Tree, HistoryError> {
    let expected = vec![TreeNode {
        relative: SavedPath::new(Path::new("")),
        identity: RecordedIdentity {
            dev: ident.dev,
            ino: ident.ino,
            mtime: ident.mtime,
            mtime_nsec: ident.mtime_nsec,
            size,
        },
        mode,
    }];
    verify_tree(path, &expected, &CancelToken::new())?;
    Ok(expected)
}
fn normalize_attribute_aliases(steps: Vec<Step>) -> Result<Vec<Step>, HistoryError> {
    let mut out: Vec<Step> = Vec::new();
    let mut seen = BTreeMap::new();
    for step in steps {
        if let Step::Attr {
            path,
            identity,
            before,
            after,
            ..
        } = &step
        {
            let kind = match before {
                Attr::Mode(_) => 0,
                Attr::Owner(..) => 1,
                Attr::Tags(_) => 2,
            };
            let key = (identity.dev, identity.ino, kind);
            if let Some(index) = seen.get(&key) {
                if let Step::Attr {
                    identity: other,
                    before: old,
                    after: new,
                    ..
                } = &out[*index]
                {
                    if identity != other || before != old || after != new {
                        return Err(HistoryError::Unsupported(format!(
                            "Attribute aliases disagree at {}",
                            path.path().display()
                        )));
                    }
                    continue;
                }
            }
            seen.insert(key, out.len());
        }
        out.push(step);
    }
    Ok(out)
}
fn verify_tree_at(
    parent: &DirHandle,
    root: &Path,
    expected: &Tree,
    cancel: &CancelToken,
) -> Result<(), HistoryError> {
    let name = root
        .file_name()
        .ok_or_else(|| HistoryError::IdentityUnknown(root.to_path_buf()))?;
    let snapshot =
        crate::history_tree::snapshot_at(parent, name, root, cancel).map_err(|error| {
            if cancel.is_cancelled() {
                HistoryError::Cancelled
            } else {
                HistoryError::Io(error.to_string())
            }
        })?;
    let current = snapshot
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
        .collect::<Result<_, HistoryError>>()?;
    compare_tree(root, expected, current)
}
fn restore_mismatching_move(
    target: &DirHandle,
    to: &Path,
    source: &DirHandle,
    from: &Path,
    moved: &FileIdentity,
) -> Result<(), HistoryError> {
    let to_name = to
        .file_name()
        .ok_or_else(|| HistoryError::IdentityUnknown(to.to_path_buf()))?;
    let from_name = from
        .file_name()
        .ok_or_else(|| HistoryError::IdentityUnknown(from.to_path_buf()))?;
    if !moved.matches(&FileIdentity::from_stat(&target.stat_child(to_name)?)) {
        return Err(HistoryError::Changed(to.to_path_buf()));
    }
    #[cfg(test)]
    run_hook(&BEFORE_INVERSE_RECOVERY);
    anchor::rename_noreplace(target, to_name, source, from_name)?;
    if !moved.matches(&FileIdentity::from_stat(&source.stat_child(from_name)?)) {
        return Err(HistoryError::Changed(from.to_path_buf()));
    }
    Ok(())
}
fn refresh_lineage(
    tree: &mut Tree,
    root: &Path,
    touched: &[PathBuf],
    cancel: &CancelToken,
) -> bool {
    if !touched
        .iter()
        .any(|path| path != root && path.starts_with(root))
    {
        return false;
    }
    let Ok(now) = tree_at(root, cancel) else {
        return false;
    };
    let keyed: BTreeMap<_, _> = now.iter().map(|node| (&node.relative.0, node)).collect();
    if keyed.len() != tree.len() {
        return false;
    }
    if !tree.iter().all(|old| {
        keyed.get(&old.relative.0).is_some_and(|new| {
            old.identity.dev == new.identity.dev
                && old.identity.ino == new.identity.ino
                && old.mode == new.mode
                && (old.mode & 0o170000 == 0o040000 || old.identity == new.identity)
        })
    }) {
        return false;
    }
    for old in tree.iter_mut() {
        if old.mode & 0o170000 == 0o040000
            && touched
                .iter()
                .any(|path| path.parent() == Some(root.join(old.relative.path()).as_path()))
        {
            if let Some(new) = keyed.get(&old.relative.0) {
                old.identity = new.identity;
            }
        }
    }
    true
}
fn metadata_only(file: &File) -> Result<bool, HistoryError> {
    Ok(rustix::fs::fcntl_getfl(file)?.contains(OFlags::PATH))
}
fn deferred_mode_lineage(file: &File, requested: &Attr) -> Result<bool, HistoryError> {
    let stat = rustix::fs::fstat(file)?;
    Ok(stat.st_mode & 0o170000 == 0o040000
        && stat.st_uid == rustix::process::geteuid().as_raw()
        && stat.st_mode & 0o500 != 0o500
        && matches!(requested, Attr::Mode(mode) if mode & 0o500 == 0o500))
}
fn sync_attribute(file: &File, parent: &Parent) -> Result<(), HistoryError> {
    if metadata_only(file)? {
        rustix::fs::syncfs(parent.open()?.as_fd())?;
    } else {
        file.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
type RaceHook = std::cell::RefCell<Option<Box<dyn FnOnce()>>>;
#[cfg(test)]
thread_local! {
    static BEFORE_ADMISSION_INVENTORY: RaceHook = const { std::cell::RefCell::new(None) };
    static BEFORE_INVERSE_RENAME: RaceHook = const { std::cell::RefCell::new(None) };
    static BEFORE_INVERSE_RECOVERY: RaceHook = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
fn run_hook(hook: &'static std::thread::LocalKey<RaceHook>) {
    hook.with(|cell| {
        if let Some(run) = cell.borrow_mut().take() {
            run();
        }
    });
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
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("gust-history-race-{}", unique()));
            std::fs::create_dir_all(path.join("work")).unwrap();
            Self(path)
        }
        fn history(&self) -> History {
            History::open(self.0.join("state/history")).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn created_admission_does_not_adopt_a_replacement_at_inventory_boundary() {
        let f = Fixture::new();
        let made = crate::create::create_from_bytes(
            &f.0.join("work"),
            std::ffi::OsStr::new("file"),
            b"original",
        )
        .unwrap();
        let path = made.path.clone();
        let saved = f.0.join("work/saved");
        let saved_in_hook = saved.clone();
        BEFORE_ADMISSION_INVENTORY.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                std::fs::rename(&path, &saved_in_hook).unwrap();
                std::fs::write(&path, b"later!!!").unwrap();
            }))
        });
        let mut history = f.history();
        assert!(history.record_created("Created", &[made]).is_err());
        assert!(history.entries().is_empty());
        assert_eq!(std::fs::read(f.0.join("work/file")).unwrap(), b"later!!!");
        assert_eq!(std::fs::read(saved).unwrap(), b"original");
    }
    #[test]
    fn link_admission_does_not_adopt_a_regular_replacement() {
        let f = Fixture::new();
        let made = crate::attrs::create_link(
            &f.0.join("target"),
            &f.0.join("work"),
            std::ffi::OsStr::new("link"),
        )
        .unwrap();
        let path = made.link.clone();
        let saved = f.0.join("work/saved-link");
        BEFORE_ADMISSION_INVENTORY.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                std::fs::rename(&path, &saved).unwrap();
                std::fs::write(&path, b"later work").unwrap();
            }))
        });
        let mut history = f.history();
        assert!(history.record_links("Link", &[made]).is_err());
        assert!(history.entries().is_empty());
        assert_eq!(std::fs::read(f.0.join("work/link")).unwrap(), b"later work");
    }
    #[test]
    fn a_racing_wrong_source_is_restored_without_hiding_later_work() {
        let f = Fixture::new();
        let made = crate::create::create_from_bytes(
            &f.0.join("work"),
            std::ffi::OsStr::new("file"),
            b"original",
        )
        .unwrap();
        let path = made.path.clone();
        let saved = f.0.join("work/saved");
        let saved_in_hook = saved.clone();
        let mut history = f.history();
        history.record_created("Created", &[made]).unwrap();
        BEFORE_INVERSE_RENAME.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                std::fs::rename(&path, &saved_in_hook).unwrap();
                std::fs::write(&path, b"later work").unwrap();
            }))
        });
        assert!(history.undo(&CancelToken::new()).is_err());
        assert_eq!(std::fs::read(f.0.join("work/file")).unwrap(), b"later work");
        assert_eq!(std::fs::read(saved).unwrap(), b"original");
    }

    #[test]
    fn occupied_race_recovery_reports_both_locations_and_keeps_the_fence() {
        let f = Fixture::new();
        let made = crate::create::create_from_bytes(
            &f.0.join("work"),
            std::ffi::OsStr::new("file"),
            b"original",
        )
        .unwrap();
        let path = made.path.clone();
        let recovery_path = path.clone();
        let saved = f.0.join("work/saved");
        let mut history = f.history();
        let entry = history.record_created("Created", &[made]).unwrap();
        BEFORE_INVERSE_RENAME.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                std::fs::rename(&path, &saved).unwrap();
                std::fs::write(&path, b"racing source").unwrap();
            }))
        });
        BEFORE_INVERSE_RECOVERY.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                std::fs::write(&recovery_path, b"new occupant").unwrap();
            }))
        });
        let error = history.undo(&CancelToken::new()).unwrap_err();
        let HistoryError::Partial { from, at, .. } = error else {
            panic!("Expected explicit partial recovery, got {error:?}");
        };
        assert_eq!(from, f.0.join("work/file"));
        assert_eq!(std::fs::read(&from).unwrap(), b"new occupant");
        assert_eq!(std::fs::read(&at).unwrap(), b"racing source");
        assert_eq!(std::fs::read(f.0.join("work/saved")).unwrap(), b"original");
        drop(history);
        let mut history = f.history();
        assert_eq!(history.entries()[0].status, HistoryStatus::Interrupted);
        assert_eq!(
            history.undo(&CancelToken::new()),
            Err(HistoryError::Barrier(entry.id))
        );
    }
}
