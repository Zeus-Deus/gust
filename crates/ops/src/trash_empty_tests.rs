use super::*;
use std::{fs, os::unix::fs::PermissionsExt};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "gust-empty-cuts-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(path.join("work/tree")).unwrap();
        fs::write(path.join("work/tree/child"), b"original child").unwrap();
        Self(path)
    }
    fn location(&self) -> TrashLocation {
        TrashLocation {
            trash: TrashDir::at(self.0.join("Trash")),
            topdir: None,
        }
    }
    fn receipt(&self) -> TrashReceipt {
        self.location()
            .trash_path(&self.0.join("work/tree"), &CancelToken::new())
            .unwrap()
    }
    fn history(&self) -> History {
        History::open(self.0.join("state/history.json")).unwrap()
    }
    fn run(&self) -> PathBuf {
        fs::read_dir(self.location().trash.root())
            .unwrap()
            .filter_map(Result::ok)
            .find(|e| e.file_name().as_bytes().starts_with(b".gust-empty-"))
            .unwrap()
            .path()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn late_deep_edit_cannot_be_resnapshotted_into_deletion_authority() {
    let f = Fixture::new();
    f.receipt();
    let plan = prepare_empty(&[f.location()], &CancelToken::new())
        .unwrap()
        .confirm_irreversible();
    let root = f.0.clone();
    BEFORE_DELETE.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let run = fs::read_dir(root.join("Trash"))
                .unwrap()
                .filter_map(Result::ok)
                .find(|e| e.file_name().as_bytes().starts_with(b".gust-empty-"))
                .unwrap()
                .path();
            fs::write(run.join("item-000000/data/child"), b"later child work").unwrap();
        }))
    });
    let e = plan
        .execute(&mut f.history(), &CancelToken::new(), |_| {})
        .unwrap_err();
    assert_eq!(e.reason, EmptyTrashFailure::Delete(DeleteFailure::Changed));
    assert_eq!(e.outcome.removed, Removed::default());
    assert_eq!(
        fs::read(f.run().join("item-000000/data/child")).unwrap(),
        b"later child work"
    );
    assert!(!fs::read_dir(f.run().join("item-000000"))
        .unwrap()
        .filter_map(Result::ok)
        .any(|e| e.file_name().as_bytes().starts_with(b".gust-delete-")));
}
#[test]
fn replacement_at_data_claim_is_returned_without_deletion() {
    let f = Fixture::new();
    let r = f.receipt();
    let source = r.data_path();
    let saved = f.0.join("saved-original");
    let plan = prepare_empty(&[f.location()], &CancelToken::new())
        .unwrap()
        .confirm_irreversible();
    let at = source.clone();
    let saved_at = saved.clone();
    AFTER_MOVE.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            BEFORE_MOVE.with(|next| {
                *next.borrow_mut() = Some(Box::new(move || {
                    fs::rename(&at, &saved_at).unwrap();
                    fs::write(&at, b"replacement at claim").unwrap();
                }))
            });
        }))
    });
    let e = plan
        .execute(&mut f.history(), &CancelToken::new(), |_| {})
        .unwrap_err();
    assert_eq!(e.outcome.removed, Removed::default());
    assert_eq!(fs::read(&source).unwrap(), b"replacement at claim");
    assert_eq!(fs::read(saved.join("child")).unwrap(), b"original child");
    assert!(e.retained.contains(&source));
    assert!(f.run().join("item-000000/info").exists());
}
#[test]
fn occupied_wrong_claim_return_preserves_both_objects_and_evidence() {
    let f = Fixture::new();
    let r = f.receipt();
    let source = r.data_path();
    let saved = f.0.join("saved-original");
    let plan = prepare_empty(&[f.location()], &CancelToken::new())
        .unwrap()
        .confirm_irreversible();
    let at = source.clone();
    let occupied = source.clone();
    let saved_at = saved.clone();
    AFTER_MOVE.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            BEFORE_MOVE.with(|next| {
                *next.borrow_mut() = Some(Box::new(move || {
                    fs::rename(&at, &saved_at).unwrap();
                    fs::write(&at, b"wrong claim").unwrap();
                }))
            });
            AFTER_MOVE.with(|next| {
                *next.borrow_mut() = Some(Box::new(move || {
                    fs::write(&occupied, b"new occupied name").unwrap();
                }))
            });
        }))
    });
    let e = plan
        .execute(&mut f.history(), &CancelToken::new(), |_| {})
        .unwrap_err();
    assert_eq!(e.outcome.removed, Removed::default());
    assert_eq!(fs::read(&source).unwrap(), b"new occupied name");
    assert_eq!(
        fs::read(f.run().join("item-000000/data")).unwrap(),
        b"wrong claim"
    );
    assert_eq!(fs::read(saved.join("child")).unwrap(), b"original child");
    assert!(e.retained.contains(&source));
}
#[test]
fn item_namespace_relocation_before_delete_preserves_both_trees() {
    let f = Fixture::new();
    f.receipt();
    let plan = prepare_empty(&[f.location()], &CancelToken::new())
        .unwrap()
        .confirm_irreversible();
    let root = f.0.clone();
    BEFORE_DELETE.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let run = fs::read_dir(root.join("Trash"))
                .unwrap()
                .filter_map(Result::ok)
                .find(|e| e.file_name().as_bytes().starts_with(b".gust-empty-"))
                .unwrap()
                .path();
            fs::rename(run.join("item-000000"), run.join("retained-item")).unwrap();
            fs::create_dir(run.join("item-000000")).unwrap();
            fs::write(run.join("item-000000/data"), b"replacement").unwrap();
        }))
    });
    let e = plan
        .execute(&mut f.history(), &CancelToken::new(), |_| {})
        .unwrap_err();
    assert_eq!(e.outcome.removed, Removed::default());
    assert_eq!(
        fs::read(f.run().join("retained-item/data/child")).unwrap(),
        b"original child"
    );
    assert_eq!(
        fs::read(f.run().join("item-000000/data")).unwrap(),
        b"replacement"
    );
    assert!(f.run().join("retained-item/info").exists());
}
#[test]
fn barrier_persistence_failure_has_no_trash_effect() {
    let f = Fixture::new();
    let r = f.receipt();
    let plan = prepare_empty(&[f.location()], &CancelToken::new())
        .unwrap()
        .confirm_irreversible();
    let mut h = f.history();
    fs::set_permissions(f.0.join("state"), fs::Permissions::from_mode(0o500)).unwrap();
    let result = plan.execute(&mut h, &CancelToken::new(), |_| {});
    fs::set_permissions(f.0.join("state"), fs::Permissions::from_mode(0o700)).unwrap();
    let e = result.unwrap_err();
    assert!(matches!(e.reason, EmptyTrashFailure::Barrier(_)));
    assert!(e.retained.is_empty());
    assert!(r.data_path().exists());
    assert!(r.info_path().exists());
    assert!(!fs::read_dir(f.location().trash.root())
        .unwrap()
        .filter_map(Result::ok)
        .any(|e| e.file_name().as_bytes().starts_with(b".gust-empty-")));
}
#[test]
fn metadata_budget_is_typed_and_never_admits_a_partial_scope() {
    // Actual traversal refuses at its depth bound before any staging.
    let f = Fixture::new();
    let mut path = f.0.join("work/tree");
    for _ in 0..65 {
        path = path.join("d");
        fs::create_dir(&path).unwrap();
    }
    let result = permanent::admit_tree(&f.0.join("work/tree"), &CancelToken::new());
    assert_eq!(result.unwrap_err(), DeleteFailure::Limit);
    assert_eq!(
        admission_failure(DeleteFailure::Limit).reason,
        EmptyTrashFailure::Limit
    );
    assert_eq!(
        fail(io::Error::new(io::ErrorKind::InvalidData, EnumerationLimit)).reason,
        EmptyTrashFailure::Limit
    );
}
#[test]
fn aggregate_metadata_limit_refuses_instead_of_confirming_a_prefix() {
    let f = Fixture::new();
    f.receipt();
    let l = f.location();
    let mut bytes =
        b"[Trash Info]\nPath=/synthetic-unused-origin/file\nDeletionDate=2026-01-01T00:00:00\n#"
            .to_vec();
    bytes.resize(65000, b'x');
    bytes.push(b'\n');
    for n in 0..520 {
        let name = format!("bounded-{n}");
        fs::write(l.trash.files.join(&name), b"x").unwrap();
        fs::write(l.trash.info.join(format!("{name}.trashinfo")), &bytes).unwrap();
    }
    let e = prepare_empty(std::slice::from_ref(&l), &CancelToken::new()).unwrap_err();
    assert_eq!(e.reason, EmptyTrashFailure::Limit);
    assert_eq!(fs::read_dir(&l.trash.files).unwrap().count(), 521);
    assert_eq!(fs::read_dir(&l.trash.info).unwrap().count(), 521);
    assert!(!fs::read_dir(l.trash.root())
        .unwrap()
        .filter_map(Result::ok)
        .any(|e| e.file_name().as_bytes().starts_with(b".gust-empty-")));
}

