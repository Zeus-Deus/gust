//! Deterministic tests at the real staging/publication boundary.
use super::*;
use std::fs::{self, OpenOptions};
use std::io::Write;

thread_local! {
    pub(crate) static WRITE_LIMIT: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
    static AFTER_WRITE: std::cell::RefCell<Option<Box<dyn FnMut()>>> = const { std::cell::RefCell::new(None) };
    pub(crate) static DISCARD_ERROR: std::cell::Cell<Option<rustix::io::Errno>> = const { std::cell::Cell::new(None) };
    pub(crate) static USE_LINK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static BEFORE_PUBLICATION: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static AFTER_LINK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

pub(crate) fn after_write() {
    AFTER_WRITE.with(|v| {
        if let Some(hook) = v.borrow_mut().as_mut() {
            hook();
        }
    });
}

pub(crate) fn after_link() {
    if let Some(hook) = AFTER_LINK.with(|h| h.borrow_mut().take()) {
        hook();
    }
}

struct LinkPublication;
impl LinkPublication {
    fn new(root: &Path) -> Self {
        crate::staging::fault::state_home(&root.join("state"));
        USE_LINK.with(|v| v.set(true));
        Self
    }
}
impl Drop for LinkPublication {
    fn drop(&mut self) {
        USE_LINK.with(|v| v.set(false));
        crate::anchor::LINK_ERROR.with(|v| v.set(None));
        crate::anchor::LINK_REPLY_ERROR.with(|v| v.set(None));
        WRITE_LIMIT.with(|v| v.set(usize::MAX));
        AFTER_WRITE.with(|v| *v.borrow_mut() = None);
    }
}

fn staged_link(f: &Fixture) -> (Staged, DirHandle, OsString) {
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
    (staged, dd, dn)
}

#[test]
fn publication_link_preserves_occupied_files_directories_and_symlinks() {
    for kind in [FileClass::Regular, FileClass::Directory, FileClass::Symlink] {
        let f = Fixture::new(&format!("link-conflict-{kind:?}"));
        let _link = LinkPublication::new(&f.0);
        let (staged, dd, dn) = staged_link(&f);
        let destination = f.0.join("destination");
        match kind {
            FileClass::Regular => {
                fs::write(&destination, b"BYSTANDER").unwrap();
            }
            FileClass::Directory => {
                fs::create_dir(&destination).unwrap();
            }
            FileClass::Symlink => {
                std::os::unix::fs::symlink("source", &destination).unwrap();
            }
            _ => unreachable!(),
        }
        let before = FileIdentity::of_path(&destination).unwrap();
        assert!(matches!(staged.commit(&dd, &dn), Err(OpError::Conflict(_))));
        assert!(before.matches(&FileIdentity::of_path(&destination).unwrap()));
        assert_eq!(fs::read(f.0.join("source")).unwrap(), b"SOURCE");
        assert!(has_stage(&f.0), "failure retains recovery evidence");
        assert!(!crate::staging::load_all().is_empty());
    }
}

#[test]
fn publication_link_preflight_refuses_before_payload_or_final_name() {
    let f = Fixture::new("link-unsupported");
    let _link = LinkPublication::new(&f.0);
    fs::write(f.0.join("source"), vec![7; 2 * 1024 * 1024]).unwrap();
    let (sd, sn, dd, dn) = f.handles();
    crate::anchor::LINK_ERROR.with(|v| v.set(Some(rustix::io::Errno::NOSYS)));
    let mut progress = 0;
    let result = copy_regular_file_with(
        &sd,
        &sn,
        &dd,
        &dn,
        ConflictPolicy::Ask,
        &CancelToken::new(),
        &mut |n| progress = n,
    );
    assert!(result.is_err());
    assert_eq!(progress, 0);
    assert!(!f.0.join("destination").exists());
    assert!(!has_stage(&f.0));
    assert!(
        !crate::staging::load_all().is_empty(),
        "unknown probe retains its claim"
    );
}

#[test]
fn publication_link_lost_reply_never_replays_or_authorizes_source_removal() {
    let f = Fixture::new("link-lost-reply");
    let _link = LinkPublication::new(&f.0);
    let (sd, sn, dd, dn) = f.handles();
    at_publication(|| crate::anchor::LINK_REPLY_ERROR.with(|v| v.set(Some(rustix::io::Errno::IO))));
    let result = move_cross_fs(&sd, &sn, &dd, &dn, ConflictPolicy::Ask, &CancelToken::new());
    assert!(matches!(result, Err(OpError::Io(e)) if e.to_string().contains("not confirmed")));
    assert_eq!(fs::read(f.0.join("source")).unwrap(), b"SOURCE");
    assert_eq!(fs::read(f.0.join("destination")).unwrap(), b"SOURCE");
    assert!(has_stage(&f.0));
    assert!(!crate::staging::load_all().is_empty());
    // A manual retry encounters the existing complete name; it is not replayed.
    assert!(matches!(
        copy_regular_file(&sd, &sn, &dd, &dn, ConflictPolicy::Ask, &CancelToken::new()),
        Err(OpError::Conflict(_))
    ));
}

#[test]
fn publication_link_success_survives_alias_cleanup_failure() {
    let f = Fixture::new("link-cleanup-failure");
    let _link = LinkPublication::new(&f.0);
    let (staged, dd, dn) = staged_link(&f);
    AFTER_LINK.with(|h| {
        *h.borrow_mut() = Some(Box::new(|| {
            DISCARD_ERROR.with(|v| v.set(Some(rustix::io::Errno::ACCESS)));
        }))
    });
    let result = staged.commit(&dd, &dn);
    assert_eq!(result.unwrap(), 6);
    assert_eq!(fs::read(f.0.join("destination")).unwrap(), b"SOURCE");
    assert!(has_stage(&f.0));
    assert!(!crate::staging::load_all().is_empty());
    assert_eq!(sweep_stale_staging(&f.0), 1);
    assert_eq!(fs::read(f.0.join("destination")).unwrap(), b"SOURCE");
}

#[test]
fn publication_link_crash_after_receipt_reclaims_only_private_alias() {
    let f = Fixture::new("link-crash");
    let _link = LinkPublication::new(&f.0);
    let (staged, dd, dn) = staged_link(&f);
    AFTER_LINK
        .with(|h| *h.borrow_mut() = Some(Box::new(|| panic!("interrupted after link receipt"))));
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| staged.commit(&dd, &dn))).is_err()
    );
    assert_eq!(fs::read(f.0.join("destination")).unwrap(), b"SOURCE");
    assert!(has_stage(&f.0));
    assert_eq!(sweep_stale_staging(&f.0), 1);
    assert_eq!(fs::read(f.0.join("destination")).unwrap(), b"SOURCE");
    assert_eq!(fs::read(f.0.join("source")).unwrap(), b"SOURCE");
}

