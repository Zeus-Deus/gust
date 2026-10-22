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