struct MountProbe;
impl MountProbe {
    fn begin(cross: bool) -> Self {
        permanent::CROSS_MOUNT.with(|flag| flag.set(cross));
        anchor::TRAVERSAL_PROBE.with(|p| p.set(Some((0, 0))));
        Self
    }
    fn counts(&self) -> (usize, usize) {
        anchor::TRAVERSAL_PROBE.with(|p| p.get().unwrap())
    }
}
impl Drop for MountProbe {
    fn drop(&mut self) {
        permanent::CROSS_MOUNT.with(|flag| flag.set(false));
        permanent::CROSS_MOUNT_FD.with(|flag| flag.set(None));
        INFO_READ_PROBE.with(|probe| probe.set(None));
        lineage::READ_PROBE.with(|probe| probe.set(None));
        anchor::TRAVERSAL_PROBE.with(|p| p.set(None));
    }
}
fn mounted_fixture() -> Fixture {
    let f = Fixture::new();
    fs::create_dir(f.0.join("work/tree/mounted")).unwrap();
    fs::write(
        f.0.join("work/tree/mounted/sentinel"),
        b"never inspect mounted contents",
    )
    .unwrap();
    f.receipt();
    f
}
#[test]
fn preparation_rejects_descendant_mount_before_open_or_enumeration() {
    let f = mounted_fixture();
    let probe = MountProbe::begin(true);
    let result = prepare_empty(&[f.location()], &CancelToken::new());
    assert_eq!(
        probe.counts(),
        (0, 0),
        "a rejected mount was traversed during preparation"
    );
    assert_eq!(result.unwrap_err().reason, EmptyTrashFailure::Unsupported);
}
#[test]
fn revalidation_rejects_new_mount_before_open_or_enumeration() {
    let f = mounted_fixture();
    let plan = prepare_empty(&[f.location()], &CancelToken::new())
        .unwrap()
        .confirm_irreversible();
    let probe = MountProbe::begin(true);
    let mut h = f.history();
    let result = plan.execute(&mut h, &CancelToken::new(), |_| {});
    assert_eq!(
        probe.counts(),
        (0, 0),
        "a rejected mount was traversed after confirmation"
    );
    assert_eq!(result.unwrap_err().reason, EmptyTrashFailure::Unsupported);
    assert!(h.entries().is_empty());
    assert_eq!(fs::read_dir(f.location().trash.files).unwrap().count(), 1);
}
#[test]
fn data_preclaim_rejects_new_mount_without_traversal() {
    let f = mounted_fixture();
    let plan = prepare_empty(&[f.location()], &CancelToken::new())
        .unwrap()
        .confirm_irreversible();
    let probe = MountProbe::begin(false);
    AFTER_MOVE.with(|h| {
        *h.borrow_mut() = Some(Box::new(|| {
            permanent::CROSS_MOUNT.with(|flag| flag.set(true));
            anchor::TRAVERSAL_PROBE.with(|p| p.set(Some((0, 0))));
        }))
    });
    let result = plan.execute(&mut f.history(), &CancelToken::new(), |_| {});
    assert_eq!(
        probe.counts(),
        (0, 0),
        "data preclaim traversed a new mount"
    );
    assert!(result.is_err());
    assert_eq!(fs::read_dir(f.location().trash.files).unwrap().count(), 1);
    assert!(f.run().join("item-000000/info").exists());
}
#[test]
fn data_postclaim_rejects_new_mount_without_traversal() {
    let f = mounted_fixture();
    let plan = prepare_empty(&[f.location()], &CancelToken::new())
        .unwrap()
        .confirm_irreversible();
    let probe = MountProbe::begin(false);
    AFTER_MOVE.with(|h| {
        *h.borrow_mut() = Some(Box::new(|| {
            AFTER_MOVE.with(|next| {
                *next.borrow_mut() = Some(Box::new(|| {
                    permanent::CROSS_MOUNT.with(|flag| flag.set(true));
                    anchor::TRAVERSAL_PROBE.with(|p| p.set(Some((0, 0))));
                }))
            });
        }))
    });
    let result = plan.execute(&mut f.history(), &CancelToken::new(), |_| {});
    assert_eq!(
        probe.counts(),
        (0, 0),
        "data postclaim traversed a new mount"
    );
    assert!(result.is_err());
    assert_eq!(fs::read_dir(f.location().trash.files).unwrap().count(), 1);
    assert!(f.run().join("item-000000/info").exists());
}

