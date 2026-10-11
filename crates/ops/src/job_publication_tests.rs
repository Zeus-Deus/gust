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
            None,
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