#[test]
fn publication_link_mode_does_not_change_source_move_or_symlink_contract() {
    let f = Fixture::new("link-scope");
    let _link = LinkPublication::new(&f.0);
    let (sd, sn, dd, dn) = f.handles();
    crate::anchor::UNSUPPORTED_ERRNO.with(|v| v.set(Some(rustix::io::Errno::INVAL)));
    assert!(move_same_fs(&sd, &sn, &dd, &dn).is_err());
    fs::remove_file(f.0.join("source")).unwrap();
    std::os::unix::fs::symlink("unread", f.0.join("source")).unwrap();
    crate::anchor::UNSUPPORTED_ERRNO.with(|v| v.set(Some(rustix::io::Errno::INVAL)));
    assert!(copy_symlink(&sd, &sn, &dd, &dn).is_err());
    assert!(fs::symlink_metadata(f.0.join("source")).is_ok());
    assert!(!f.0.join("destination").exists());
}

#[test]
fn publication_observed_cancellation_stops_before_rename_or_link() {
    for link in [false, true] {
        let f = Fixture::new(&format!("cancel-publication-{link}"));
        let _link = LinkPublication::new(&f.0);
        USE_LINK.with(|v| v.set(link));
        let (sd, sn, dd, dn) = f.handles();
        let cancel = CancelToken::new();
        let staged = stage_regular_file(
            &sd,
            &sn,
            &dd,
            &dn,
            ConflictPolicy::Ask,
            &cancel,
            &mut |_| {},
            true,
        )
        .unwrap()
        .unwrap();
        cancel.cancel();
        assert!(matches!(staged.commit(&dd, &dn), Err(OpError::Cancelled)));
        assert!(!f.0.join("destination").exists());
        assert!(!has_stage(&f.0));
        assert_eq!(fs::read(f.0.join("source")).unwrap(), b"SOURCE");
    }
}

