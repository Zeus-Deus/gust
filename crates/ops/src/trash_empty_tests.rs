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
