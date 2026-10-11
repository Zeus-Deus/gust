//! Deterministic tests at the real staging/publication boundary.
use super::*;
use std::fs::{self, OpenOptions};
use std::io::Write;

thread_local! {
    static BEFORE_PUBLICATION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

pub(crate) fn before_publication() {
    let hook = BEFORE_PUBLICATION.with(|h| h.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

pub(crate) fn at_publication(hook: impl FnOnce() + 'static) {
    BEFORE_PUBLICATION.with(|h| {
        assert!(h.borrow().is_none());
        *h.borrow_mut() = Some(Box::new(hook));
    });
}

#[test]
fn publication_symlink_preserves_a_writer_after_staging() {
    let f = Fixture::new("symlink");
    fs::remove_file(f.0.join("source")).unwrap();
    std::os::unix::fs::symlink("unread-target", f.0.join("source")).unwrap();
    let (sd, sn, dd, dn) = f.handles();
    let observed = std::rc::Rc::new(std::cell::Cell::new(None));
    let observed_at_boundary = observed.clone();
    let root = f.0.clone();
    at_publication(move || {
        assert!(
            fs::symlink_metadata(root.join(staging_name(OsStr::new("destination"))))
                .unwrap()
                .file_type()
                .is_symlink(),
            "the real link has been staged"
        );
        let path = root.join("destination");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(b"BYSTANDER").unwrap();
        file.sync_all().unwrap();
        observed_at_boundary.set(Some(FileIdentity::of_path(&path).unwrap()));
    });
    let outcome = copy_symlink(&sd, &sn, &dd, &dn);
    assert_eq!(
        fs::read(f.0.join("destination")).ok().as_deref(),
        Some(&b"BYSTANDER"[..]),
        "symlink publication overwrote a late writer"
    );
    assert!(observed
        .get()
        .unwrap()
        .matches(&FileIdentity::of_path(&f.0.join("destination")).unwrap()));
    assert_eq!(
        fs::read_link(f.0.join("source")).unwrap(),
        Path::new("unread-target")
    );
    assert!(matches!(outcome, Err(OpError::Conflict(_))), "{outcome:?}");
    assert!(f.0.join(staging_name(&dn)).symlink_metadata().is_err());
}

#[test]
fn publication_same_fs_move_preserves_a_late_writer() {
    let f = Fixture::new("move-ask");
    let observed = std::rc::Rc::new(std::cell::Cell::new(None));
    let at_boundary = observed.clone();
    let root = f.0.clone();
    at_publication(move || {
        let path = root.join("destination");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(b"BYSTANDER").unwrap();
        file.sync_all().unwrap();
        at_boundary.set(Some(FileIdentity::of_path(&path).unwrap()));
    });
    let outcome = crate::recursive::move_tree(
        &f.0.join("source"),
        &f.0.join("destination"),
        ConflictPolicy::Ask,
        &CancelToken::new(),
        &mut |_, _, _, _| {},
    );
    assert_eq!(
        fs::read(f.0.join("source")).ok().as_deref(),
        Some(&b"SOURCE"[..]),
        "a failed publication must retain the source"
    );
    f.assert_preserved(observed.get().unwrap());
    assert!(matches!(outcome, Err(OpError::Conflict(_))), "{outcome:?}");
}

#[test]
fn publication_unsupported_directory_rename_refuses_without_a_fallback() {
    let f = Fixture::new("unsupported-directory");
    fs::remove_file(f.0.join("source")).unwrap();
    fs::create_dir(f.0.join("source")).unwrap();
    fs::write(f.0.join("source/child"), b"SOURCE").unwrap();
    let (sd, sn, dd, dn) = f.handles();
    crate::anchor::UNSUPPORTED_ERRNO.with(|e| e.set(Some(rustix::io::Errno::OPNOTSUPP)));
    let outcome = crate::anchor::rename_noreplace(&sd, &sn, &dd, &dn);
    assert!(
        matches!(&outcome, Err(e) if e.kind() == io::ErrorKind::Unsupported),
        "unsupported atomic rename must refuse, not check then rename: {outcome:?}"
    );
    assert_eq!(fs::read(f.0.join("source/child")).unwrap(), b"SOURCE");
    assert!(!f.0.join("destination").exists());
}

#[test]
fn publication_low_level_move_preserves_an_occupied_name() {
    let f = Fixture::new("low-level-move");
    let (sd, sn, dd, dn) = f.handles();
    let bystander = f.writer("destination");
    let outcome = move_same_fs(&sd, &sn, &dd, &dn);
    assert_eq!(
        fs::read(f.0.join("source")).ok().as_deref(),
        Some(&b"SOURCE"[..]),
        "an occupied destination must not consume the source"
    );
    f.assert_preserved(bystander);
    assert!(matches!(outcome, Err(OpError::Conflict(_))), "{outcome:?}");
}

#[test]
fn publication_inverse_move_preserves_a_late_writer() {
    let f = Fixture::new("inverse-move");
    let observed = std::rc::Rc::new(std::cell::Cell::new(None));
    let at_boundary = observed.clone();
    let root = f.0.clone();
    at_publication(move || {
        let path = root.join("destination");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(b"BYSTANDER").unwrap();
        file.sync_all().unwrap();
        at_boundary.set(Some(FileIdentity::of_path(&path).unwrap()));
    });
    let plan = crate::undo::UndoPlan::Safe {
        steps: vec![crate::undo::UndoStep::MoveBack {
            from: f.0.join("source"),
            to: f.0.join("destination"),
        }],
    };
    let outcome = crate::undo::apply_undo(&plan);
    assert_eq!(
        fs::read(f.0.join("source")).ok().as_deref(),
        Some(&b"SOURCE"[..]),
        "inverse move must retain its source when another writer takes the restore name"
    );
    f.assert_preserved(observed.get().unwrap());
    assert!(!outcome.is_complete(), "{outcome:?}");
    assert_eq!(outcome.steps_done, 0);
    assert_eq!(outcome.problems.len(), 1);
}

fn late_copy_with_policy(policy: ConflictPolicy) {
    let f = Fixture::new(&format!("copy-{policy:?}"));
    let (sd, sn, dd, dn) = f.handles();
    let staged = stage_regular_file(
        &sd,
        &sn,
        &dd,
        &dn,
        policy,
        &CancelToken::new(),
        &mut |_| {},
        true,
    )
    .unwrap()
    .unwrap();
    let bystander = f.writer("destination");
    let outcome = staged.commit(&dd, &dn);
    f.assert_preserved(bystander);
    // A late Skip is a reported conflict, not an ambiguous successful zero-byte
    // copy that a Move caller could mistake for permission to remove its source.
    assert!(matches!(outcome, Err(OpError::Conflict(_))), "{outcome:?}");
}
#[test]
fn publication_staged_copy_skip_preserves_a_late_writer() {
    late_copy_with_policy(ConflictPolicy::Skip);
}
#[test]
fn publication_staged_copy_keep_both_preserves_a_late_writer() {
    late_copy_with_policy(ConflictPolicy::KeepBoth);
}
#[test]
fn publication_staged_copy_replace_preserves_a_late_writer() {
    late_copy_with_policy(ConflictPolicy::Replace);
}
#[test]
fn publication_copy_replace_without_preservation_cannot_clobber() {
    let f = Fixture::new("replace-without-preservation");
    let (sd, sn, dd, dn) = f.handles();
    let bystander = f.writer("destination");
    let outcome = copy_regular_file(
        &sd,
        &sn,
        &dd,
        &dn,
        ConflictPolicy::Replace,
        &CancelToken::new(),
    );
    f.assert_preserved(bystander);
    assert!(matches!(outcome, Err(OpError::Conflict(_))), "{outcome:?}");
}
#[test]
fn publication_unsupported_errors_refuse_every_source_type() {
    for errno in [
        rustix::io::Errno::INVAL,
        rustix::io::Errno::NOSYS,
        rustix::io::Errno::OPNOTSUPP,
    ] {
        for kind in [FileClass::Regular, FileClass::Symlink, FileClass::Directory] {
            let f = Fixture::new(&format!("unsupported-{errno:?}-{kind:?}"));
            match kind {
                FileClass::Symlink => {
                    fs::remove_file(f.0.join("source")).unwrap();
                    std::os::unix::fs::symlink("unread-target", f.0.join("source")).unwrap();
                }
                FileClass::Directory => {
                    fs::remove_file(f.0.join("source")).unwrap();
                    fs::create_dir(f.0.join("source")).unwrap();
                    fs::write(f.0.join("source/child"), b"SOURCE").unwrap();
                }
                _ => {}
            }
            let before = FileIdentity::of_path(&f.0.join("source")).unwrap();
            let (sd, sn, dd, dn) = f.handles();
            crate::anchor::UNSUPPORTED_ERRNO.with(|e| e.set(Some(errno)));
            let outcome = crate::anchor::rename_noreplace(&sd, &sn, &dd, &dn);
            assert!(
                matches!(&outcome, Err(e) if e.kind() == io::ErrorKind::Unsupported),
                "{outcome:?}"
            );
            assert!(before.matches(&FileIdentity::of_path(&f.0.join("source")).unwrap()));
            assert!(!f.0.join("destination").exists());
        }
    }
}
#[test]
fn publication_unsupported_copy_discards_only_its_stage() {
    let f = Fixture::new("unsupported-copy");
    let (sd, sn, dd, dn) = f.handles();
    let staged = stage_regular_file(
        &sd,
        &sn,
        &dd,
        &dn,
        ConflictPolicy::Ask,
        &CancelToken::new(),
        &mut |_| {},
        true,
    )
    .unwrap()
    .unwrap();
    let bystander = f.writer("destination");
    crate::anchor::UNSUPPORTED_ERRNO.with(|e| e.set(Some(rustix::io::Errno::NOSYS)));
    let outcome = staged.commit(&dd, &dn);
    f.assert_preserved(bystander);
    assert!(matches!(outcome, Err(OpError::Io(e)) if e.kind() == io::ErrorKind::Unsupported));
}
#[test]
fn publication_recursive_symlink_skip_keeps_an_existing_item() {
    let f = Fixture::new("symlink-skip");
    fs::remove_file(f.0.join("source")).unwrap();
    std::os::unix::fs::symlink("unread-target", f.0.join("source")).unwrap();
    let bystander = f.writer("destination");
    let outcome = crate::recursive::copy_tree(
        &f.0.join("source"),
        &f.0.join("destination"),
        ConflictPolicy::Skip,
        &CancelToken::new(),
        &mut |_, _, _, _| {},
    );
    assert!(
        outcome.is_ok(),
        "Skip must not fail only because its source is a link: {outcome:?}"
    );
    assert_eq!(fs::read(f.0.join("destination")).unwrap(), b"BYSTANDER");
    assert!(bystander.matches(&FileIdentity::of_path(&f.0.join("destination")).unwrap()));
    assert_eq!(
        fs::read_link(f.0.join("source")).unwrap(),
        Path::new("unread-target")
    );
}

#[test]
fn publication_recursive_batch_preserves_a_writer_after_staging() {
    let f = Fixture::new("batch-copy");
    fs::remove_file(f.0.join("source")).unwrap();
    fs::create_dir(f.0.join("source")).unwrap();
    fs::write(f.0.join("source/child"), b"SOURCE").unwrap();
    let destination = f.0.join("destination/child");
    let mut observed = None;
    let root = f.0.clone();
    let outcome = crate::recursive::copy_tree(
        &f.0.join("source"),
        &f.0.join("destination"),
        ConflictPolicy::Ask,
        &CancelToken::new(),
        &mut |done, _, _, _| {
            if done == 1 && observed.is_none() {
                assert!(
                    !destination.exists(),
                    "must exercise deferred batch publication"
                );
                assert!(fs::symlink_metadata(
                    root.join("destination")
                        .join(staging_name(OsStr::new("child")))
                )
                .unwrap()
                .is_file());
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&destination)
                    .unwrap();
                file.write_all(b"BYSTANDER").unwrap();
                file.sync_all().unwrap();
                observed = Some(FileIdentity::of_path(&destination).unwrap());
            }
        },
    );
    assert!(observed
        .unwrap()
        .matches(&FileIdentity::of_path(&destination).unwrap()));
    assert_eq!(fs::read(&destination).unwrap(), b"BYSTANDER");
    assert_eq!(fs::read(f.0.join("source/child")).unwrap(), b"SOURCE");
    assert!(matches!(outcome, Err(OpError::Conflict(_))), "{outcome:?}");
    assert_eq!(fs::read_dir(f.0.join("destination")).unwrap().count(), 1);
}

struct Fixture(PathBuf);
impl Fixture {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "gust-publication-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("source"), b"SOURCE").unwrap();
        Self(root)
    }

    fn handles(&self) -> (DirHandle, OsString, DirHandle, OsString) {
        let (sd, sn) = resolve_parent(&self.0.join("source")).unwrap();
        let (dd, dn) = resolve_parent(&self.0.join("destination")).unwrap();
        (sd, sn, dd, dn)
    }

    fn writer(&self, name: &str) -> FileIdentity {
        let path = self.0.join(name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(b"BYSTANDER").unwrap();
        file.sync_all().unwrap();
        FileIdentity::of_path(&path).unwrap()
    }

    fn assert_preserved(&self, bystander: FileIdentity) {
        assert_eq!(fs::read(self.0.join("source")).unwrap(), b"SOURCE");
        assert_eq!(
            fs::read(self.0.join("destination")).unwrap(),
            b"BYSTANDER",
            "a writer's object created after staging must never be overwritten"
        );
        assert!(bystander.matches(&FileIdentity::of_path(&self.0.join("destination")).unwrap()));
        assert!(!fs::read_dir(&self.0).unwrap().flatten().any(|e| e
            .file_name()
            .to_string_lossy()
            .starts_with(".gust-staging-")));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn publication_staged_copy_ask_preserves_a_late_writer() {
    let f = Fixture::new("copy-ask");
    let (sd, sn, dd, dn) = f.handles();
    let staged = stage_regular_file(
        &sd,
        &sn,
        &dd,
        &dn,
        ConflictPolicy::Ask,
        &CancelToken::new(),
        &mut |_| {},
        true,
    )
    .unwrap()
    .unwrap();
    assert!(
        !f.0.join("destination").exists(),
        "staging is not publication"
    );
    let bystander = f.writer("destination");
    let outcome = staged.commit(&dd, &dn);
    f.assert_preserved(bystander);
    assert!(matches!(outcome, Err(OpError::Conflict(_))), "{outcome:?}");
}
