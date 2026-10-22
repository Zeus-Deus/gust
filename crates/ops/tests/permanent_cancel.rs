use gust_core::lister::CancelToken;
use gust_ops::{
    anchor::FileIdentity,
    history::{History, HistoryError},
    permanent::{self, DeleteFailure, DeletePhase, Removed},
};
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::PathBuf,
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "gust-permanent-owned-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn stage(&self) -> PathBuf {
        fs::read_dir(&self.0)
            .unwrap()
            .filter_map(Result::ok)
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".gust-delete-")
            })
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
fn recursive_cancellation_reports_exact_partial_counts_and_retains_remaining_data() {
    let f = Fixture::new();
    let root = f.0.join("tree");
    fs::create_dir(&root).unwrap();
    for i in 0..2005 {
        fs::write(root.join(format!("file-{i:04}")), b"private fixture").unwrap();
    }
    let expected = FileIdentity::of_path(&root).unwrap().into();
    let cancel = CancelToken::new();
    let stop = cancel.clone();
    let mut totals = Vec::new();
    let error = permanent::delete_permanently_with_progress(
        &f.0,
        &root,
        Some(expected),
        &cancel,
        |progress| {
            if progress.phase == DeletePhase::Deleting {
                totals.push(progress.total);
            }
            if progress.removed.files == 17 {
                stop.cancel();
            }
        },
    )
    .unwrap_err();
    assert_eq!(error.reason, DeleteFailure::Cancelled);
    assert_eq!(error.removed, Removed { files: 17, dirs: 0 });
    assert!(totals.iter().all(|total| *total == 2006));
    let stage = f.stage();
    assert!(error.retained.contains(&stage));
    assert!(stage.join("intent.json").exists());
    assert_eq!(fs::read_dir(stage.join("item")).unwrap().count(), 1988);
    for entry in fs::read_dir(stage.join("item")).unwrap() {
        assert_eq!(fs::read(entry.unwrap().path()).unwrap(), b"private fixture");
    }
    assert!(!root.exists());
}
#[test]
fn cancellation_during_complete_admission_has_no_stage_or_deletions() {
    let f = Fixture::new();
    let root = f.0.join("tree");
    fs::create_dir(&root).unwrap();
    for i in 0..20 {
        fs::write(root.join(format!("file-{i}")), b"keep").unwrap();
    }
    let cancel = CancelToken::new();
    let stop = cancel.clone();
    let error = permanent::delete_permanently_with_progress(&f.0, &root, None, &cancel, |p| {
        if p.phase == DeletePhase::Checking && p.total == 7 {
            stop.cancel();
        }
    })
    .unwrap_err();
    assert_eq!(error.reason, DeleteFailure::Cancelled);
    assert_eq!(error.removed, Removed::default());
    assert!(error.retained.is_empty());
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 1);
    assert_eq!(fs::read_dir(root).unwrap().count(), 20);
}
#[test]
fn successful_delete_never_follows_links_and_keeps_the_irreversible_barrier() {
    let f = Fixture::new();
    let outside = f.0.join("outside");
    fs::write(&outside, b"keep").unwrap();
    let root = f.0.join("tree");
    fs::create_dir_all(root.join("nested")).unwrap();
    fs::write(root.join("nested/file"), b"remove").unwrap();
    symlink(&outside, root.join("link")).unwrap();
    let mut history = History::open(f.0.join("state/history.json")).unwrap();
    history
        .record_barrier("Confirmed irreversible deletion")
        .unwrap();
    let mut progress = Vec::new();
    let removed = permanent::delete_permanently_with_progress(
        &f.0,
        &root,
        Some(FileIdentity::of_path(&root).unwrap().into()),
        &CancelToken::new(),
        |p| progress.push(p),
    )
    .unwrap();
    assert_eq!(removed, Removed { files: 2, dirs: 2 });
    assert_eq!(fs::read(outside).unwrap(), b"keep");
    assert!(!root.exists());
    assert_eq!(progress.last().unwrap().phase, DeletePhase::Finished);
    assert_eq!(progress.last().unwrap().removed, removed);
    assert!(matches!(
        history.undo(&CancelToken::new()),
        Err(HistoryError::Barrier(_))
    ));
    assert!(f.stage().join("intent.json").exists());
    assert!(!f.stage().join("item").exists());
}
#[test]
fn old_confirmation_identity_never_adopts_a_replacement() {
    let f = Fixture::new();
    let root = f.0.join("file");
    fs::write(&root, b"original").unwrap();
    let expected = FileIdentity::of_path(&root).unwrap().into();
    fs::rename(&root, f.0.join("kept")).unwrap();
    fs::write(&root, b"new work").unwrap();
    let error = permanent::delete_permanently_with_progress(
        &f.0,
        &root,
        Some(expected),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap_err();
    assert_eq!(error.reason, DeleteFailure::Changed);
    assert_eq!(error.removed, Removed::default());
    assert!(error.retained.is_empty());
    assert_eq!(fs::read(root).unwrap(), b"new work");
    assert_eq!(fs::read(f.0.join("kept")).unwrap(), b"original");
}
#[test]
fn changed_parent_permissions_return_partial_counts_and_preserve_the_private_tree() {
    let f = Fixture::new();
    let root = f.0.join("tree");
    fs::create_dir(&root).unwrap();
    for i in 0..3 {
        fs::write(root.join(format!("file-{i}")), b"private fixture").unwrap();
    }
    let error =
        permanent::delete_permanently_with_progress(&f.0, &root, None, &CancelToken::new(), |p| {
            if p.removed.files == 1 {
                fs::set_permissions(f.stage().join("item"), fs::Permissions::from_mode(0o500))
                    .unwrap();
            }
        })
        .unwrap_err();
    assert_eq!(error.reason, DeleteFailure::Changed);
    assert_eq!(error.removed, Removed { files: 1, dirs: 0 });
    let partial = f.stage().join("item");
    assert_eq!(fs::read_dir(&partial).unwrap().count(), 2);
    fs::set_permissions(partial, fs::Permissions::from_mode(0o700)).unwrap();
}
#[test]
fn refused_scope_and_precancel_never_create_storage() {
    let f = Fixture::new();
    let file = f.0.join("file");
    fs::write(&file, b"keep").unwrap();
    let error =
        permanent::delete_permanently_with_cancel(&f.0, &f.0, &CancelToken::new()).unwrap_err();
    assert!(matches!(error.reason, DeleteFailure::Refused(_)));
    assert_eq!(error.removed, Removed::default());
    let cancel = CancelToken::new();
    cancel.cancel();
    let error = permanent::delete_permanently_with_cancel(&f.0, &file, &cancel).unwrap_err();
    assert_eq!(error.reason, DeleteFailure::Cancelled);
    assert!(error.retained.is_empty());
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 1);
}

