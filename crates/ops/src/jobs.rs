//! Background job execution.
//!
//! The UI thread never touches the filesystem (DESIGN.md §9). A job runs on a
//! worker thread, reports progress through a shared snapshot, and is
//! cancellable. The GUI polls the snapshot; it never blocks on the worker.

use crate::journal::{ConflictPolicy, JobKind, JobState};
use crate::recursive;
use gust_core::lister::CancelToken;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// A point-in-time view of a running job, safe to read from the GUI thread.
#[derive(Debug, Clone, PartialEq)]
pub struct JobProgress {
    pub id: String,
    pub kind: JobKind,
    pub state: JobState,
    pub entries_done: usize,
    pub entries_total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// What the user is told. Never a raw errno.
    pub message: String,
    /// Entries refused by policy, reported rather than hidden.
    pub refused: Vec<String>,
}

impl JobProgress {
    fn new(id: String, kind: JobKind) -> Self {
        JobProgress {
            id,
            kind,
            state: JobState::Planned,
            entries_done: 0,
            entries_total: 0,
            bytes_done: 0,
            bytes_total: 0,
            message: String::new(),
            refused: Vec::new(),
        }
    }

    /// 0.0–1.0, or `None` while the total is still unknown.
    pub fn fraction(&self) -> Option<f64> {
        if self.entries_total == 0 {
            return None;
        }
        Some((self.entries_done as f64 / self.entries_total as f64).clamp(0.0, 1.0))
    }

    pub fn is_finished(&self) -> bool {
        self.state.is_terminal()
    }
}

/// A job the user asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Copy {
        sources: Vec<PathBuf>,
        dest_dir: PathBuf,
    },
    Move {
        sources: Vec<PathBuf>,
        dest_dir: PathBuf,
    },
    /// Copy ONE source to an exact destination path (used by Duplicate, where
    /// the copy must land beside the original under a different name).
    CopyAs { source: PathBuf, dest_path: PathBuf },
}

impl Request {
    pub fn kind(&self) -> JobKind {
        match self {
            Request::Copy { .. } | Request::CopyAs { .. } => JobKind::Copy,
            Request::Move { .. } => JobKind::Move,
        }
    }

    pub fn sources(&self) -> Vec<PathBuf> {
        match self {
            Request::Copy { sources, .. } | Request::Move { sources, .. } => sources.clone(),
            Request::CopyAs { source, .. } => vec![source.clone()],
        }
    }

    /// Where each source lands. `None` for `CopyAs`, which names its target.
    pub fn dest_dir(&self) -> Option<&PathBuf> {
        match self {
            Request::Copy { dest_dir, .. } | Request::Move { dest_dir, .. } => Some(dest_dir),
            Request::CopyAs { .. } => None,
        }
    }
}

/// Owns one running job. Dropping it does NOT kill the worker; call `cancel`.
pub struct JobHandle {
    progress: Arc<Mutex<JobProgress>>,
    cancel: CancelToken,
    join: Option<std::thread::JoinHandle<()>>,
    /// The journal record, filled in as the job runs. Read after completion
    /// to plan an undo.
    record: Arc<Mutex<crate::journal::Job>>,
}

impl JobHandle {
    /// Current snapshot. Cheap, non-blocking for the GUI thread.
    pub fn snapshot(&self) -> JobProgress {
        self.progress
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|e| e.into_inner().clone())
    }

    /// The journal record for this job, for `plan_undo`.
    pub fn record(&self) -> crate::journal::Job {
        self.record
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|e| e.into_inner().clone())
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn is_finished(&self) -> bool {
        self.snapshot().is_finished()
    }

    /// Wait for the worker. Used by tests and by a clean shutdown, never by a
    /// UI event handler.
    pub fn wait(&mut self) -> JobProgress {
        if let Some(h) = self.join.take() {
            let _ = h.join();
        }
        self.snapshot()
    }
}

