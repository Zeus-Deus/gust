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
