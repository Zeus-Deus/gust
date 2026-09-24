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
}

/// One affected path plus everything needed to invert it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    pub source: PathBuf,
    pub destination: Option<PathBuf>,
    /// Identity of the source at plan time.
    pub source_identity: Option<RecordedIdentity>,
    /// Where overwritten content was preserved, if anything was overwritten.
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
    pub subtree_identities: Vec<(PathBuf, RecordedIdentity)>,
    /// Paths that already existed at the destination BEFORE this operation
    /// ran. Undo must never remove these: they are the user's own data that
    /// a merging copy happened to land beside.
    #[serde(default)]
    pub preexisting_at_dest: Vec<PathBuf>,
    pub created_unix: i64,
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
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let line = serde_json::to_string(job)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        // Restrictive from creation — the journal names user paths.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
        }
        writeln!(f, "{line}")?;
        f.sync_all()?;
        Ok(())
    }

    /// Read back all records, skipping a torn final line.
    pub fn load(&self) -> Vec<Job> {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        text.lines()
            .filter_map(|l| serde_json::from_str::<Job>(l).ok())
            .collect()
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
}