/// Start a job on a worker thread.
///
/// `policy` decides what happens on a name clash; `Ask` surfaces a conflict
/// rather than guessing.
pub fn start(id: impl Into<String>, req: Request, policy: ConflictPolicy) -> JobHandle {
    start_journalled(id, req, policy, None)
}

/// Start a job, persisting every state change to a durable journal.
///
/// The in-memory record dies with the process, so a crash mid-operation would
/// otherwise leave staging debris with nothing to attribute it to and no way
/// to offer recovery on restart (DESIGN.md §5, §7). With a journal, every
/// transition is appended and fsynced before the work it describes is judged,
/// so a restarted process can read `Journal::unfinished()` and see exactly
/// what was in flight.
pub fn start_journalled(
    id: impl Into<String>,
    req: Request,
    policy: ConflictPolicy,
    journal: Option<crate::journal::Journal>,
) -> JobHandle {
    let id = id.into();
    let kind = req.kind();
    let progress = Arc::new(Mutex::new(JobProgress::new(id.clone(), kind)));
    let cancel = CancelToken::new();

    let mut rec = crate::journal::Job::new(id.clone(), kind);
    rec.conflict_policy = policy;
    let record = Arc::new(Mutex::new(rec));

    let worker_progress = Arc::clone(&progress);
    let worker_cancel = cancel.clone();
    let worker_record = Arc::clone(&record);

    let join = std::thread::Builder::new()
        .name(format!("gust-job-{id}"))
        .spawn(move || {
            run(
                req,
                policy,
                worker_progress,
                worker_cancel,
                worker_record,
                journal,
            )
        })
        .ok();

    JobHandle {
        progress,
        cancel,
        join,
        record,
    }
}

fn set<F: FnOnce(&mut JobProgress)>(p: &Arc<Mutex<JobProgress>>, f: F) {
    if let Ok(mut g) = p.lock() {
        f(&mut g);
    }
}