fn outer_binding_case(move_trash: bool) {
    let f = Fixture::new();
    fs::write(f.0.join("work/tree/second"), b"remaining work").unwrap();
    fs::write(f.0.join("work/tree/third"), b"remaining work").unwrap();
    let r = f.receipt();
    let info_id = FileIdentity::of_path(&r.info_path()).unwrap();
    let info_bytes = fs::read(r.info_path()).unwrap();
    let plan = prepare_empty(&[f.location()], &CancelToken::new())
        .unwrap()
        .confirm_irreversible();
    let mut moved = false;
    let mut finished = false;
    let e = plan
        .execute(&mut f.history(), &CancelToken::new(), |p| {
            finished |= p.phase == EmptyTrashPhase::Finished;
            if !moved && p.outcome.removed.files == 1 {
                let run = f.run();
                if move_trash {
                    let trash = f.location().trash.root().to_path_buf();
                    let old_trash = f.0.join("retained-trash");
                    fs::rename(&trash, &old_trash).unwrap();
                    fs::create_dir(&trash).unwrap();
                    fs::set_permissions(&trash, fs::Permissions::from_mode(0o700)).unwrap();
                    for name in ["files", "info"] {
                        fs::create_dir(trash.join(name)).unwrap();
                        fs::set_permissions(trash.join(name), fs::Permissions::from_mode(0o700))
                            .unwrap();
                    }
                    fs::rename(old_trash.join(run.file_name().unwrap()), &run).unwrap();
                } else {
                    let old_run = f.0.join("retained-run");
                    fs::rename(&run, &old_run).unwrap();
                    fs::create_dir(&run).unwrap();
                    fs::set_permissions(&run, fs::Permissions::from_mode(0o700)).unwrap();
                    fs::rename(old_run.join("item-000000"), run.join("item-000000")).unwrap();
                }
                fs::write(run.join("replacement-evidence"), b"retain replacement").unwrap();
                moved = true;
            }
        })
        .unwrap_err();
    assert_eq!(
        e.outcome.removed,
        Removed { files: 1, dirs: 0 },
        "lost outer authority did not stop the worker"
    );
    assert_eq!(e.outcome.items_completed, 0);
    assert!(!finished);
    assert!(!e.retained.is_empty());
    let run = f.run();
    let pair = run.join("item-000000");
    let stage = fs::read_dir(&pair)
        .unwrap()
        .filter_map(Result::ok)
        .find(|e| e.file_name().as_bytes().starts_with(b".gust-delete-"))
        .unwrap()
        .path();
    assert_eq!(fs::read_dir(stage.join("item")).unwrap().count(), 2);
    assert!(stage.join("intent.json").exists());
    assert!(pair.join("intent.json").exists());
    assert_eq!(FileIdentity::of_path(&pair.join("info")).unwrap(), info_id);
    assert_eq!(fs::read(pair.join("info")).unwrap(), info_bytes);
    assert_eq!(
        fs::read(run.join("replacement-evidence")).unwrap(),
        b"retain replacement"
    );
    if move_trash {
        assert!(f.0.join("retained-trash").exists());
    } else {
        assert!(f.0.join("retained-run/intent.json").exists());
    }
}
#[test]
fn moved_pair_cannot_hide_lost_run_outer_binding() {
    outer_binding_case(false);
}
#[test]
fn moved_run_cannot_hide_lost_trash_outer_binding() {
    outer_binding_case(true);
}