#[test]
fn late_stage_relocation_stops_before_another_unlink_or_finished() {
    late_relocation_case(false);
}
#[test]
fn late_root_relocation_stops_before_another_unlink_or_finished() {
    late_relocation_case(true);
}
fn late_relocation_case(relocate_root: bool) {
    {
        let f = Fixture::new();
        let root = f.0.join("tree");
        fs::create_dir(&root).unwrap();
        for i in 0..3 {
            fs::write(root.join(format!("file-{i}")), b"keep remainder").unwrap();
        }
        let kept = f.0.join("relocated");
        let mut replacement = None;
        let mut finished = false;
        let result = permanent::delete_permanently_with_progress(
            &f.0,
            &root,
            None,
            &CancelToken::new(),
            |p| {
                finished |= p.phase == DeletePhase::Finished;
                if p.removed.files == 1 && replacement.is_none() {
                    let old = if relocate_root {
                        f.stage().join("item")
                    } else {
                        f.stage()
                    };
                    fs::rename(&old, &kept).unwrap();
                    fs::create_dir(&old).unwrap();
                    fs::write(old.join("later"), b"replacement work").unwrap();
                    replacement = Some(old);
                }
            },
        );
        let remaining = if relocate_root {
            kept.clone()
        } else {
            kept.join("item")
        };
        assert_eq!(
            fs::read_dir(&remaining)
                .map(|entries| entries.count())
                .unwrap_or(0),
            2,
            "persistent relocation must stop before the next removal: {result:?}"
        );
        assert!(!finished, "relocated namespace must not emit Finished");
        let error = result.unwrap_err();
        assert_eq!(error.reason, DeleteFailure::Changed);
        assert_eq!(error.removed, Removed { files: 1, dirs: 0 });
        assert!(error.locations_uncertain);
        assert_eq!(
            fs::read(replacement.unwrap().join("later")).unwrap(),
            b"replacement work"
        );
        let intent = if relocate_root {
            f.stage().join("intent.json")
        } else {
            kept.join("intent.json")
        };
        assert!(intent.is_file());
        for entry in fs::read_dir(remaining).unwrap() {
            assert_eq!(fs::read(entry.unwrap().path()).unwrap(), b"keep remainder");
        }
    }
}

#[test]
fn late_stage_relocation_at_last_unlink_never_reports_finished() {
    let f = Fixture::new();
    let path = f.0.join("file");
    fs::write(&path, b"confirmed removal").unwrap();
    let mut old = None;
    let mut finished = false;
    let kept = f.0.join("kept-stage");
    let result =
        permanent::delete_permanently_with_progress(&f.0, &path, None, &CancelToken::new(), |p| {
            finished |= p.phase == DeletePhase::Finished;
            if p.removed.files == 1 && old.is_none() {
                let stage = f.stage();
                fs::rename(&stage, &kept).unwrap();
                fs::create_dir(&stage).unwrap();
                fs::write(stage.join("later"), b"new work").unwrap();
                old = Some(stage);
            }
        });
    let error = result.unwrap_err();
    assert_eq!(error.reason, DeleteFailure::Changed);
    assert_eq!(error.removed, Removed { files: 1, dirs: 0 });
    assert!(error.locations_uncertain);
    assert!(!finished);
    assert!(kept.join("intent.json").exists());
    assert_eq!(fs::read(old.unwrap().join("later")).unwrap(), b"new work");
}
