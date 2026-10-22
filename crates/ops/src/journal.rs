//! Operation journal.
//!
//! Written BEFORE the destructive commit and updated after, so a process death
//! mid-operation leaves a recoverable record (DESIGN.md §5, §7).

use crate::anchor::FileIdentity;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobKind {
    Copy,
    Move,
    Trash,
    DeletePermanent,
    Rename,
    NewFolder,
    Extract,
}

impl JobKind {
    /// Can the inverse of this operation be performed safely?
    ///
    /// Permanent deletion has no inverse — the UI must not offer Undo for it
    /// (plan §9 "Undo must not be displayed for irreversible deletion").
    pub fn is_undoable(self) -> bool {
        match self {
            JobKind::Copy
            | JobKind::Move
            | JobKind::Trash
            | JobKind::Rename
            | JobKind::NewFolder
            | JobKind::Extract => true,
            JobKind::DeletePermanent => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    Planned,
    Running,
    Committing,
    /// Bytes sent, backend has NOT confirmed. Never displayed as "uploaded".
    RemotePending,
    Confirmed,
    Cancelling,
    Cancelled,
    Retrying,
    Failed,
    Conflict,
}

impl JobState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobState::Confirmed | JobState::Cancelled | JobState::Failed
        )
    }

    /// Only a backend-confirmed job may be described as complete to the user.
    pub fn is_user_visible_success(self) -> bool {
        self == JobState::Confirmed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictPolicy {
    Ask,
    Skip,
    Replace,
    KeepBoth,
    /// A folder onto a folder of the same name: fold the contents in,
    /// never overwriting anything (DESIGN §6.24). For anything that is not
    /// a real folder on both sides it means Keep both.
    Merge,
}

/// One affected path plus everything needed to invert it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    #[serde(with = "path_wire")]
    pub source: PathBuf,
    #[serde(with = "optional_path_wire")]
    pub destination: Option<PathBuf>,
    /// Identity of the source at plan time.
    pub source_identity: Option<RecordedIdentity>,
    /// Where overwritten content was preserved, if anything was overwritten.
    #[serde(with = "optional_path_wire")]
    pub preserved: Option<PathBuf>,
    pub done: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedIdentity {
    pub dev: u64,
    pub ino: u64,
    pub mtime: i64,
    pub mtime_nsec: i64,
    pub size: u64,
}

impl From<FileIdentity> for RecordedIdentity {
    fn from(f: FileIdentity) -> Self {
        RecordedIdentity {
            dev: f.dev,
            ino: f.ino,
            mtime: f.mtime,
            mtime_nsec: f.mtime_nsec,
            size: f.size,
        }
    }
}

impl RecordedIdentity {
    pub fn matches(&self, f: &FileIdentity) -> bool {
        self.dev == f.dev
            && self.ino == f.ino
            && self.mtime == f.mtime
            && self.mtime_nsec == f.mtime_nsec
            && self.size == f.size
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub kind: JobKind,
    pub state: JobState,
    pub conflict_policy: ConflictPolicy,
    pub entries: Vec<JournalEntry>,
    /// Every descendant identity captured at plan time, for subtree-safe undo.
    #[serde(with = "identities_wire")]
    pub subtree_identities: Vec<(PathBuf, RecordedIdentity)>,
    /// Paths that already existed at the destination BEFORE this operation
    /// ran. Undo must never remove these: they are the user's own data that
    /// a merging copy happened to land beside.
    #[serde(default)]
    #[serde(with = "paths_wire")]
    pub preexisting_at_dest: Vec<PathBuf>,
    pub created_unix: i64,
    /// The process that ran the job and the boot it ran in. Together they
    /// tell a crashed job from one still running in another Gust window.
    /// Records from before these existed read as pid 0: owner unknown.
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub boot: String,
    /// What the job set out to do, written before the first byte, so a
    /// crash during the first file still leaves a record that says where.
    #[serde(default)]
    #[serde(with = "paths_wire")]
    pub planned: Vec<PathBuf>,
    #[serde(default)]
    #[serde(with = "optional_path_wire")]
    pub dest_dir: Option<PathBuf>,
    /// The user has seen the start-up notice for this interrupted job.
    #[serde(default)]
    pub acknowledged: bool,
    /// Source folders a merging move emptied and removed, so Undo can
    /// recreate them before moving anything back (DESIGN §6.24).
    #[serde(default)]
    #[serde(with = "paths_wire")]
    pub removed_empty_dirs: Vec<PathBuf>,
}

impl Job {
    pub fn new(id: impl Into<String>, kind: JobKind) -> Self {
        Job {
            id: id.into(),
            kind,
            state: JobState::Planned,
            conflict_policy: ConflictPolicy::Ask,
            entries: Vec::new(),
            subtree_identities: Vec::new(),
            preexisting_at_dest: Vec::new(),
            created_unix: now_unix(),
            pid: std::process::id(),
            boot: crate::staging::boot_id(),
            planned: Vec::new(),
            dest_dir: None,
            acknowledged: false,
            removed_empty_dirs: Vec::new(),
        }
    }

    /// Undo is offered only when the inverse is genuinely safe.
    pub fn offers_undo(&self) -> bool {
        self.kind.is_undoable() && self.state == JobState::Confirmed
    }
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Append-only journal on disk. One JSON object per line so a torn write at
/// the tail can be discarded without losing earlier records.
pub struct Journal {
    path: PathBuf,
}

impl Journal {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Journal { path: path.into() }
    }

    /// Default private journal location.
    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        base.join("gust/journal.ndjson")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Record a job state. Durable: fsync before returning, so the journal
    /// cannot lag behind the destructive action it protects.
    pub fn record(&self, job: &Job) -> std::io::Result<()> {
        self.append(&serde_json::to_vec(job)?)
    }

    pub(crate) fn record_update(&self, update: &JournalUpdate) -> std::io::Result<()> {
        self.append(&serde_json::to_vec(update)?)
    }

    fn append(&self, bytes: &[u8]) -> std::io::Result<()> {
        use std::os::unix::fs::DirBuilderExt;
        if let Some(parent) = self.path.parent() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
        let (parent, name) = crate::anchor::resolve_parent(&self.path)?;
        let fd = rustix::fs::openat(
            parent.as_fd(),
            &name,
            rustix::fs::OFlags::WRONLY
                | rustix::fs::OFlags::APPEND
                | rustix::fs::OFlags::CREATE
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::from_bits_truncate(0o600),
        )?;
        let stat = rustix::fs::fstat(&fd)?;
        if stat.st_mode & 0o170000 != 0o100000 || stat.st_uid != rustix::process::geteuid().as_raw()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "journal is not a private owned regular file",
            ));
        }
        rustix::fs::flock(&fd, rustix::fs::FlockOperation::LockExclusive)?;
        rustix::fs::fchmod(&fd, rustix::fs::Mode::from_bits_truncate(0o600))?;
        let mut file = std::fs::File::from(fd);
        file.write_all(bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        rustix::fs::fsync(parent.as_fd())?;
        Ok(())
    }

    /// Read records and fold compact archive updates into their admitted job.
    /// A torn tail is ignored; an unknown update cannot invent a new job.
    pub fn load(&self) -> Vec<Job> {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        let mut jobs = Vec::<Job>::new();
        let mut latest = std::collections::BTreeMap::<String, usize>::new();
        for line in text.lines() {
            if let Ok(job) = serde_json::from_str::<Job>(line) {
                latest.insert(job.id.clone(), jobs.len());
                jobs.push(job);
            } else if let Ok(update) = serde_json::from_str::<JournalUpdate>(line) {
                let Some(index) = latest.get(update.id()).copied() else {
                    continue;
                };
                update.apply(&mut jobs[index]);
            }
        }
        jobs
    }

    /// Jobs whose process died before they ended, not yet acknowledged —
    /// what the start-up notice reports.
    ///
    /// A job still running in a live Gust window of this boot is NOT
    /// interrupted. A record from an earlier boot is, whatever its pid: that
    /// number may now belong to an unrelated process.
    pub fn interrupted(&self) -> Vec<Interrupted> {
        let boot = crate::staging::boot_id();
        let mut latest: std::collections::BTreeMap<String, Job> = Default::default();
        let mut order: Vec<String> = Vec::new();
        for j in self.load() {
            if !latest.contains_key(&j.id) {
                order.push(j.id.clone());
            }
            latest.insert(j.id.clone(), j);
        }
        order
            .into_iter()
            .filter_map(|id| latest.remove(&id))
            // Conflict also ends a run: the worker returns after recording
            // it, having told the user at the time.
            .filter(|j| !j.state.is_terminal() && j.state != JobState::Conflict)
            .filter(|j| !j.acknowledged)
            // No owner recorded: written by an older build, whose fate cannot
            // be known. Never guess a crash from it.
            .filter(|j| j.pid != 0 && !j.boot.is_empty())
            .filter(|j| {
                let alive =
                    j.boot == boot && j.pid != 0 && Path::new(&format!("/proc/{}", j.pid)).exists();
                !alive
            })
            .map(|j| {
                let finished = j.entries.iter().filter(|e| e.done).count();
                let total = j.planned.len().max(j.entries.len());
                let dest_dir = j.dest_dir.clone().or_else(|| {
                    j.entries
                        .iter()
                        .find_map(|e| e.destination.as_ref()?.parent().map(Path::to_path_buf))
                });
                let first = j
                    .planned
                    .first()
                    .or_else(|| j.entries.first().map(|e| &e.source))
                    .cloned();
                Interrupted {
                    id: j.id,
                    kind: j.kind,
                    first,
                    finished,
                    total,
                    dest_dir,
                }
            })
            .collect()
    }

    /// Mark an interrupted job as seen, durably, so the notice is shown once.
    pub fn acknowledge(&self, id: &str) -> std::io::Result<()> {
        let Some(mut j) = self.load().into_iter().rfind(|j| j.id == id) else {
            return Ok(());
        };
        j.acknowledged = true;
        self.record(&j)
    }

    /// Jobs that were interrupted and need recovery on next start.
    pub fn unfinished(&self) -> Vec<Job> {
        let mut latest: std::collections::BTreeMap<String, Job> = Default::default();
        for j in self.load() {
            latest.insert(j.id.clone(), j);
        }
        latest
            .into_values()
            .filter(|j| !j.state.is_terminal())
            .collect()
    }
}

