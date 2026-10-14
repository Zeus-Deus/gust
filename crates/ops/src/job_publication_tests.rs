//! Exercise the actual synchronous worker body, not a model of its lifecycle.
use super::*;
use crate::anchor::FileIdentity;
use crate::engine::{publication_tests::at_publication, FileClass};
use crate::trash::TrashDir;
use std::fs::{self, OpenOptions};
use std::io::Write;

struct Fixture {
    root: PathBuf,
    source: PathBuf,
    destination: PathBuf,
    trash: TrashDir,
}
impl Fixture {
    fn new(tag: &str, class: FileClass) -> Self {
        let root = std::env::temp_dir().join(format!(
            "gust-job-publication-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("src")).unwrap();
        fs::create_dir(root.join("dest")).unwrap();
        let source = root.join("src/item");
        match class {
            FileClass::Symlink => std::os::unix::fs::symlink("missing-target", &source).unwrap(),
            FileClass::Directory => {
                fs::create_dir(&source).unwrap();
                fs::write(source.join("child"), b"SOURCE").unwrap();
            }
            _ => fs::write(&source, b"SOURCE").unwrap(),
        }
        let destination = root.join("dest/item");
        let trash = TrashDir::at(root.join("Trash"));
        Self {
            root,
            source,
            destination,
            trash,
        }
    }

    fn run(&self, kind: JobKind, policy: ConflictPolicy) -> (JobProgress, crate::journal::Job) {
        self.run_with(kind, policy, None)
    }

    fn run_with(
        &self,
        kind: JobKind,
        policy: ConflictPolicy,
        journal: Option<crate::journal::Journal>,
    ) -> (JobProgress, crate::journal::Job) {
        let req = match kind {
            JobKind::Copy => Request::Copy {
                sources: vec![self.source.clone()],
                dest_dir: self.root.join("dest"),
            },
            JobKind::Move => Request::Move {
                sources: vec![self.source.clone()],
                dest_dir: self.root.join("dest"),
            },
            _ => unreachable!(),
        };
        let progress = Arc::new(Mutex::new(JobProgress::new("publication".into(), kind)));
        let record = Arc::new(Mutex::new(crate::journal::Job::new("publication", kind)));
        run(
            req,
            policy,
            Default::default(),
            progress.clone(),
            CancelToken::new(),
            record.clone(),
            journal,
            Some(self.trash.clone()),
        );
        let progress = progress.lock().unwrap().clone();
        let record = record.lock().unwrap().clone();
        (progress, record)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn publication_job_late_writers_are_not_confirmed_or_adopted() {
    for kind in [JobKind::Copy, JobKind::Move] {
        for class in [FileClass::Regular, FileClass::Symlink, FileClass::Directory] {
            if kind == JobKind::Copy && class == FileClass::Directory {
                continue; // Child-file publication is covered separately by the real batched walk.
            }
            for policy in [
                ConflictPolicy::Ask,
                ConflictPolicy::Skip,
                ConflictPolicy::KeepBoth,
                ConflictPolicy::Replace,
            ] {
                let f = Fixture::new(&format!("late-{kind:?}-{class:?}-{policy:?}"), class);
                let source_id = FileIdentity::of_path(&f.source).unwrap();
                let target = if policy == ConflictPolicy::KeepBoth {
                    fs::write(&f.destination, b"APPROVED-OLD").unwrap();
                    f.root.join("dest/item (copy)")
                } else {
                    if policy == ConflictPolicy::Replace {
                        fs::write(&f.destination, b"APPROVED-OLD").unwrap();
                    }
                    f.destination.clone()
                };
                let observed = std::rc::Rc::new(std::cell::Cell::new(None));
                let at_boundary = observed.clone();
                let writer_target = target.clone();
                let preserved = f.trash.files.join("item");
                at_publication(move || {
                    if policy == ConflictPolicy::Replace {
                        assert_eq!(
                            fs::read(preserved).unwrap(),
                            b"APPROVED-OLD",
                            "Replace must preserve the approved old object before publication"
                        );
                    }
                    let mut file = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&writer_target)
                        .unwrap();
                    file.write_all(b"BYSTANDER").unwrap();
                    file.sync_all().unwrap();
                    at_boundary.set(Some(FileIdentity::of_path(&writer_target).unwrap()));
                });
                let (progress, record) = f.run(kind, policy);
                assert_eq!(
                    fs::read(&target).unwrap(),
                    b"BYSTANDER",
                    "{kind:?}/{class:?}/{policy:?}"
                );
                assert!(observed
                    .get()
                    .unwrap()
                    .matches(&FileIdentity::of_path(&target).unwrap()));
                assert!(source_id.matches(&FileIdentity::of_path(&f.source).unwrap()));
                assert_eq!(progress.state, JobState::Conflict, "{progress:?}");
                assert_eq!(record.state, JobState::Conflict);
                assert_eq!(record.entries.len(), 1);
                assert!(!record.entries[0].done);
                assert!(
                    record.subtree_identities.is_empty(),
                    "a bystander must never become an undo-owned copy"
                );
                assert!(!record.offers_undo());
                if policy == ConflictPolicy::Replace {
                    assert_eq!(
                        record.entries[0].preserved,
                        Some(f.trash.files.join("item"))
                    );
                    assert_eq!(
                        fs::read(f.trash.files.join("item")).unwrap(),
                        b"APPROVED-OLD"
                    );
                }
            }
        }
    }
}

#[test]
fn publication_job_initial_policy_lifecycle_and_replace_undo() {
    for kind in [JobKind::Copy, JobKind::Move] {
        for class in [FileClass::Regular, FileClass::Symlink] {
            for policy in [
                ConflictPolicy::Ask,
                ConflictPolicy::Skip,
                ConflictPolicy::KeepBoth,
                ConflictPolicy::Replace,
            ] {
                let f = Fixture::new(&format!("initial-{kind:?}-{class:?}-{policy:?}"), class);
                fs::write(&f.destination, b"APPROVED-OLD").unwrap();
                let old_id = FileIdentity::of_path(&f.destination).unwrap();
                let (progress, record) = f.run(kind, policy);
                if policy == ConflictPolicy::Ask {
                    assert_eq!(progress.state, JobState::Conflict);
                    assert_eq!(fs::read(&f.destination).unwrap(), b"APPROVED-OLD");
                    assert!(old_id.matches(&FileIdentity::of_path(&f.destination).unwrap()));
                    assert!(f.source.symlink_metadata().is_ok());
                    continue;
                }
                assert_eq!(progress.state, JobState::Confirmed, "{progress:?}");
                if policy == ConflictPolicy::Skip {
                    assert_eq!(fs::read(&f.destination).unwrap(), b"APPROVED-OLD");
                    assert!(old_id.matches(&FileIdentity::of_path(&f.destination).unwrap()));
                    assert!(f.source.symlink_metadata().is_ok());
                    assert!(record.entries.is_empty());
                    continue;
                }
                let target = if policy == ConflictPolicy::KeepBoth {
                    assert_eq!(fs::read(&f.destination).unwrap(), b"APPROVED-OLD");
                    f.root.join("dest/item (copy)")
                } else {
                    assert_eq!(
                        fs::read(f.trash.files.join("item")).unwrap(),
                        b"APPROVED-OLD"
                    );
                    f.destination.clone()
                };
                match class {
                    FileClass::Symlink => {
                        assert_eq!(fs::read_link(target).unwrap(), Path::new("missing-target"))
                    }
                    _ => assert_eq!(fs::read(target).unwrap(), b"SOURCE"),
                }
                assert_eq!(f.source.symlink_metadata().is_ok(), kind == JobKind::Copy);
                let undo = crate::undo::apply_undo(&crate::undo::plan_undo(&record));
                assert!(undo.is_complete(), "{undo:?}");
                assert!(f.source.symlink_metadata().is_ok());
                assert_eq!(fs::read(&f.destination).unwrap(), b"APPROVED-OLD");
                assert!(old_id.matches(&FileIdentity::of_path(&f.destination).unwrap()));
            }
        }
    }
}

/// Names left in `dir` that look like Gust staging debris.
fn staging_debris(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".gust-staging"))
        .collect()
}

/// A recovery record that cannot be saved stops the job before anything is
/// written, and says so. Found by review: `jobs.rs` discarded the journal's
/// error and `engine.rs` turned a failed staging claim into `None`, so a full
/// home disk let copies and moves go ahead with no way to recover them.
#[test]
fn a_staging_claim_that_cannot_be_saved_stops_the_copy() {
    for (tag, which) in [("claim", 0), ("confirm", 1)] {
        // A same-disk move is one rename and stages nothing; the copy path
        // (which a move across disks also takes) is where staging happens.
        for kind in [JobKind::Copy] {
            for class in [FileClass::Regular, FileClass::Symlink] {
                let f = Fixture::new(&format!("nofault-{tag}-{kind:?}-{class:?}"), class);
                let flag = if which == 0 {
                    &crate::staging::fault::CLAIM
                } else {
                    &crate::staging::fault::CONFIRM
                };
                flag.with(|c| c.set(true));
                let (progress, record) = f.run(kind, ConflictPolicy::Ask);
                flag.with(|c| c.set(false));
                let what = format!("{tag}/{kind:?}/{class:?}");
                assert_eq!(progress.state, JobState::Failed, "{what}: {progress:?}");
                assert!(
                    progress.message.contains("recovery record"),
                    "{what}: {}",
                    progress.message
                );
                assert!(
                    !progress.message.contains("os error"),
                    "{}",
                    progress.message
                );
                assert!(
                    f.destination.symlink_metadata().is_err(),
                    "{what}: nothing may be published without a recovery record"
                );
                assert!(
                    staging_debris(&f.root.join("dest")).is_empty(),
                    "{what}: no staging file may be left behind"
                );
                assert!(f.source.symlink_metadata().is_ok(), "{what}: source kept");
                assert_eq!(record.state, JobState::Failed, "{what}");
                assert!(!record.offers_undo(), "{what}");
            }
        }
    }
}

#[test]
fn a_journal_that_cannot_be_written_stops_the_job_before_any_byte() {
    for kind in [JobKind::Copy, JobKind::Move] {
        let f = Fixture::new(&format!("nojournal-{kind:?}"), FileClass::Regular);
        // A regular file where the journal's folder must go: every write
        // fails (ENOTDIR), exactly like an unwritable state folder.
        fs::write(f.root.join("blocker"), b"not a folder").unwrap();
        let journal = crate::journal::Journal::new(f.root.join("blocker/journal.ndjson"));
        let (progress, record) = f.run_with(kind, ConflictPolicy::Ask, Some(journal));
        assert_eq!(progress.state, JobState::Failed, "{kind:?}: {progress:?}");
        assert!(
            progress.message.contains("recovery record"),
            "{kind:?}: {}",
            progress.message
        );
        assert!(
            f.destination.symlink_metadata().is_err(),
            "{kind:?}: nothing written"
        );
        assert_eq!(
            fs::read(&f.source).unwrap(),
            b"SOURCE",
            "{kind:?}: source kept"
        );
        assert_eq!(record.state, JobState::Failed);
        assert!(!record.offers_undo());
    }
}
