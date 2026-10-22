use gust_core::lister::CancelToken;
use gust_ops::{
    attrs, create,
    history::{History, HistoryError, HistoryStatus},
    journal::{Job, JobKind, JobState, JournalEntry},
};
use std::{
    ffi::{OsStr, OsString},
    os::unix::{ffi::OsStringExt, fs::PermissionsExt},
    path::{Path, PathBuf},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "gust-history-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(p.join("work")).unwrap();
        Self(p)
    }
    fn store(&self) -> PathBuf {
        self.0.join("state/history.json")
    }
    fn open(&self) -> History {
        History::open(self.store()).unwrap()
    }
    fn create(&self, name: &OsStr) -> create::Created {
        create::create_from_bytes(&self.0.join("work"), name, b"original").unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn record_tree(path: &Path) -> Job {
    let mut job = Job::new("tree", JobKind::Copy);
    job.state = JobState::Confirmed;
    job.subtree_identities = gust_ops::undo::try_snapshot_subtree(path, &CancelToken::new())
        .unwrap()
        .into_iter()
        .collect();
    job.entries.push(JournalEntry {
        source: PathBuf::from("/synthetic/source"),
        destination: Some(path.to_path_buf()),
        source_identity: None,
        preserved: None,
        done: true,
        error: None,
    });
    job
}
#[test]
fn undo_redo_preserves_exact_objects_across_restart_and_non_utf8_paths() {
    let f = Fixture::new();
    let name = OsString::from_vec(b"raw-\xff".to_vec());
    let c = f.create(&name);
    let mut h = f.open();
    let entry = h
        .record_created("Created file", std::slice::from_ref(&c))
        .unwrap();
    assert!(entry.created_unix > 0);
    assert_eq!(
        History::open(f.store()).err().unwrap().kind(),
        std::io::ErrorKind::WouldBlock
    );
    h.undo(&CancelToken::new()).unwrap();
    assert!(!c.path.exists());
    drop(h);
    let mut h = f.open();
    assert_eq!(h.entries()[0].label, "Created file");
    assert_eq!(h.entries()[0].status, HistoryStatus::Undone);
    h.redo(&CancelToken::new()).unwrap();
    assert_eq!(std::fs::read(&c.path).unwrap(), b"original");
    assert_eq!(attrs::ident_of(&c.path).unwrap(), c.ident);
    h.undo(&CancelToken::new()).unwrap();
    h.redo(&CancelToken::new()).unwrap();
    assert_eq!(attrs::ident_of(&c.path).unwrap(), c.ident);
}
#[test]
fn multiple_steps_restore_directory_lineage_before_older_inverse() {
    let f = Fixture::new();
    let root = f.0.join("work/tree");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("before"), b"data").unwrap();
    let job = record_tree(&root);
    let mut h = f.open();
    h.record_job("Copied tree", &job).unwrap();
    let rename = attrs::rename_recorded(&root.join("before"), OsStr::new("after")).unwrap();
    h.record_renames("Renamed child", &[rename]).unwrap();
    let result = h.undo_many(2, &CancelToken::new());
    assert!(result.refusal.is_none(), "{result:?}");
    assert_eq!(result.completed.len(), 2);
    assert!(!root.exists());
    let result = h.redo_many(2, &CancelToken::new());
    assert!(result.refusal.is_none(), "{result:?}");
    assert_eq!(std::fs::read(root.join("after")).unwrap(), b"data");
    assert_eq!(h.undo_many(2, &CancelToken::new()).completed.len(), 2);
}
#[test]
fn new_work_and_deep_edits_refuse_without_consuming_history() {
    let f = Fixture::new();
    let root = f.0.join("work/tree");
    std::fs::create_dir_all(root.join("deep")).unwrap();
    std::fs::write(root.join("deep/file"), b"original").unwrap();
    let mut h = f.open();
    h.record_job("Copied tree", &record_tree(&root)).unwrap();
    std::fs::write(root.join("deep/new-work"), b"keep").unwrap();
    assert!(matches!(
        h.undo(&CancelToken::new()),
        Err(HistoryError::NewWork(_))
    ));
    assert_eq!(h.entries()[0].status, HistoryStatus::Applied);
    assert_eq!(std::fs::read(root.join("deep/new-work")).unwrap(), b"keep");
    let f2 = Fixture::new();
    let root = f2.0.join("work/tree");
    std::fs::create_dir_all(root.join("deep")).unwrap();
    std::fs::write(root.join("deep/file"), b"original").unwrap();
    let mut h = f2.open();
    h.record_job("Copied tree", &record_tree(&root)).unwrap();
    std::fs::write(root.join("deep/file"), b"edited").unwrap();
    assert!(matches!(
        h.undo(&CancelToken::new()),
        Err(HistoryError::Changed(_))
    ));
    assert_eq!(std::fs::read(root.join("deep/file")).unwrap(), b"edited");
}
#[test]
fn redo_refuses_dangling_destination_then_remains_retryable() {
    let f = Fixture::new();
    let c = f.create(OsStr::new("file"));
    let mut h = f.open();
    h.record_created("Created file", std::slice::from_ref(&c))
        .unwrap();
    h.undo(&CancelToken::new()).unwrap();
    std::os::unix::fs::symlink("absent", &c.path).unwrap();
    assert_eq!(
        h.redo(&CancelToken::new()),
        Err(HistoryError::Occupied(c.path.clone()))
    );
    assert_eq!(h.entries()[0].status, HistoryStatus::Undone);
    std::fs::remove_file(&c.path).unwrap();
    h.redo(&CancelToken::new()).unwrap();
    assert_eq!(attrs::ident_of(&c.path).unwrap(), c.ident);
}
#[test]
fn new_action_retires_redo_without_destroying_preserved_bytes() {
    let f = Fixture::new();
    let old = f.create(OsStr::new("old"));
    let mut h = f.open();
    h.record_created("Old", &[old]).unwrap();
    h.undo(&CancelToken::new()).unwrap();
    let new = f.create(OsStr::new("new"));
    h.record_created("New", &[new]).unwrap();
    assert_eq!(h.redo(&CancelToken::new()), Err(HistoryError::Empty));
    assert!(h
        .entries()
        .iter()
        .any(|e| e.status == HistoryStatus::Retired));
    let vaults: Vec<_> = std::fs::read_dir(f.0.join("work"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".gust-undo-"))
        .collect();
    assert_eq!(vaults.len(), 1);
    assert_eq!(
        std::fs::read(vaults[0].path().join("item")).unwrap(),
        b"original"
    );
}
#[test]
fn rename_receipts_never_adopt_later_descendants() {
    let f = Fixture::new();
    let root = f.0.join("work/before");
    std::fs::create_dir(&root).unwrap();
    let rename = attrs::rename_recorded(&root, OsStr::new("after")).unwrap();
    std::fs::write(rename.renamed.join("new-work"), b"keep").unwrap();
    assert!(matches!(
        f.open().record_renames("Rename", &[rename]),
        Err(HistoryError::NewWork(_))
    ));
}
#[test]
fn permissions_tags_and_links_have_typed_reversible_receipts() {
    let f = Fixture::new();
    let c = f.create(OsStr::new("file"));
    std::fs::set_permissions(&c.path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let changes = attrs::set_modes(std::slice::from_ref(&c.path), attrs::ModeSpec::Exact(0o640));
    let mut h = f.open();
    h.record_modes("Mode", &changes.changed).unwrap();
    h.undo(&CancelToken::new()).unwrap();
    assert_eq!(
        std::fs::metadata(&c.path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    h.redo(&CancelToken::new()).unwrap();
    assert_eq!(
        std::fs::metadata(&c.path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let changes = gust_ops::tags::set_tags(
        std::slice::from_ref(&c.path),
        gust_ops::tags::TagOp::Toggle(1),
    );
    assert!(changes.failed.is_empty(), "{:?}", changes.failed);
    h.record_tags("Tag", &changes.changed).unwrap();
    h.undo(&CancelToken::new()).unwrap();
    assert_eq!(gust_core::tags::read_raw(&c.path).unwrap(), None);
    h.redo(&CancelToken::new()).unwrap();
    assert!(gust_core::tags::read_raw(&c.path).unwrap().is_some());
    let link = attrs::create_link(&c.path, &f.0.join("work"), OsStr::new("link")).unwrap();
    h.record_links("Link", std::slice::from_ref(&link)).unwrap();
    h.undo(&CancelToken::new()).unwrap();
    assert!(!link.link.exists());
    h.redo(&CancelToken::new()).unwrap();
    assert_eq!(std::fs::read_link(&link.link).unwrap(), c.path);
}
#[test]
fn nested_permission_batch_undo_reopen_redo_keeps_verified_descendant_modes() {
    for child_first in [false, true] {
        for directory in [false, true] {
            let f = Fixture::new();
            let parent = f.0.join("work/tree");
            std::fs::create_dir(&parent).unwrap();
            let child = parent.join("child");
            if directory {
                std::fs::create_dir(&child).unwrap();
                std::fs::write(child.join("file"), b"keep").unwrap();
            } else {
                std::fs::write(&child, b"keep").unwrap();
            }
            for path in [&parent, &child] {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let paths = if child_first {
                vec![child.clone(), parent.clone()]
            } else {
                vec![parent.clone(), child.clone()]
            };
            let changed = attrs::set_modes(&paths, attrs::ModeSpec::Exact(0o700));
            assert!(changed.failed.is_empty());
            assert_eq!(changed.changed.len(), 2);
            let mut history = f.open();
            history
                .record_modes("Nested modes", &changed.changed)
                .unwrap();
            history.undo(&CancelToken::new()).unwrap();
            for path in [&parent, &child] {
                assert_eq!(
                    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777,
                    0o755
                );
            }
            drop(history);
            let mut history = f.open();
            history
                .redo(&CancelToken::new())
                .expect("verified nested mode batch must redo");
            for path in [&parent, &child] {
                assert_eq!(
                    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777,
                    0o700
                );
            }
            history.undo(&CancelToken::new()).unwrap();
            history.redo(&CancelToken::new()).unwrap();
        }
    }
}
#[test]
fn old_nested_mode_lineage_repairs_only_recorded_modes_before_full_preflight() {
    for changed in [false, true] {
        let f = Fixture::new();
        let parent = f.0.join("work/tree");
        std::fs::create_dir(&parent).unwrap();
        let child = parent.join("child");
        std::fs::write(&child, b"keep").unwrap();
        for path in [&parent, &child] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let changes = attrs::set_modes(
            &[parent.clone(), child.clone()],
            attrs::ModeSpec::Exact(0o700),
        );
        assert_eq!(changes.changed.len(), 2);
        let mut history = f.open();
        history
            .record_modes("Nested modes", &changes.changed)
            .unwrap();
        history.undo(&CancelToken::new()).unwrap();
        drop(history);
        let mut store: serde_json::Value =
            serde_json::from_slice(&std::fs::read(f.store()).unwrap()).unwrap();
        let mut stale = 0;
        for step in store["records"][0]["steps"].as_array_mut().unwrap() {
            if let Some(lineage) = step
                .get_mut("Attr")
                .and_then(|step| step.get_mut("lineage"))
                .and_then(serde_json::Value::as_array_mut)
            {
                for node in lineage {
                    if node["relative"] == serde_json::json!(b"child".to_vec()) {
                        node["mode"] = serde_json::json!(0o100700);
                        stale += 1;
                    }
                }
            }
        }
        assert_eq!(stale, 1);
        std::fs::write(f.store(), serde_json::to_vec(&store).unwrap()).unwrap();
        if changed {
            std::fs::write(&child, b"new user work").unwrap();
        }
        let mut history = f.open();
        let result = history.redo(&CancelToken::new());
        if changed {
            assert!(matches!(result, Err(HistoryError::Changed(_))));
            assert_eq!(history.entries()[0].status, HistoryStatus::Undone);
            assert_eq!(std::fs::read(&child).unwrap(), b"new user work");
            assert_eq!(
                std::fs::metadata(&parent).unwrap().permissions().mode() & 0o7777,
                0o755
            );
            history.record_barrier("Untracked changed data").unwrap();
        } else {
            result.unwrap();
            assert_eq!(
                std::fs::metadata(&child).unwrap().permissions().mode() & 0o7777,
                0o700
            );
        }
    }
}
#[test]
fn nested_mode_batch_tracks_all_hardlink_aliases_without_adopting_new_work() {
    let f = Fixture::new();
    let parent = f.0.join("work/tree");
    std::fs::create_dir(&parent).unwrap();
    let child = parent.join("child");
    let alias = parent.join("alias");
    std::fs::write(&child, b"keep").unwrap();
    std::fs::hard_link(&child, &alias).unwrap();
    for p in [&parent, &child] {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let changes = attrs::set_modes(
        &[parent.clone(), child.clone()],
        attrs::ModeSpec::Exact(0o700),
    );
    let mut history = f.open();
    history
        .record_modes("Aliased nested modes", &changes.changed)
        .unwrap();
    history.undo(&CancelToken::new()).unwrap();
    drop(history);
    let mut history = f.open();
    history.redo(&CancelToken::new()).unwrap();
    assert_eq!(
        std::fs::metadata(alias).unwrap().permissions().mode() & 0o7777,
        0o700
    );
    std::fs::write(parent.join("new"), b"later work").unwrap();
    assert!(history.undo(&CancelToken::new()).is_err());
    assert_eq!(std::fs::read(parent.join("new")).unwrap(), b"later work");
}
#[test]
fn completed_merged_and_new_destinations_never_admit_a_partial_inverse() {
    use gust_ops::{
        jobs::{self, Request},
        journal::ConflictPolicy,
    };
    let f = Fixture::new();
    let mut history = f.open();
    let older = f.create(OsStr::new("older"));
    history
        .record_created("Older", std::slice::from_ref(&older))
        .unwrap();
    history.undo(&CancelToken::new()).unwrap();
    let before = history.entries();
    let source = f.0.join("source");
    let destination = f.0.join("destination");
    std::fs::create_dir_all(source.join("tree")).unwrap();
    std::fs::create_dir_all(destination.join("tree")).unwrap();
    std::fs::write(source.join("tree/copied"), b"copied child").unwrap();
    std::fs::write(source.join("separate"), b"copied file").unwrap();
    std::fs::write(destination.join("tree/kept"), b"existing work").unwrap();
    let mut job = jobs::start(
        "mixed-copy",
        Request::Copy {
            sources: vec![source.join("tree"), source.join("separate")],
            dest_dir: destination.clone(),
        },
        ConflictPolicy::Replace,
    );
    assert_eq!(job.wait().state, JobState::Confirmed);
    let record = job.record();
    assert_eq!(record.entries.iter().filter(|entry| entry.done).count(), 2);
    assert!(record
        .preexisting_at_dest
        .contains(&destination.join("tree")));
    let result = history.record_job("Mixed copy", &record);
    assert!(
        matches!(result, Err(HistoryError::Unsupported(_))),
        "whole mixed job must refuse, not offer a partial inverse: {result:?}"
    );
    assert_eq!(history.entries(), before);
    drop(history);
    let mut history = f.open();
    assert_eq!(history.entries(), before);
    history.redo(&CancelToken::new()).unwrap();
    assert_eq!(std::fs::read(older.path).unwrap(), b"original");
    assert_eq!(
        std::fs::read(destination.join("tree/copied")).unwrap(),
        b"copied child"
    );
    assert_eq!(
        std::fs::read(destination.join("tree/kept")).unwrap(),
        b"existing work"
    );
    assert_eq!(
        std::fs::read(destination.join("separate")).unwrap(),
        b"copied file"
    );
}
fn other_group() -> Option<u32> {
    rustix::process::getgroups()
        .unwrap_or_default()
        .into_iter()
        .map(|gid| gid.as_raw())
        .find(|gid| *gid != rustix::process::getegid().as_raw())
}
#[test]
fn setgid_directory_owner_undo_reopen_redo_does_not_fence_history() {
    use std::os::unix::fs::MetadataExt;
    let Some(other) = other_group() else {
        gust_testkit::skip!(
            "GROUPS",
            "setgid ownership fixture needs a second eligible group"
        );
    };
    let f = Fixture::new();
    let directory = f.0.join("work/tree");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("file"), b"keep").unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o2755)).unwrap();
    let old = std::fs::metadata(&directory).unwrap().gid();
    let changed = attrs::set_owners(
        std::slice::from_ref(&directory),
        attrs::OwnerSpec {
            uid: None,
            gid: Some(other),
        },
    );
    assert!(changed.failed.is_empty(), "{changed:?}");
    assert_eq!(changed.changed.len(), 1);
    assert_eq!(
        std::fs::metadata(&directory).unwrap().permissions().mode() & 0o7777,
        0o2755
    );
    let mut history = f.open();
    history
        .record_owners("Directory group", &changed.changed)
        .unwrap();
    history
        .undo(&CancelToken::new())
        .expect("forward-supported setgid directory owner must undo");
    assert_eq!(std::fs::metadata(&directory).unwrap().gid(), old);
    drop(history);
    let mut history = f.open();
    history.redo(&CancelToken::new()).unwrap();
    assert_eq!(std::fs::metadata(&directory).unwrap().gid(), other);
    assert_eq!(
        std::fs::metadata(&directory).unwrap().permissions().mode() & 0o7777,
        0o2755
    );
    assert_eq!(std::fs::read(directory.join("file")).unwrap(), b"keep");
    history.undo(&CancelToken::new()).unwrap();
    assert!(!history
        .entries()
        .iter()
        .any(|entry| entry.status == HistoryStatus::Interrupted));
}
#[test]
fn privileged_regular_owner_inverse_refuses_before_pending() {
    let Some(other) = other_group() else {
        gust_testkit::skip!("GROUPS", "ownership fixture needs a second eligible group");
    };
    let f = Fixture::new();
    let file = f.create(OsStr::new("file"));
    let changed = attrs::set_owners(
        std::slice::from_ref(&file.path),
        attrs::OwnerSpec {
            uid: None,
            gid: Some(other),
        },
    );
    assert_eq!(changed.changed.len(), 1);
    let mut history = f.open();
    history
        .record_owners("File group", &changed.changed)
        .unwrap();
    std::fs::set_permissions(&file.path, std::fs::Permissions::from_mode(0o2755)).unwrap();
    assert!(matches!(
        history.undo(&CancelToken::new()),
        Err(HistoryError::Unsupported(_))
    ));
    drop(history);
    let mut history = f.open();
    assert!(!history
        .entries()
        .iter()
        .any(|entry| entry.status == HistoryStatus::Interrupted));
    history
        .record_barrier("Explicit unsupported owner inverse")
        .unwrap();
}
#[test]
fn privileged_regular_owner_admission_refuses_without_recording_or_effects() {
    let Some(other) = other_group() else {
        gust_testkit::skip!("GROUPS", "ownership fixture needs a second eligible group");
    };
    let f = Fixture::new();
    let file = f.create(OsStr::new("file"));
    let changes = attrs::set_owners(
        std::slice::from_ref(&file.path),
        attrs::OwnerSpec {
            uid: None,
            gid: Some(other),
        },
    );
    assert_eq!(changes.changed.len(), 1);
    std::fs::set_permissions(&file.path, std::fs::Permissions::from_mode(0o2755)).unwrap();
    let mut history = f.open();
    assert!(matches!(
        history.record_owners("Unsupported file group", &changes.changed),
        Err(HistoryError::Unsupported(_))
    ));
    assert!(history.entries().is_empty());
    assert_eq!(
        std::fs::metadata(file.path).unwrap().permissions().mode() & 0o7777,
        0o2755
    );
    history
        .record_barrier("Untracked privileged owner state")
        .unwrap();
}
#[test]
fn cancel_and_barriers_stop_a_batch_without_skipping_earlier_steps() {
    let f = Fixture::new();
    let mut h = f.open();
    let old = f.create(OsStr::new("old"));
    h.record_created("Old", &[old]).unwrap();
    h.record_barrier("Untracked mutation").unwrap();
    let new = f.create(OsStr::new("new"));
    h.record_created("New", std::slice::from_ref(&new)).unwrap();
    let cancel = CancelToken::new();
    cancel.cancel();
    assert_eq!(h.undo(&cancel), Err(HistoryError::Cancelled));
    assert!(new.path.exists());
    let result = h.undo_many(3, &CancelToken::new());
    assert_eq!(result.completed.len(), 1);
    assert!(matches!(result.refusal, Some(HistoryError::Barrier(_))));
    assert!(!new.path.exists());
    assert!(f.0.join("work/old").exists());
}

#[test]
fn owned_folder_receipts_are_empty_only_and_keep_descendant_history() {
    let f = Fixture::new();
    let made = create::new_folder_recorded(&f.0.join("work"), OsStr::new("folder")).unwrap();
    let mut h = f.open();
    h.record_folders("New folder", std::slice::from_ref(&made))
        .unwrap();
    let file = create::create_from_bytes(&made.path, OsStr::new("child"), b"retained").unwrap();
    h.record_created("New child", &[file]).unwrap();
    let result = h.undo_many(2, &CancelToken::new());
    assert!(result.refusal.is_none(), "{result:?}");
    assert!(!made.path.exists());
    let result = h.redo_many(2, &CancelToken::new());
    assert!(result.refusal.is_none(), "{result:?}");
    assert_eq!(std::fs::read(made.path.join("child")).unwrap(), b"retained");
    let second = create::new_folder_recorded(&f.0.join("work"), OsStr::new("late")).unwrap();
    std::fs::write(second.path.join("user-work"), b"keep").unwrap();
    assert!(matches!(
        h.record_folders("Unadmitted folder", &[second]),
        Err(HistoryError::NewWork(_))
    ));
}
#[test]
fn folder_and_link_creation_refuse_symlinked_parents_and_occupied_names() {
    let f = Fixture::new();
    std::os::unix::fs::symlink(f.0.join("work"), f.0.join("redirect")).unwrap();
    assert!(create::new_folder_recorded(&f.0.join("redirect"), OsStr::new("new-folder")).is_err());
    assert!(!f.0.join("work/new-folder").exists());
    assert!(attrs::create_link(
        &f.0.join("target"),
        &f.0.join("redirect"),
        OsStr::new("new-link")
    )
    .is_err());
    assert!(f.0.join("work/new-link").symlink_metadata().is_err());
    std::os::unix::fs::symlink("absent", f.0.join("work/taken")).unwrap();
    assert!(create::new_folder_recorded(&f.0.join("work"), OsStr::new("taken")).is_err());
    assert_eq!(
        std::fs::read_link(f.0.join("work/taken")).unwrap(),
        PathBuf::from("absent")
    );
}

#[test]
fn permission_history_handles_owned_unreadable_files_and_directories() {
    let f = Fixture::new();
    let file = f.create(OsStr::new("file"));
    let mut h = f.open();
    std::fs::set_permissions(&file.path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let changed = attrs::set_modes(
        std::slice::from_ref(&file.path),
        attrs::ModeSpec::Exact(0o000),
    );
    assert!(changed.failed.is_empty());
    h.record_modes("Unreadable", &changed.changed).unwrap();
    h.undo(&CancelToken::new()).unwrap();
    assert_eq!(
        std::fs::metadata(&file.path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    h.redo(&CancelToken::new()).unwrap();
    assert_eq!(
        std::fs::metadata(&file.path).unwrap().permissions().mode() & 0o777,
        0o0
    );
    let changed = attrs::set_modes(
        std::slice::from_ref(&file.path),
        attrs::ModeSpec::Exact(0o600),
    );
    h.record_modes("Readable", &changed.changed).unwrap();
    h.undo(&CancelToken::new()).unwrap();
    h.redo(&CancelToken::new()).unwrap();
    let dir = f.0.join("work/dir");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let changed = attrs::set_modes(std::slice::from_ref(&dir), attrs::ModeSpec::Exact(0o000));
    h.record_modes("Directory unreadable", &changed.changed)
        .unwrap();
    h.undo(&CancelToken::new()).unwrap();
    h.redo(&CancelToken::new()).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
}
#[test]
fn hard_link_tag_aliases_are_one_reversible_attribute_transition() {
    let f = Fixture::new();
    let file = f.create(OsStr::new("one"));
    let alias = f.0.join("work/two");
    std::fs::hard_link(&file.path, &alias).unwrap();
    let paths = vec![file.path.clone(), alias.clone()];
    let changes = gust_ops::tags::set_tags(&paths, gust_ops::tags::TagOp::Toggle(1));
    assert_eq!(changes.changed.len(), 2);
    let mut h = f.open();
    h.record_tags("Two aliases", &changes.changed).unwrap();
    h.undo(&CancelToken::new()).unwrap();
    for path in &paths {
        assert_eq!(gust_core::tags::read_raw(path).unwrap(), None);
    }
    drop(h);
    let mut h = f.open();
    assert_eq!(h.entries()[0].status, HistoryStatus::Undone);
    h.redo(&CancelToken::new()).unwrap();
    for path in &paths {
        assert!(gust_core::tags::read_raw(path).unwrap().is_some());
    }
}
#[test]
fn real_new_extraction_receipt_survives_a_tracked_child_rename_round_trip() {
    use gust_ops::archive::{
        io::{self, ArchiveFormat},
        jobs::{start_journalled, ArchiveRequest},
    };
    let f = Fixture::new();
    let source = f.0.join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("before"), b"contents").unwrap();
    let archive = f.0.join("input.zip");
    io::create_with(
        &source,
        &["before".into()],
        &archive,
        ArchiveFormat::Zip,
        None,
        &CancelToken::new(),
        &mut |_, _, _| {},
    )
    .unwrap();
    let mut h = f.open();
    let destination = f.0.join("work/out");
    let mut job = start_journalled(
        "extract",
        ArchiveRequest::ExtractIntoNew {
            archive,
            destination: destination.clone(),
            policy: Default::default(),
            password: None,
        },
        gust_ops::Journal::new(f.0.join("state/journal")),
    )
    .unwrap();
    let result = job.wait().result.unwrap().unwrap();
    assert_eq!(
        result.record.subtree_identities.last().unwrap().0,
        destination
    );
    h.record_job("Extracted", &result.record).unwrap();
    let renamed = attrs::rename_recorded(&destination.join("before"), OsStr::new("after")).unwrap();
    h.record_renames("Renamed", &[renamed]).unwrap();
    let undone = h.undo_many(2, &CancelToken::new());
    assert!(undone.refusal.is_none(), "{undone:?}");
    assert_eq!(undone.completed.len(), 2);
    assert!(!destination.exists());
}
#[test]
fn directory_attributes_reconcile_only_fully_undone_tracked_children() {
    let f = Fixture::new();
    let directory = f.0.join("work/dir");
    std::fs::create_dir(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755)).unwrap();
    let changes = attrs::set_modes(
        std::slice::from_ref(&directory),
        attrs::ModeSpec::Exact(0o700),
    );
    let mut h = f.open();
    h.record_modes("Directory mode", &changes.changed).unwrap();
    let child = create::create_from_bytes(&directory, OsStr::new("child"), b"safe").unwrap();
    h.record_created("Child", &[child]).unwrap();
    let undone = h.undo_many(2, &CancelToken::new());
    assert!(undone.refusal.is_none(), "{undone:?}");
    assert_eq!(
        std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o755
    );
    let redone = h.redo_many(2, &CancelToken::new());
    assert!(redone.refusal.is_none(), "{redone:?}");
    assert_eq!(std::fs::read(directory.join("child")).unwrap(), b"safe");
}

#[test]
fn directory_attribute_refresh_refuses_untracked_new_work() {
    let f = Fixture::new();
    let directory = f.0.join("work/dir");
    std::fs::create_dir(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755)).unwrap();
    let changes = attrs::set_modes(
        std::slice::from_ref(&directory),
        attrs::ModeSpec::Exact(0o700),
    );
    let mut h = f.open();
    h.record_modes("Directory mode", &changes.changed).unwrap();
    let child = create::create_from_bytes(&directory, OsStr::new("child"), b"tracked").unwrap();
    h.record_created("Child", &[child]).unwrap();
    std::fs::write(directory.join("later"), b"new work").unwrap();
    h.undo(&CancelToken::new()).unwrap();
    assert!(matches!(
        h.undo(&CancelToken::new()),
        Err(HistoryError::Changed(_) | HistoryError::NewWork(_))
    ));
    assert_eq!(std::fs::read(directory.join("later")).unwrap(), b"new work");
    assert_eq!(
        std::fs::metadata(directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[test]
fn readable_directory_mode_redoes_after_undo_to_zero_and_reopen() {
    for original_mode in [0o000, 0o100, 0o400] {
        let f = Fixture::new();
        let directory = f.0.join("work/dir");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("child"), b"retained contents").unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(original_mode))
            .unwrap();
        let changes = attrs::set_modes(
            std::slice::from_ref(&directory),
            attrs::ModeSpec::Exact(0o700),
        );
        assert!(changes.failed.is_empty());
        let mut h = f.open();
        h.record_modes("Readable directory", &changes.changed)
            .unwrap();
        h.undo(&CancelToken::new()).unwrap();
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            original_mode
        );
        drop(h);
        let mut h = f.open();
        h.redo(&CancelToken::new()).unwrap();
        assert_eq!(
            std::fs::read(directory.join("child")).unwrap(),
            b"retained contents"
        );
        h.undo(&CancelToken::new()).unwrap();
        drop(h);
        f.open().redo(&CancelToken::new()).unwrap();
    }
}

#[test]
fn unreadable_directory_redo_preserves_deep_edits_and_the_pending_fence() {
    let f = Fixture::new();
    let directory = f.0.join("work/dir");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("child"), b"original").unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o000)).unwrap();
    let changes = attrs::set_modes(
        std::slice::from_ref(&directory),
        attrs::ModeSpec::Exact(0o700),
    );
    let mut h = f.open();
    let entry = h
        .record_modes("Readable directory", &changes.changed)
        .unwrap();
    h.undo(&CancelToken::new()).unwrap();
    let root_before = gust_ops::FileIdentity::of_path(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(directory.join("child"), b"later deep work").unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert!(
        root_before.matches(&gust_ops::FileIdentity::of_path(&directory).unwrap()),
        "root identity alone must not authorize the inverse"
    );
    drop(h);
    let mut h = f.open();
    assert!(matches!(
        h.redo(&CancelToken::new()),
        Err(HistoryError::Partial { .. })
    ));
    assert_eq!(
        std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o000
    );
    drop(h);
    let mut h = f.open();
    assert_eq!(h.entries()[0].status, HistoryStatus::Interrupted);
    assert_eq!(
        h.redo(&CancelToken::new()),
        Err(HistoryError::Barrier(entry.id))
    );
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        std::fs::read(directory.join("child")).unwrap(),
        b"later deep work"
    );
}