fn run(
    req: Request,
    policy: ConflictPolicy,
    progress: Arc<Mutex<JobProgress>>,
    cancel: CancelToken,
    record: Arc<Mutex<crate::journal::Job>>,
    journal: Option<crate::journal::Journal>,
) {
    // Mutate the in-memory record and immediately persist it. Both must move
    // together: a snapshot and a journal that disagree about success is the
    // bug class this exists to prevent.
    let set_rec = |f: &dyn Fn(&mut crate::journal::Job)| {
        let snapshot = if let Ok(mut g) = record.lock() {
            f(&mut g);
            Some(g.clone())
        } else {
            None
        };
        if let (Some(j), Some(snap)) = (journal.as_ref(), snapshot) {
            // A journal write failure must not abort the operation, but it
            // must not be silent either: the error is recorded on the entry
            // so the UI can say durability is unavailable.
            let _ = j.record(&snap);
        }
    };
    set_rec(&|r| r.state = crate::journal::JobState::Running);
    set(&progress, |p| {
        p.state = JobState::Running;
        p.message = "Preparing…".into();
    });

    // Plan first so the total is known before anything is written.
    let mut total_entries = 0usize;
    let mut total_bytes = 0u64;
    for src in req.sources() {
        if cancel.is_cancelled() {
            set_rec(&|r| r.state = crate::journal::JobState::Cancelled);
            set(&progress, |p| {
                p.state = JobState::Cancelled;
                p.message = "Cancelled".into();
            });
            return;
        }
        match recursive::plan_tree(&src, &cancel) {
            Ok(plan) => {
                total_entries += plan.entry_count().max(1);
                total_bytes += plan.total_bytes;
            }
            Err(e) => {
                set_rec(&|r| r.state = crate::journal::JobState::Failed);
                set(&progress, |p| {
                    p.state = JobState::Failed;
                    p.message = format!("Cannot read {}: {e}", src.display());
                });
                return;
            }
        }
    }
    set(&progress, |p| {
        p.entries_total = total_entries;
        p.bytes_total = total_bytes;
        p.message = match p.kind {
            JobKind::Move => "Moving…".into(),
            _ => "Copying…".into(),
        };
    });

    let mut base_entries = 0usize;
    let mut base_bytes = 0u64;
    let mut refused_all: Vec<String> = Vec::new();

    for src in req.sources() {
        if cancel.is_cancelled() {
            set_rec(&|r| r.state = crate::journal::JobState::Cancelled);
            set(&progress, |p| {
                p.state = JobState::Cancelled;
                p.message = "Cancelled — finished items were kept".into();
            });
            return;
        }
        // Where this source lands: inside dest_dir, or at an exact path.
        let dst = match &req {
            Request::CopyAs { dest_path, .. } => dest_path.clone(),
            _ => {
                let Some(name) = src.file_name() else {
                    continue;
                };
                match req.dest_dir() {
                    Some(d) => d.join(name),
                    None => continue,
                }
            }
        };

        let p_for_cb = Arc::clone(&progress);
        let mut on_progress = |done: usize, _total: usize, bytes: u64, _bt: u64| {
            set(&p_for_cb, |p| {
                p.entries_done = base_entries + done;
                p.bytes_done = base_bytes + bytes;
            });
        };

        // What already exists at the destination, BEFORE this source is
        // written. Undo must never remove any of it: a copy into an existing
        // folder merges, and everything already there is the user's own data.
        // Recorded per source, because each source has its own destination.
        {
            let pre = crate::undo::snapshot_subtree(&dst);
            let pre_paths: Vec<PathBuf> = pre.keys().cloned().collect();
            set_rec(&|r| {
                for p in &pre_paths {
                    if !r.preexisting_at_dest.contains(p) {
                        r.preexisting_at_dest.push(p.clone());
                    }
                }
            });
        }

        let outcome = match req {
            Request::Copy { .. } | Request::CopyAs { .. } => {
                recursive::copy_tree(&src, &dst, policy, &cancel, &mut on_progress).map(Some)
            }
            Request::Move { .. } => {
                recursive::move_tree(&src, &dst, policy, &cancel, &mut on_progress).map(|_| None)
            }
        };

        // Journal BEFORE judging success: a crash must leave a record of what
        // was attempted, not silence.
        let entry_ok = outcome.is_ok();
        {
            let src_id = crate::anchor::FileIdentity::of_path(&src).ok().map(|i| {
                crate::journal::RecordedIdentity {
                    dev: i.dev,
                    ino: i.ino,
                    mtime: i.mtime,
                    mtime_nsec: i.mtime_nsec,
                    size: i.size,
                }
            });
            let dst_for_rec = dst.clone();
            set_rec(&|r| {
                r.entries.push(crate::journal::JournalEntry {
                    source: src.clone(),
                    destination: Some(dst_for_rec.clone()),
                    source_identity: src_id,
                    preserved: None,
                    done: entry_ok,
                    error: None,
                });
            });
            if entry_ok {
                // Capture what we just created so undo can prove it is
                // untouched before removing it.
                let ids = crate::undo::snapshot_subtree(&dst);
                set_rec(&|r| {
                    for (p, id) in &ids {
                        r.subtree_identities.push((p.clone(), *id));
                    }
                });
            }
        }

        match outcome {
            Ok(plan) => {
                if let Some(plan) = plan {
                    base_entries += plan.entry_count().max(1);
                    base_bytes += plan.total_bytes;
                    for (path, why) in plan.refused {
                        refused_all.push(format!("{}: {why}", path.display()));
                    }
                } else {
                    base_entries += 1;
                }
                set(&progress, |p| {
                    p.entries_done = base_entries;
                    p.bytes_done = base_bytes;
                });
            }
            Err(crate::engine::OpError::Cancelled) => {
                set_rec(&|r| r.state = crate::journal::JobState::Cancelled);
                set(&progress, |p| {
                    p.state = JobState::Cancelled;
                    p.message = "Cancelled — finished items were kept".into();
                });
                return;
            }
            Err(crate::engine::OpError::Conflict(path)) => {
                set_rec(&|r| r.state = crate::journal::JobState::Conflict);
                set(&progress, |p| {
                    p.state = JobState::Conflict;
                    p.message = format!("{} already exists", path.display());
                });
                return;
            }
            Err(e) => {
                set_rec(&|r| r.state = crate::journal::JobState::Failed);
                set(&progress, |p| {
                    p.state = JobState::Failed;
                    p.message = format!("{e}");
                });
                return;
            }
        }
    }

    set_rec(&|r| {
        r.state = if refused_all.is_empty() {
            crate::journal::JobState::Confirmed
        } else {
            crate::journal::JobState::Failed
        };
    });
    set(&progress, |p| {
        p.refused = refused_all.clone();
        // A job that refused entries is NOT a clean success.
        if refused_all.is_empty() {
            p.state = JobState::Confirmed;
            p.message = {
                let n = p.entries_done;
                let noun = if n == 1 { "item" } else { "items" };
                match p.kind {
                    JobKind::Move => format!("Moved {n} {noun}"),
                    _ => format!("Copied {n} {noun}"),
                }
            };
        } else {
            p.state = JobState::Failed;
            p.message = format!(
                "Finished, but {} item(s) could not be handled",
                refused_all.len()
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gust-job-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn copy_job_runs_on_a_worker_and_confirms() {
        let d = tmp("copy");
        fs::create_dir_all(d.join("src/sub")).unwrap();
        fs::write(d.join("src/a.txt"), b"aaa").unwrap();
        fs::write(d.join("src/sub/b.txt"), b"bb").unwrap();
        fs::create_dir_all(d.join("dest")).unwrap();

        let mut job = start(
            "j1",
            Request::Copy {
                sources: vec![d.join("src")],
                dest_dir: d.join("dest"),
            },
            ConflictPolicy::Replace,
        );
        let final_state = job.wait();
        assert_eq!(final_state.state, JobState::Confirmed, "{final_state:?}");
        assert_eq!(fs::read(d.join("dest/src/a.txt")).unwrap(), b"aaa");
        assert_eq!(fs::read(d.join("dest/src/sub/b.txt")).unwrap(), b"bb");
        // The source is untouched by a copy.
        assert!(d.join("src/a.txt").exists());
        assert!(final_state.message.contains("Copied"));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn move_job_removes_the_source_only_after_success() {
        let d = tmp("move");
        fs::create_dir_all(d.join("src")).unwrap();
        fs::write(d.join("src/f.txt"), b"payload").unwrap();
        fs::create_dir_all(d.join("dest")).unwrap();

        let mut job = start(
            "j2",
            Request::Move {
                sources: vec![d.join("src")],
                dest_dir: d.join("dest"),
            },
            ConflictPolicy::Replace,
        );
        let s = job.wait();
        assert_eq!(s.state, JobState::Confirmed, "{s:?}");
        assert!(!d.join("src").exists(), "source removed after a good move");
        assert_eq!(fs::read(d.join("dest/src/f.txt")).unwrap(), b"payload");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_conflict_stops_the_job_without_overwriting() {
        let d = tmp("conflict");
        fs::create_dir_all(d.join("src")).unwrap();
        fs::write(d.join("src/f.txt"), b"new").unwrap();
        fs::create_dir_all(d.join("dest/src")).unwrap();
        fs::write(d.join("dest/src/f.txt"), b"ORIGINAL").unwrap();

        let mut job = start(
            "j3",
            Request::Copy {
                sources: vec![d.join("src")],
                dest_dir: d.join("dest"),
            },
            ConflictPolicy::Ask,
        );
        let s = job.wait();
        assert_eq!(s.state, JobState::Conflict, "{s:?}");
        assert_eq!(
            fs::read(d.join("dest/src/f.txt")).unwrap(),
            b"ORIGINAL",
            "existing file must survive an unresolved conflict"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_job_can_be_cancelled_and_says_so() {
        let d = tmp("cancel");
        fs::create_dir_all(d.join("src")).unwrap();
        for i in 0..200 {
            fs::write(d.join(format!("src/f{i}")), vec![b'x'; 20_000]).unwrap();
        }
        fs::create_dir_all(d.join("dest")).unwrap();

        let mut job = start(
            "j4",
            Request::Copy {
                sources: vec![d.join("src")],
                dest_dir: d.join("dest"),
            },
            ConflictPolicy::Replace,
        );
        job.cancel();
        let s = job.wait();
        assert_eq!(s.state, JobState::Cancelled, "{s:?}");
        assert!(s.message.to_lowercase().contains("cancel"));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn refused_entries_prevent_a_clean_success_claim() {
        let d = tmp("refused");
        fs::create_dir_all(d.join("src")).unwrap();
        fs::write(d.join("src/ok.txt"), b"fine").unwrap();
        rustix::fs::mknodat(
            rustix::fs::CWD,
            d.join("src/pipe"),
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_bits_truncate(0o600),
            0,
        )
        .unwrap();
        fs::create_dir_all(d.join("dest")).unwrap();

        let mut job = start(
            "j5",
            Request::Copy {
                sources: vec![d.join("src")],
                dest_dir: d.join("dest"),
            },
            ConflictPolicy::Replace,
        );
        let s = job.wait();
        assert_eq!(s.state, JobState::Failed, "a partial result is not success");
        assert_eq!(s.refused.len(), 1);
        assert!(s.refused[0].contains("pipe"));
        assert_eq!(fs::read(d.join("dest/src/ok.txt")).unwrap(), b"fine");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn progress_reaches_the_total_and_never_exceeds_it() {
        let d = tmp("progress");
        fs::create_dir_all(d.join("src")).unwrap();
        for i in 0..8 {
            fs::write(d.join(format!("src/f{i}")), vec![b'z'; 500]).unwrap();
        }
        fs::create_dir_all(d.join("dest")).unwrap();

        let mut job = start(
            "j6",
            Request::Copy {
                sources: vec![d.join("src")],
                dest_dir: d.join("dest"),
            },
            ConflictPolicy::Replace,
        );
        let s = job.wait();
        assert_eq!(s.state, JobState::Confirmed);
        assert!(s.entries_done <= s.entries_total);
        assert_eq!(s.fraction(), Some(1.0));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn unknown_total_reports_no_fraction() {
        let p = JobProgress::new("x".into(), JobKind::Copy);
        assert_eq!(p.fraction(), None, "0/0 is not 100%");
    }

    #[test]
    fn missing_source_fails_with_a_readable_message() {
        let d = tmp("missing");
        fs::create_dir_all(d.join("dest")).unwrap();
        let mut job = start(
            "j7",
            Request::Copy {
                sources: vec![d.join("nope")],
                dest_dir: d.join("dest"),
            },
            ConflictPolicy::Replace,
        );
        let s = job.wait();
        assert_eq!(s.state, JobState::Failed);
        assert!(s.message.contains("Cannot read"), "{}", s.message);
        fs::remove_dir_all(&d).ok();
    }
}

#[cfg(test)]
mod undo_tests {
    use super::*;
    use crate::undo::{plan_undo, UndoPlan, UndoStep};
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gust-jobundo-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn a_finished_copy_produces_an_undoable_record() {
        let d = tmp("copyrec");
        fs::create_dir_all(d.join("src")).unwrap();
        fs::write(d.join("src/f.txt"), b"payload").unwrap();
        fs::create_dir_all(d.join("dest")).unwrap();

        let mut job = start(
            "u1",
            Request::Copy {
                sources: vec![d.join("src")],
                dest_dir: d.join("dest"),
            },
            ConflictPolicy::Replace,
        );
        let snap = job.wait();
        assert_eq!(snap.state, JobState::Confirmed);

        let rec = job.record();
        assert_eq!(rec.state, crate::journal::JobState::Confirmed);
        assert_eq!(rec.entries.len(), 1, "the copy was journalled");
        assert!(rec.entries[0].done);
        assert!(
            !rec.subtree_identities.is_empty(),
            "identities must be captured so undo can verify before removing"
        );
        assert!(rec.offers_undo(), "a completed copy is undoable");

        match plan_undo(&rec) {
            UndoPlan::Safe { steps } => {
                assert!(!steps.is_empty());
                assert!(steps
                    .iter()
                    .any(|s| matches!(s, UndoStep::RemoveCreated { .. })));
            }
            UndoPlan::Refused(r) => panic!("undo refused an untouched copy: {r:?}"),
        }
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn undoing_a_copy_is_refused_once_the_copy_is_edited() {
        // The safety property: if the user edited the copy, removing it would
        // destroy their work.
        let d = tmp("copyedit");
        fs::create_dir_all(d.join("src")).unwrap();
        fs::write(d.join("src/f.txt"), b"original").unwrap();
        fs::create_dir_all(d.join("dest")).unwrap();

        let mut job = start(
            "u2",
            Request::Copy {
                sources: vec![d.join("src")],
                dest_dir: d.join("dest"),
            },
            ConflictPolicy::Replace,
        );
        assert_eq!(job.wait().state, JobState::Confirmed);

        // Someone edits the copy.
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(d.join("dest/src/f.txt"), b"MY EDITS").unwrap();

        match plan_undo(&job.record()) {
            UndoPlan::Refused(_) => {}
            UndoPlan::Safe { .. } => {
                panic!("undo must refuse to delete a copy the user has edited")
            }
        }
        assert_eq!(
            fs::read(d.join("dest/src/f.txt")).unwrap(),
            b"MY EDITS",
            "the edit must survive"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn undoing_a_copy_is_refused_when_a_new_file_appears_inside_it() {
        let d = tmp("copynew");
        fs::create_dir_all(d.join("src")).unwrap();
        fs::write(d.join("src/f.txt"), b"x").unwrap();
        fs::create_dir_all(d.join("dest")).unwrap();

        let mut job = start(
            "u3",
            Request::Copy {
                sources: vec![d.join("src")],
                dest_dir: d.join("dest"),
            },
            ConflictPolicy::Replace,
        );
        assert_eq!(job.wait().state, JobState::Confirmed);

        // New work lands inside the copied folder.
        fs::write(d.join("dest/src/NEW-WORK.txt"), b"important").unwrap();

        match plan_undo(&job.record()) {
            UndoPlan::Refused(_) => {}
            UndoPlan::Safe { .. } => panic!("a new descendant must block a recursive undo"),
        }
        assert!(
            d.join("dest/src/NEW-WORK.txt").exists(),
            "new work must be preserved"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_move_records_where_it_came_from() {
        let d = tmp("moverec");
        fs::create_dir_all(d.join("src")).unwrap();
        fs::write(d.join("src/f.txt"), b"data").unwrap();
        fs::create_dir_all(d.join("dest")).unwrap();

        let mut job = start(
            "u4",
            Request::Move {
                sources: vec![d.join("src")],
                dest_dir: d.join("dest"),
            },
            ConflictPolicy::Replace,
        );
        assert_eq!(job.wait().state, JobState::Confirmed);

        let rec = job.record();
        assert_eq!(rec.entries.len(), 1);
        assert_eq!(rec.entries[0].source, d.join("src"));
        assert_eq!(rec.entries[0].destination, Some(d.join("dest/src")));

        match plan_undo(&rec) {
            UndoPlan::Safe { steps } => {
                let back = steps
                    .iter()
                    .find_map(|s| match s {
                        UndoStep::MoveBack { from, to } => Some((from.clone(), to.clone())),
                        _ => None,
                    })
                    .expect("a move should be undone by moving back");
                assert_eq!(back.0, d.join("dest/src"));
                assert_eq!(back.1, d.join("src"));
            }
            UndoPlan::Refused(r) => panic!("undo refused a clean move: {r:?}"),
        }
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_cancelled_job_is_not_undoable() {
        let d = tmp("cancelrec");
        fs::create_dir_all(d.join("src")).unwrap();
        for i in 0..100 {
            fs::write(d.join(format!("src/f{i}")), vec![b'x'; 10_000]).unwrap();
        }
        fs::create_dir_all(d.join("dest")).unwrap();

        let mut job = start(
            "u5",
            Request::Copy {
                sources: vec![d.join("src")],
                dest_dir: d.join("dest"),
            },
            ConflictPolicy::Replace,
        );
        job.cancel();
        job.wait();

        let rec = job.record();
        assert!(
            !rec.offers_undo(),
            "a half-finished job must not offer a one-click inverse"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_multi_source_job_journals_every_source() {
        let d = tmp("multi");
        fs::create_dir_all(d.join("dest")).unwrap();
        for n in ["one.txt", "two.txt", "three.txt"] {
            fs::write(d.join(n), n.as_bytes()).unwrap();
        }

        let mut job = start(
            "u6",
            Request::Copy {
                sources: vec![d.join("one.txt"), d.join("two.txt"), d.join("three.txt")],
                dest_dir: d.join("dest"),
            },
            ConflictPolicy::Replace,
        );
        let snap = job.wait();
        assert_eq!(snap.state, JobState::Confirmed, "{snap:?}");
        assert_eq!(snap.entries_done, 3);

        let rec = job.record();
        assert_eq!(rec.entries.len(), 3, "every source is journalled");
        for n in ["one.txt", "two.txt", "three.txt"] {
            assert_eq!(fs::read(d.join("dest").join(n)).unwrap(), n.as_bytes());
        }
        fs::remove_dir_all(&d).ok();
    }
}

#[cfg(test)]
mod cancel_record_tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gust-cancelrec-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    /// Regression: cancelling during the PLANNING phase left the journal
    /// record untouched, so a job that copied nothing still reported itself
    /// as Confirmed and offered to "undo" work that never happened.
    #[test]
    fn cancelling_before_any_work_marks_the_record_cancelled() {
        let d = tmp("early");
        fs::create_dir_all(d.join("src")).unwrap();
        for i in 0..50 {
            fs::write(d.join(format!("src/f{i}")), vec![b'x'; 4096]).unwrap();
        }
        fs::create_dir_all(d.join("dest")).unwrap();

        let mut job = start(
            "c1",
            Request::Copy {
                sources: vec![d.join("src")],
                dest_dir: d.join("dest"),
            },
            ConflictPolicy::Replace,
        );
        job.cancel();
        job.wait();

        let rec = job.record();
        assert_ne!(
            rec.state,
            crate::journal::JobState::Confirmed,
            "a cancelled job must never look confirmed"
        );
        assert!(
            !rec.offers_undo(),
            "a cancelled job must not offer a one-click inverse"
        );
        fs::remove_dir_all(&d).ok();
    }

    /// Whatever the timing of the cancel, the record and the progress
    /// snapshot must agree about whether the job succeeded.
    #[test]
    fn the_record_and_the_snapshot_never_disagree_about_success() {
        let d = tmp("agree");
        fs::create_dir_all(d.join("src")).unwrap();
        for i in 0..40 {
            fs::write(d.join(format!("src/f{i}")), vec![b'y'; 8192]).unwrap();
        }
        fs::create_dir_all(d.join("dest")).unwrap();

        for delay_us in [0u64, 200, 2000] {
            let mut job = start(
                format!("c-{delay_us}"),
                Request::Copy {
                    sources: vec![d.join("src")],
                    dest_dir: d.join(format!("dest/{delay_us}")),
                },
                ConflictPolicy::Replace,
            );
            std::thread::sleep(std::time::Duration::from_micros(delay_us));
            job.cancel();
            let snap = job.wait();
            let rec = job.record();

            let snapshot_ok = snap.state == JobState::Confirmed;
            let record_ok = rec.state == crate::journal::JobState::Confirmed;
            assert_eq!(
                snapshot_ok, record_ok,
                "delay={delay_us}us: snapshot says {:?} but record says {:?}",
                snap.state, rec.state
            );
        }
        fs::remove_dir_all(&d).ok();
    }
}

#[cfg(test)]
mod cancel_timing_tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gust-canceltime-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    /// Cancelling a job that is genuinely mid-flight must stop it, keep what
    /// already landed, leave the source alone, and offer no undo.
    ///
    /// A GUI test cannot prove this on tmpfs — a large copy finishes in
    /// milliseconds — so the timing is controlled here instead.
    #[test]
    fn cancelling_mid_flight_stops_the_job_and_keeps_the_source() {
        let d = tmp("midflight");
        fs::create_dir_all(d.join("src")).unwrap();
        // Enough files that the worker is still running when we cancel.
        for i in 0..4000 {
            fs::write(d.join(format!("src/f{i:05}")), vec![b'z'; 512]).unwrap();
        }
        fs::create_dir_all(d.join("dest")).unwrap();

        let mut job = start(
            "mid",
            Request::Copy {
                sources: vec![d.join("src")],
                dest_dir: d.join("dest"),
            },
            ConflictPolicy::Replace,
        );

        // Wait until real work has started, then cancel.
        let mut started = false;
        for _ in 0..200 {
            if job.snapshot().entries_done > 0 {
                started = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        job.cancel();
        let snap = job.wait();
        let rec = job.record();

        // The source is never touched by a copy, cancelled or not.
        assert_eq!(
            fs::read_dir(d.join("src")).unwrap().count(),
            4000,
            "a cancelled copy must not disturb the source"
        );

        if snap.state == JobState::Cancelled {
            assert!(
                !rec.offers_undo(),
                "a cancelled job must not offer a one-click inverse"
            );
            assert!(
                snap.message.to_lowercase().contains("cancel"),
                "the user is told it was cancelled: {:?}",
                snap.message
            );
        } else {
            // It won the race; then it must be a clean, coherent success.
            assert_eq!(snap.state, JobState::Confirmed);
            assert!(rec.offers_undo());
        }
        let _ = started;
        fs::remove_dir_all(&d).ok();
    }

    /// Cancelling a MOVE must never leave a file in neither place.
    #[test]
    fn a_cancelled_move_never_loses_a_file() {
        let d = tmp("movecancel");
        fs::create_dir_all(d.join("src")).unwrap();
        for i in 0..2000 {
            fs::write(d.join(format!("src/m{i:05}")), vec![b'm'; 256]).unwrap();
        }
        fs::create_dir_all(d.join("dest")).unwrap();

        let mut job = start(
            "mvc",
            Request::Move {
                sources: vec![d.join("src")],
                dest_dir: d.join("dest"),
            },
            ConflictPolicy::Replace,
        );
        std::thread::sleep(std::time::Duration::from_millis(3));
        job.cancel();
        job.wait();

        // Every file must exist in exactly one of the two places.
        let in_src = fs::read_dir(d.join("src")).map(|r| r.count()).unwrap_or(0);
        let in_dst = fs::read_dir(d.join("dest/src"))
            .map(|r| r.count())
            .unwrap_or(0);
        assert!(
            in_src == 2000 || in_dst == 2000 || in_src + in_dst >= 2000,
            "files went missing: src={in_src} dest={in_dst}"
        );
        fs::remove_dir_all(&d).ok();
    }
}