#[test]
fn publication_cancellation_after_link_receipt_preserves_success() {
    let f = Fixture::new("cancel-after-link");
    let _link = LinkPublication::new(&f.0);
    let (sd, sn, dd, dn) = f.handles();
    let cancel = CancelToken::new();
    let staged = stage_regular_file(
        &sd,
        &sn,
        &dd,
        &dn,
        ConflictPolicy::Ask,
        &cancel,
        &mut |_| {},
        true,
    )
    .unwrap()
    .unwrap();
    let during_link = cancel.clone();
    AFTER_LINK.with(|h| *h.borrow_mut() = Some(Box::new(move || during_link.cancel())));
    assert_eq!(staged.commit(&dd, &dn).unwrap(), 6);
    assert!(cancel.is_cancelled());
    assert_eq!(fs::read(f.0.join("destination")).unwrap(), b"SOURCE");
}

#[test]
fn publication_cancellation_after_last_read_stops_before_metadata_and_fsync() {
    let f = Fixture::new("cancel-last-read");
    let _link = LinkPublication::new(&f.0);
    fs::write(f.0.join("source"), vec![7; 1024 * 1024]).unwrap();
    let (sd, sn, dd, dn) = f.handles();
    let cancel = CancelToken::new();
    let result = copy_regular_file_with(
        &sd,
        &sn,
        &dd,
        &dn,
        ConflictPolicy::Ask,
        &cancel,
        &mut |_| cancel.cancel(),
    );
    assert!(matches!(result, Err(OpError::Cancelled)));
    assert!(!f.0.join("destination").exists());
    assert!(!has_stage(&f.0));
}

#[test]
fn publication_skipped_move_has_no_receipt_authorizing_source_removal() {
    let f = Fixture::new("move-skip-receipt");
    let _link = LinkPublication::new(&f.0);
    let (sd, sn, dd, dn) = f.handles();
    let before = f.writer("destination");
    move_cross_fs(
        &sd,
        &sn,
        &dd,
        &dn,
        ConflictPolicy::Skip,
        &CancelToken::new(),
    )
    .unwrap();
    f.assert_preserved(before);
}

#[test]
fn publication_move_cancelled_after_copy_keeps_receipt_and_source() {
    let f = Fixture::new("move-cancel-after-copy");
    let _link = LinkPublication::new(&f.0);
    let (sd, sn, dd, dn) = f.handles();
    let cancel = CancelToken::new();
    let during_link = cancel.clone();
    AFTER_LINK.with(|h| *h.borrow_mut() = Some(Box::new(move || during_link.cancel())));
    assert!(
        matches!(move_cross_fs(&sd, &sn, &dd, &dn, ConflictPolicy::Ask, &cancel),
        Err(OpError::Refused(message)) if message.contains("destination was published"))
    );
    assert_eq!(fs::read(f.0.join("source")).unwrap(), b"SOURCE");
    assert_eq!(fs::read(f.0.join("destination")).unwrap(), b"SOURCE");
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
            fs::symlink_metadata(stage_path(&root))
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
    assert!(!has_stage(&f.0));
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
                assert!(fs::symlink_metadata(stage_path(&root.join("destination")))
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

#[test]
fn publication_changed_stage_is_never_published_or_cleaned_as_ours() {
    let f = Fixture::new("stage-replaced");
    let _link = LinkPublication::new(&f.0);
    let (staged, dd, dn) = staged_link(&f);
    let path = stage_path(&f.0);
    fs::rename(&path, f.0.join("held-stage")).unwrap();
    std::os::unix::fs::symlink("source", &path).unwrap();
    assert!(matches!(staged.commit(&dd, &dn), Err(OpError::Refused(_))));
    assert!(!f.0.join("destination").exists());
    assert_eq!(fs::read_link(path).unwrap(), Path::new("source"));
    assert_eq!(sweep_stale_staging(&f.0), 0);
    assert!(!crate::staging::load_all().is_empty());
}

#[test]
fn publication_pruning_retains_unconfirmed_and_unmounted_network_claims() {
    let f = Fixture::new("claim-prune");
    let _link = LinkPublication::new(&f.0);
    let unknown = crate::staging::claim(&f.0.join("absent-probe")).unwrap();
    let remote_path = f.0.join("remote-stage");
    let remote = crate::staging::claim(&remote_path).unwrap();
    fs::write(&remote_path, b"complete").unwrap();
    crate::staging::confirm(&remote, &remote_path).unwrap();
    crate::staging::fault::rewrite_claim(&remote, |c| c.net = true);
    fs::remove_file(remote_path).unwrap();
    let local_path = f.0.join("local-stage");
    let local = crate::staging::claim(&local_path).unwrap();
    fs::write(&local_path, b"complete").unwrap();
    crate::staging::confirm(&local, &local_path).unwrap();
    fs::remove_file(local_path).unwrap();
    assert_eq!(prune_dead_claims(), 1);
    assert!(!local.exists());
    assert!(remote.exists());
    assert!(unknown.exists());
}

#[test]
fn publication_unknown_mount_evidence_cannot_authorize_move_durability() {
    assert!(require_move_durability_on(None).is_err());
    assert!(require_move_durability_on(Some("")).is_err());
    assert!(require_move_durability_on(Some("fuse.sshfs")).is_err());
}

#[test]
fn publication_link_cleanup_does_not_unlink_a_replaced_alias() {
    let f = Fixture::new("link-alias-replaced");
    let _link = LinkPublication::new(&f.0);
    let (staged, dd, dn) = staged_link(&f);
    let root = f.0.clone();
    let stage = staged.stage.clone();
    AFTER_LINK.with(|h| {
        *h.borrow_mut() = Some(Box::new(move || {
            fs::rename(root.join(&stage), root.join("original-alias")).unwrap();
            fs::write(root.join(stage), b"BYSTANDER").unwrap();
        }))
    });
    assert_eq!(staged.commit(&dd, &dn).unwrap(), 6);
    assert_eq!(fs::read(f.0.join("destination")).unwrap(), b"SOURCE");
    assert_eq!(fs::read(stage_path(&f.0)).unwrap(), b"BYSTANDER");
    assert!(!crate::staging::load_all().is_empty());
    assert_eq!(sweep_stale_staging(&f.0), 0);
}

fn has_stage(root: &Path) -> bool {
    fs::read_dir(root).unwrap().flatten().any(|e| {
        e.file_name()
            .to_string_lossy()
            .starts_with(".gust-staging-")
    })
}

fn stage_path(root: &Path) -> PathBuf {
    fs::read_dir(root)
        .unwrap()
        .flatten()
        .find(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".gust-staging-")
        })
        .unwrap()
        .path()
}

