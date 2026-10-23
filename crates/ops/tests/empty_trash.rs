//! Only uniquely owned private fixtures are ever admitted by these tests.
use gust_core::lister::CancelToken;
use gust_ops::{
    anchor::FileIdentity,
    history::{History, HistoryError, HistoryKind},
    permanent::{DeleteFailure, Removed},
    trash::{self, EmptyTrashFailure, EmptyTrashPhase, TrashDir, TrashLocation},
};
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "gust-empty-private-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("work")).unwrap();
        Self(root)
    }
    fn location(&self) -> TrashLocation {
        TrashLocation {
            trash: TrashDir::at(self.0.join("Trash")),
            topdir: None,
        }
    }
    fn history(&self) -> History {
        History::open(self.0.join("state/history.json")).unwrap()
    }
    fn file(&self, name: &str) -> PathBuf {
        let path = self.0.join("work").join(name);
        fs::write(&path, b"owned synthetic bytes").unwrap();
        path
    }
    fn tree(&self, count: usize) -> PathBuf {
        let root = self.0.join("work/tree");
        fs::create_dir(&root).unwrap();
        for n in 0..count {
            fs::write(root.join(format!("file-{n:04}")), b"retain on stop").unwrap();
        }
        root
    }
    fn runs(&self) -> Vec<PathBuf> {
        fs::read_dir(self.location().trash.root())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".gust-empty-")
            })
            .map(|entry| entry.path())
            .collect()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn cancel() -> CancelToken {
    CancelToken::new()
}
fn all_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if fs::symlink_metadata(&path).unwrap().is_dir() {
            out.extend(all_files(&path));
        } else {
            out.push(path);
        }
    }
    out
}
#[test]
fn preparation_and_cancelled_confirmation_are_read_only() {
    let f = Fixture::new();
    let l = f.location();
    let r = l.trash_path(&f.file("one"), &cancel()).unwrap();
    let plan = trash::prepare_empty(std::slice::from_ref(&l), &cancel()).unwrap();
    assert_eq!(plan.summary().items, 1);
    assert_eq!(plan.summary().data_entries, 1);
    assert!(plan.retained_bytes() >= r.retained_bytes());
    assert!(f.runs().is_empty());
    assert!(r.item.info_file.exists());
    drop(plan); // No execution authority or filesystem effect from drop.
    let token = cancel();
    token.cancel();
    let e = trash::prepare_empty(std::slice::from_ref(&l), &token).unwrap_err();
    assert_eq!(e.reason, EmptyTrashFailure::Cancelled);
    let plan = trash::prepare_empty(std::slice::from_ref(&l), &cancel()).unwrap();
    let mut h = f.history();
    let e = plan
        .confirm_irreversible()
        .execute(&mut h, &token, |_| panic!("no progress before admission"))
        .unwrap_err();
    assert_eq!(e.reason, EmptyTrashFailure::Cancelled);
    assert!(h.entries().is_empty());
    assert!(f.runs().is_empty());
    assert!(l.trash.files.join(&r.item.trashed_name).exists());
}
#[test]
fn confirmed_execution_is_irreversible_and_retains_exact_information() {
    let f = Fixture::new();
    let l = f.location();
    let original = f.file("one");
    let r = l.trash_path(&original, &cancel()).unwrap();
    let info_id = FileIdentity::of_path(&r.item.info_file).unwrap();
    let info_bytes = fs::read(&r.item.info_file).unwrap();
    fs::write(&original, b"new original-path work").unwrap();
    let plan = trash::prepare_empty(std::slice::from_ref(&l), &cancel()).unwrap();
    let mut h = f.history();
    let mut phases = Vec::new();
    let outcome = plan
        .confirm_irreversible()
        .execute(&mut h, &cancel(), |p| phases.push(p))
        .unwrap();
    assert_eq!(outcome.items_completed, 1);
    assert_eq!(outcome.removed, Removed { files: 1, dirs: 0 });
    assert_eq!(outcome.information_retained, 1);
    assert_eq!(phases.last().unwrap().phase, EmptyTrashPhase::Finished);
    assert_eq!(phases.last().unwrap().outcome, outcome);
    assert_eq!(h.entries().last().unwrap().kind, HistoryKind::Barrier);
    assert!(matches!(h.undo(&cancel()), Err(HistoryError::Barrier(_))));
    drop(h);
    assert!(matches!(
        f.history().undo(&cancel()),
        Err(HistoryError::Barrier(_))
    ));
    assert_eq!(fs::read(&original).unwrap(), b"new original-path work");
    assert!(!r.item.info_file.exists());
    assert!(!l.trash.files.join(&r.item.trashed_name).exists());
    let run = &f.runs()[0];
    let retained = run.join("item-000000/info");
    assert_eq!(FileIdentity::of_path(&retained).unwrap(), info_id);
    assert_eq!(fs::read(&retained).unwrap(), info_bytes);
    assert!(run.join("intent.json").exists());
    assert!(run.join("item-000000/intent.json").exists());
    for path in [run.as_path(), &run.join("item-000000")] {
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}
#[test]
fn full_2005_member_tree_is_deleted_without_a_preview_cap() {
    let f = Fixture::new();
    let l = f.location();
    let r = l.trash_path(&f.tree(2005), &cancel()).unwrap();
    let plan = trash::prepare_empty(std::slice::from_ref(&l), &cancel()).unwrap();
    assert_eq!(plan.summary().data_entries, 2006);
    let outcome = plan
        .confirm_irreversible()
        .execute(&mut f.history(), &cancel(), |_| {})
        .unwrap();
    assert_eq!(
        outcome.removed,
        Removed {
            files: 2005,
            dirs: 1
        }
    );
    assert_eq!(outcome.items_completed, 1);
    assert!(!l.trash.files.join(&r.item.trashed_name).exists());
    assert!(all_files(l.trash.root()).iter().all(|p| p
        .file_name()
        .is_some_and(|n| n == "intent.json" || n == "info")));
}
#[test]
fn authoritative_pair_enumeration_admits_all_2005_slots() {
    let f = Fixture::new();
    let l = f.location();
    l.trash_path(&f.file("seed"), &cancel()).unwrap();
    for n in 1..2005 {
        let name = format!("synthetic-{n:04}");
        fs::write(l.trash.files.join(&name), b"x").unwrap();
        fs::write(l.trash.info.join(format!("{name}.trashinfo")), b"[Trash Info]\nPath=/synthetic-unopened-origin/file\nDeletionDate=2026-01-01T00:00:00\n").unwrap();
    }
    let plan = trash::prepare_empty(&[l], &cancel()).unwrap();
    assert_eq!(plan.summary().items, 2005);
    assert_eq!(plan.summary().data_entries, 2005);
    assert_eq!(plan.summary().retained_entries, 0);
    assert!(f.runs().is_empty());
}
#[test]
fn pending_orphan_and_history_evidence_is_retained() {
    let f = Fixture::new();
    let l = f.location();
    l.trash_path(&f.file("ordinary"), &cancel()).unwrap();
    let tree = f.tree(1);
    fs::create_dir(tree.join(".gust-history-owned")).unwrap();
    fs::write(tree.join(".gust-history-owned/bytes"), b"recovery").unwrap();
    let r = l.trash_path(&tree, &cancel()).unwrap();
    fs::write(l.trash.files.join("unpaired"), b"no information").unwrap();
    fs::write(
        l.trash.info.join("orphan.trashinfo"),
        b"unfinished metadata",
    )
    .unwrap();
    fs::create_dir(l.trash.files.join(".gust-pending")).unwrap();
    fs::write(
        l.trash.info.join(".gust-pending.trashinfo"),
        b"not a record to discard",
    )
    .unwrap();
    let plan = trash::prepare_empty(std::slice::from_ref(&l), &cancel()).unwrap();
    assert_eq!(plan.summary().items, 1);
    assert_eq!(plan.summary().retained_entries, 6);
    let outcome = plan
        .confirm_irreversible()
        .execute(&mut f.history(), &cancel(), |_| {})
        .unwrap();
    assert_eq!(outcome.retained_entries, 6);
    assert_eq!(
        fs::read(
            l.trash
                .files
                .join(&r.item.trashed_name)
                .join(".gust-history-owned/bytes")
        )
        .unwrap(),
        b"recovery"
    );
    assert!(r.item.info_file.exists());
    assert!(l.trash.files.join("unpaired").exists());
    assert!(l.trash.info.join("orphan.trashinfo").exists());
}
#[test]
fn deep_edit_after_confirmation_is_never_adopted() {
    let f = Fixture::new();
    let l = f.location();
    let r = l.trash_path(&f.tree(2), &cancel()).unwrap();
    let plan = trash::prepare_empty(std::slice::from_ref(&l), &cancel())
        .unwrap()
        .confirm_irreversible();
    fs::write(
        l.trash.files.join(&r.item.trashed_name).join("file-0001"),
        b"later work",
    )
    .unwrap();
    let mut h = f.history();
    let e = plan.execute(&mut h, &cancel(), |_| {}).unwrap_err();
    assert_eq!(e.reason, EmptyTrashFailure::Changed);
    assert_eq!(e.outcome.removed, Removed::default());
    assert!(f.runs().is_empty());
    assert!(h.entries().is_empty());
    assert_eq!(
        fs::read(l.trash.files.join(&r.item.trashed_name).join("file-0001")).unwrap(),
        b"later work"
    );
}
#[test]
fn new_slot_after_confirmation_refuses_before_effects() {
    let f = Fixture::new();
    let l = f.location();
    l.trash_path(&f.file("one"), &cancel()).unwrap();
    let plan = trash::prepare_empty(std::slice::from_ref(&l), &cancel())
        .unwrap()
        .confirm_irreversible();
    fs::write(l.trash.files.join("new"), b"new work").unwrap();
    let mut h = f.history();
    assert_eq!(
        plan.execute(&mut h, &cancel(), |_| {}).unwrap_err().reason,
        EmptyTrashFailure::Changed
    );
    assert!(f.runs().is_empty());
    assert!(h.entries().is_empty());
}
#[test]
fn cancellation_after_seventeen_removals_reports_partial_and_preserves_remainder() {
    let f = Fixture::new();
    let l = f.location();
    l.trash_path(&f.tree(50), &cancel()).unwrap();
    let plan = trash::prepare_empty(&[l], &cancel())
        .unwrap()
        .confirm_irreversible();
    let token = cancel();
    let stop = token.clone();
    let mut finished = false;
    let e = plan
        .execute(&mut f.history(), &token, |p| {
            finished |= p.phase == EmptyTrashPhase::Finished;
            if p.outcome.removed.files == 17 {
                stop.cancel();
            }
        })
        .unwrap_err();
    assert_eq!(
        e.reason,
        EmptyTrashFailure::Delete(DeleteFailure::Cancelled)
    );
    assert_eq!(e.outcome.removed, Removed { files: 17, dirs: 0 });
    assert_eq!(e.outcome.items_completed, 0);
    assert_eq!(e.outcome.information_retained, 1);
    assert!(!e.retained.is_empty());
    assert!(!finished);
    let remaining = all_files(f.location().trash.root())
        .into_iter()
        .filter(|p| fs::read(p).unwrap() == b"retain on stop")
        .count();
    assert_eq!(remaining, 33);
    assert!(matches!(
        f.history().undo(&cancel()),
        Err(HistoryError::Barrier(_))
    ));
}
#[test]
fn cancel_after_private_claim_retains_zero_count_data_and_information() {
    let f = Fixture::new();
    let l = f.location();
    l.trash_path(&f.file("one"), &cancel()).unwrap();
    let plan = trash::prepare_empty(&[l], &cancel())
        .unwrap()
        .confirm_irreversible();
    let token = cancel();
    let stop = token.clone();
    let e = plan
        .execute(&mut f.history(), &token, |_| stop.cancel())
        .unwrap_err();
    assert_eq!(e.outcome.removed, Removed::default());
    assert_eq!(e.outcome.information_retained, 1);
    assert_eq!(e.outcome.items_completed, 0);
    assert!(!e.retained.is_empty());
    assert_eq!(
        fs::read(f.runs()[0].join("item-000000/data")).unwrap(),
        b"owned synthetic bytes"
    );
    assert!(f.runs()[0].join("item-000000/info").exists());
}
#[test]
fn discard_never_opens_original_restore_parent() {
    let f = Fixture::new();
    let l = f.location();
    let original = f.file("one");
    let r = l.trash_path(&original, &cancel()).unwrap();
    fs::remove_dir(original.parent().unwrap()).unwrap();
    symlink("work", f.0.join("work")).unwrap(); // ELOOP if discard tries to open it.
    assert!(l.receipt_named(&r.item.trashed_name, &cancel()).is_err());
    let plan = trash::prepare_empty(&[l], &cancel()).unwrap();
    assert_eq!(
        plan.confirm_irreversible()
            .execute(&mut f.history(), &cancel(), |_| {})
            .unwrap()
            .removed
            .files,
        1
    );
}
#[test]
fn duplicate_locations_and_invalid_paired_information_refuse_without_claims() {
    let f = Fixture::new();
    let l = f.location();
    let r = l.trash_path(&f.file("one"), &cancel()).unwrap();
    assert_eq!(
        trash::prepare_empty(&[l.clone(), l.clone()], &cancel())
            .unwrap_err()
            .reason,
        EmptyTrashFailure::Changed
    );
    fs::write(&r.item.info_file, b"invalid").unwrap();
    assert!(trash::prepare_empty(&[l], &cancel()).is_err());
    assert!(f.runs().is_empty());
}
#[test]
fn relocation_after_first_unlink_preserves_remaining_data_and_refuses_finished() {
    let f = Fixture::new();
    let l = f.location();
    l.trash_path(&f.tree(3), &cancel()).unwrap();
    let plan = trash::prepare_empty(std::slice::from_ref(&l), &cancel())
        .unwrap()
        .confirm_irreversible();
    let mut moved = false;
    let mut finished = false;
    let e = plan
        .execute(&mut f.history(), &cancel(), |p| {
            finished |= p.phase == EmptyTrashPhase::Finished;
            if !moved && p.outcome.removed.files == 1 {
                let run = f.runs().pop().unwrap();
                fs::rename(&run, l.trash.root().join("retained-run")).unwrap();
                moved = true;
            }
        })
        .unwrap_err();
    assert_eq!(e.outcome.removed, Removed { files: 1, dirs: 0 });
    assert_eq!(e.outcome.items_completed, 0);
    assert!(!finished);
    let retained = l.trash.root().join("retained-run");
    assert_eq!(
        all_files(&retained)
            .into_iter()
            .filter(|p| fs::read(p).unwrap() == b"retain on stop")
            .count(),
        2
    );
    assert!(retained.join("item-000000/info").exists());
}
#[test]
fn symlink_data_never_deletes_its_target() {
    let f = Fixture::new();
    let l = f.location();
    let target = f.file("keep");
    let link = f.0.join("work/link");
    symlink(&target, &link).unwrap();
    l.trash_path(&link, &cancel()).unwrap();
    let plan = trash::prepare_empty(&[l], &cancel()).unwrap();
    assert_eq!(
        plan.confirm_irreversible()
            .execute(&mut f.history(), &cancel(), |_| {})
            .unwrap()
            .removed
            .files,
        1
    );
    assert_eq!(fs::read(target).unwrap(), b"owned synthetic bytes");
}
#[test]
fn evidence_only_plan_has_no_mutation_or_barrier() {
    let f = Fixture::new();
    let l = f.location();
    l.trash_path(&f.file(".gust-retained"), &cancel()).unwrap();
    let plan = trash::prepare_empty(&[l], &cancel()).unwrap();
    assert_eq!(plan.summary().items, 0);
    assert_eq!(plan.summary().retained_entries, 2);
    let mut h = f.history();
    let out = plan
        .confirm_irreversible()
        .execute(&mut h, &cancel(), |_| {})
        .unwrap();
    assert_eq!(out.retained_entries, 2);
    assert!(h.entries().is_empty());
    assert!(f.runs().is_empty());
}
#[test]
fn root_recovery_objects_are_counted_and_never_swept() {
    let f = Fixture::new();
    let l = f.location();
    l.trash_path(&f.file("one"), &cancel()).unwrap();
    let evidence = l.trash.root().join(".gust-prior-pending");
    fs::create_dir(&evidence).unwrap();
    fs::write(evidence.join("intent.json"), b"retained previous intent").unwrap();
    let plan = trash::prepare_empty(std::slice::from_ref(&l), &cancel()).unwrap();
    assert_eq!(plan.summary().retained_entries, 1);
    let outcome = plan
        .confirm_irreversible()
        .execute(&mut f.history(), &cancel(), |_| {})
        .unwrap();
    assert_eq!(outcome.retained_entries, 1);
    assert_eq!(
        fs::read(evidence.join("intent.json")).unwrap(),
        b"retained previous intent"
    );
}
#[test]
fn new_root_recovery_object_after_confirmation_refuses_before_effects() {
    let f = Fixture::new();
    let l = f.location();
    l.trash_path(&f.file("one"), &cancel()).unwrap();
    let plan = trash::prepare_empty(std::slice::from_ref(&l), &cancel())
        .unwrap()
        .confirm_irreversible();
    fs::write(l.trash.root().join("new-evidence"), b"retain new work").unwrap();
    let mut h = f.history();
    assert_eq!(
        plan.execute(&mut h, &cancel(), |_| {}).unwrap_err().reason,
        EmptyTrashFailure::Changed
    );
    assert!(h.entries().is_empty());
    assert!(f.runs().is_empty());
}
#[test]
fn multiple_explicit_locations_keep_separate_claims_and_one_barrier() {
    let f = Fixture::new();
    let first = f.location();
    let second = TrashLocation {
        trash: TrashDir::at(f.0.join("OtherTrash")),
        topdir: None,
    };
    first.trash_path(&f.file("one"), &cancel()).unwrap();
    second.trash_path(&f.file("two"), &cancel()).unwrap();
    let plan = trash::prepare_empty(&[first.clone(), second.clone()], &cancel()).unwrap();
    assert_eq!(plan.summary().locations, 2);
    assert_eq!(plan.summary().items, 2);
    let mut h = f.history();
    let outcome = plan
        .confirm_irreversible()
        .execute(&mut h, &cancel(), |_| {})
        .unwrap();
    assert_eq!(outcome.items_completed, 2);
    assert_eq!(outcome.information_retained, 2);
    assert_eq!(h.entries().len(), 1);
    assert_eq!(fs::read_dir(first.trash.files).unwrap().count(), 0);
    assert_eq!(fs::read_dir(second.trash.files).unwrap().count(), 0);
}