/// Compact, durable archive publication records. Intent has no authority to
/// delete; only a committed identity may become a completed entry.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "update")]
pub(crate) enum JournalUpdate {
    ArchiveIntent {
        id: String,
        index: usize,
        source: Vec<u8>,
        destination: Vec<u8>,
        expected: Option<RecordedIdentity>,
    },
    ArchivePublished {
        id: String,
        index: usize,
        identity: RecordedIdentity,
    },
}
impl JournalUpdate {
    fn id(&self) -> &str {
        match self {
            Self::ArchiveIntent { id, .. } | Self::ArchivePublished { id, .. } => id,
        }
    }
    fn apply(self, job: &mut Job) {
        use std::os::unix::ffi::OsStringExt;
        if job.state.is_terminal() {
            return;
        }
        match self {
            Self::ArchiveIntent {
                index,
                source,
                destination,
                expected,
                ..
            } if index == job.entries.len() => {
                job.state = JobState::Committing;
                job.entries.push(JournalEntry {
                    source: std::ffi::OsString::from_vec(source).into(),
                    destination: Some(std::ffi::OsString::from_vec(destination).into()),
                    source_identity: expected,
                    preserved: None,
                    done: false,
                    error: None,
                });
            }
            Self::ArchivePublished {
                index, identity, ..
            } => {
                if let Some(entry) = job.entries.get_mut(index) {
                    entry.done = true;
                    if let Some(path) = &entry.destination {
                        job.subtree_identities.push((path.clone(), identity));
                    }
                }
            }
            _ => {}
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum WirePath {
    Text(String),
    Bytes(Vec<u8>),
}
impl WirePath {
    fn new(path: &Path) -> Self {
        use std::os::unix::ffi::OsStrExt;
        match path.to_str() {
            Some(text) => Self::Text(text.into()),
            None => Self::Bytes(path.as_os_str().as_bytes().to_vec()),
        }
    }
    fn path(self) -> PathBuf {
        use std::os::unix::ffi::OsStringExt;
        match self {
            Self::Text(text) => text.into(),
            Self::Bytes(bytes) => std::ffi::OsString::from_vec(bytes).into(),
        }
    }
}
mod path_wire {
    use super::*;
    pub fn serialize<S: serde::Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
        WirePath::new(path).serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<PathBuf, D::Error> {
        Ok(WirePath::deserialize(deserializer)?.path())
    }
}
mod optional_path_wire {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        path: &Option<PathBuf>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        path.as_ref()
            .map(|p| WirePath::new(p))
            .serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<PathBuf>, D::Error> {
        Ok(Option::<WirePath>::deserialize(deserializer)?.map(WirePath::path))
    }
}
mod paths_wire {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        paths: &[PathBuf],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        paths
            .iter()
            .map(|p| WirePath::new(p))
            .collect::<Vec<_>>()
            .serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<PathBuf>, D::Error> {
        Ok(Vec::<WirePath>::deserialize(deserializer)?
            .into_iter()
            .map(WirePath::path)
            .collect())
    }
}
mod identities_wire {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        paths: &[(PathBuf, RecordedIdentity)],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        paths
            .iter()
            .map(|(p, id)| (WirePath::new(p), id))
            .collect::<Vec<_>>()
            .serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<(PathBuf, RecordedIdentity)>, D::Error> {
        Ok(
            Vec::<(WirePath, RecordedIdentity)>::deserialize(deserializer)?
                .into_iter()
                .map(|(p, id)| (p.path(), id))
                .collect(),
        )
    }
}

/// One job a crash interrupted, as the start-up notice reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Interrupted {
    pub id: String,
    pub kind: JobKind,
    /// The first item it was working on, for naming the job.
    pub first: Option<PathBuf>,
    pub finished: usize,
    pub total: usize,
    pub dest_dir: Option<PathBuf>,
}

impl Interrupted {
    /// One plain sentence: what, where, how far.
    pub fn sentence(&self) -> String {
        let verb = match self.kind {
            JobKind::Copy => "Copying",
            JobKind::Move => "Moving",
            JobKind::Trash => "Moving to Trash",
            JobKind::Extract => "Extracting",
            JobKind::Rename => "Renaming",
            JobKind::NewFolder => "Creating",
            JobKind::DeletePermanent => "Deleting",
        };
        let name = self
            .first
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "an item".into());
        let more = self.total.saturating_sub(1);
        let what = if more > 0 {
            format!("{name} and {more} more")
        } else {
            name
        };
        let dest = self
            .dest_dir
            .as_ref()
            .and_then(|d| d.file_name())
            .map(|n| format!(" to {}", n.to_string_lossy()))
            .unwrap_or_default();
        let how_far = if self.finished == 0 {
            "nothing had finished".to_string()
        } else {
            format!("{} of {} finished", self.finished, self.total)
        };
        format!("{verb} {what}{dest} was interrupted: {how_far}.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_journal(tag: &str) -> Journal {
        let p =
            std::env::temp_dir().join(format!("gust-journal-{tag}-{}.ndjson", std::process::id()));
        let _ = std::fs::remove_file(&p);
        Journal::new(p)
    }

    #[test]
    fn permanent_delete_is_never_undoable() {
        assert!(!JobKind::DeletePermanent.is_undoable());
        let mut j = Job::new("j1", JobKind::DeletePermanent);
        j.state = JobState::Confirmed;
        assert!(
            !j.offers_undo(),
            "irreversible deletion must not offer undo"
        );
    }

    #[test]
    fn trash_is_undoable_when_confirmed() {
        let mut j = Job::new("j2", JobKind::Trash);
        assert!(!j.offers_undo(), "not undoable until confirmed");
        j.state = JobState::Confirmed;
        assert!(j.offers_undo());
    }

    #[test]
    fn remote_pending_is_not_success() {
        assert!(!JobState::RemotePending.is_user_visible_success());
        assert!(!JobState::RemotePending.is_terminal());
        assert!(JobState::Confirmed.is_user_visible_success());
    }

    #[test]
    fn journal_round_trips_and_survives_torn_line() {
        let jr = tmp_journal("torn");
        let mut job = Job::new("abc", JobKind::Move);
        job.state = JobState::Running;
        jr.record(&job).unwrap();
        job.state = JobState::Confirmed;
        jr.record(&job).unwrap();

        // Simulate a torn write at the tail.
        let mut text = std::fs::read_to_string(jr.path()).unwrap();
        text.push_str("{\"id\":\"broken\",\"kin");
        std::fs::write(jr.path(), text).unwrap();

        let jobs = jr.load();
        assert_eq!(jobs.len(), 2, "torn tail discarded, earlier records intact");
        assert_eq!(jobs[1].state, JobState::Confirmed);
        std::fs::remove_file(jr.path()).ok();
    }

    #[test]
    fn unfinished_reports_only_non_terminal_latest_state() {
        let jr = tmp_journal("unfinished");
        let mut a = Job::new("a", JobKind::Copy);
        a.state = JobState::Running;
        jr.record(&a).unwrap();
        a.state = JobState::Confirmed;
        jr.record(&a).unwrap();

        let mut b = Job::new("b", JobKind::Copy);
        b.state = JobState::Running;
        jr.record(&b).unwrap();

        let un = jr.unfinished();
        assert_eq!(un.len(), 1);
        assert_eq!(un[0].id, "b");
        std::fs::remove_file(jr.path()).ok();
    }

    #[test]
    fn journal_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let jr = tmp_journal("perms");
        jr.record(&Job::new("p", JobKind::Copy)).unwrap();
        let mode = std::fs::metadata(jr.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "journal must not be group/world readable");
        std::fs::remove_file(jr.path()).ok();
    }
    #[test]
    fn compact_archive_intents_fold_without_inventing_jobs_or_completion() {
        let d = std::env::temp_dir().join(format!("gust-journal-delta-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let path = d.join("journal");
        let _ = std::fs::remove_file(&path);
        let journal = Journal::new(&path);
        let job = Job::new("delta", JobKind::Extract);
        journal.record(&job).unwrap();
        journal
            .record_update(&JournalUpdate::ArchiveIntent {
                id: "delta".into(),
                index: 0,
                source: b"/synthetic/staging".to_vec(),
                destination: b"/synthetic/destination".to_vec(),
                expected: None,
            })
            .unwrap();
        journal
            .record_update(&JournalUpdate::ArchiveIntent {
                id: "unknown".into(),
                index: 0,
                source: Vec::new(),
                destination: Vec::new(),
                expected: None,
            })
            .unwrap();
        let loaded = journal.load();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].state, JobState::Committing);
        assert_eq!(loaded[0].entries.len(), 1);
        assert!(!loaded[0].entries[0].done);
        let identity = RecordedIdentity {
            dev: 1,
            ino: 2,
            mtime: 3,
            mtime_nsec: 4,
            size: 5,
        };
        journal
            .record_update(&JournalUpdate::ArchivePublished {
                id: "delta".into(),
                index: 0,
                identity,
            })
            .unwrap();
        let loaded = journal.load();
        assert!(loaded[0].entries[0].done);
        assert_eq!(loaded[0].subtree_identities.len(), 1);
        assert_eq!(
            loaded[0].state,
            JobState::Committing,
            "a member receipt is not whole-job success"
        );
        std::fs::remove_dir_all(d).unwrap();
    }
}