#[test]
fn opened_directory_mount_is_checked_before_enumeration() {
    let f = mounted_fixture();
    let root = f.location().trash.files.join("tree").join("mounted");
    let id = FileIdentity::of_path(&root).unwrap();
    let probe = MountProbe::begin(false);
    permanent::CROSS_MOUNT_FD.with(|flag| flag.set(Some((id.dev, id.ino))));
    let result = prepare_empty(&[f.location()], &CancelToken::new());
    assert_eq!(probe.counts(), (1, 0));
    assert_eq!(result.unwrap_err().reason, EmptyTrashFailure::Unsupported);
}
#[test]
fn opened_info_mount_is_checked_before_reading_bytes() {
    let f = Fixture::new();
    let r = f.receipt();
    let id = FileIdentity::of_path(&r.info_path()).unwrap();
    let _probe = MountProbe::begin(false);
    INFO_READ_PROBE.with(|p| p.set(Some(0)));
    permanent::CROSS_MOUNT_FD.with(|flag| flag.set(Some((id.dev, id.ino))));
    let result = prepare_empty(&[f.location()], &CancelToken::new());
    assert_eq!(INFO_READ_PROBE.with(|p| p.get()), Some(0));
    assert_eq!(result.unwrap_err().reason, EmptyTrashFailure::Unsupported);
}
#[test]
fn opened_lineage_mount_is_checked_before_reading_certificate() {
    let f = Fixture::new();
    let r = f.receipt();
    let data = r.data_path();
    fs::create_dir(data.join("temporary-revision")).unwrap();
    fs::remove_dir(data.join("temporary-revision")).unwrap();
    let tree = history_tree::snapshot(&data, &CancelToken::new()).unwrap();
    assert_ne!(tree.get(&data), Some(&r.data));
    // Protocol fixture: the production inverse is the only ordinary publisher.
    r.lineage_binding()
        .publish(&tree, &CancelToken::new())
        .unwrap();
    let record = fs::read_dir(f.location().trash.info.join(".gust-lineage"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let id = FileIdentity::of_path(&record).unwrap();
    let _probe = MountProbe::begin(false);
    lineage::READ_PROBE.with(|p| p.set(Some(0)));
    permanent::CROSS_MOUNT_FD.with(|flag| flag.set(Some((id.dev, id.ino))));
    let result = prepare_empty(&[f.location()], &CancelToken::new());
    assert_eq!(lineage::READ_PROBE.with(|p| p.get()), Some(0));
    assert_eq!(result.unwrap_err().reason, EmptyTrashFailure::Unsupported);
}

#[test]
fn final_unlink_with_lost_outer_binding_never_reports_finished() {
    let f = Fixture::new();
    let location = f.location();
    location
        .trash_path(&f.0.join("work/tree/child"), &CancelToken::new())
        .unwrap();
    let plan = prepare_empty(&[location], &CancelToken::new())
        .unwrap()
        .confirm_irreversible();
    let mut moved = false;
    let mut finished = false;
    let e = plan
        .execute(&mut f.history(), &CancelToken::new(), |p| {
            finished |= p.phase == EmptyTrashPhase::Finished;
            if !moved && p.outcome.removed.files == 1 {
                let run = f.run();
                let old = f.0.join("retained-run");
                fs::rename(&run, &old).unwrap();
                fs::create_dir(&run).unwrap();
                fs::set_permissions(&run, fs::Permissions::from_mode(0o700)).unwrap();
                fs::rename(old.join("item-000000"), run.join("item-000000")).unwrap();
                moved = true;
            }
        })
        .unwrap_err();
    assert_eq!(e.outcome.removed, Removed { files: 1, dirs: 0 });
    assert_eq!(e.outcome.items_completed, 0);
    assert!(!finished);
    assert!(f.run().join("item-000000/info").exists());
    assert!(f.0.join("retained-run/intent.json").exists());
}