#[test]
fn publication_explicit_retry_uses_a_new_stage_without_losing_old_unknown_evidence() {
    let f = Fixture::new("retry-claim");
    let _link = LinkPublication::new(&f.0);
    let (staged, dd, dn) = staged_link(&f);
    let previous_path = staged.staged_abs.clone();
    let previous_claim = staged.claim.clone().unwrap();
    crate::anchor::LINK_ERROR.with(|v| v.set(Some(rustix::io::Errno::IO)));
    assert!(staged.commit(&dd, &dn).is_err());
    let evidence = fs::read(&previous_claim).unwrap();
    let (retry, dd, dn) = staged_link(&f);
    assert_ne!(retry.staged_abs, previous_path);
    assert_eq!(retry.commit(&dd, &dn).unwrap(), 6);
    assert_eq!(fs::read(previous_claim).unwrap(), evidence);
    assert_eq!(fs::read(previous_path).unwrap(), b"SOURCE");
    assert_eq!(fs::read(f.0.join("destination")).unwrap(), b"SOURCE");
    assert_eq!(fs::read(f.0.join("source")).unwrap(), b"SOURCE");
}

#[test]
fn publication_cancellation_between_short_writes_stops_remaining_bytes() {
    let f = Fixture::new("cancel-short-write");
    let _link = LinkPublication::new(&f.0);
    let (sd, sn, dd, dn) = f.handles();
    let cancel = CancelToken::new();
    let at_write = cancel.clone();
    let root = f.0.clone();
    WRITE_LIMIT.with(|v| v.set(2));
    AFTER_WRITE.with(|v| {
        *v.borrow_mut() = Some(Box::new(move || {
            assert!(
                !at_write.is_cancelled(),
                "another write was admitted after observed cancellation"
            );
            assert_eq!(fs::read(stage_path(&root)).unwrap(), b"SO");
            at_write.cancel();
        }))
    });
    assert!(matches!(
        copy_regular_file(&sd, &sn, &dd, &dn, ConflictPolicy::Ask, &cancel),
        Err(OpError::Cancelled)
    ));
    assert!(!has_stage(&f.0));
    assert!(!f.0.join("destination").exists());
    assert_eq!(fs::read(f.0.join("source")).unwrap(), b"SOURCE");
}
