use gust_core::lister::CancelToken;
use gust_ops::undo::{try_snapshot_subtree, verify_subtree_unchanged, UndoRefusal};
use std::{os::unix::fs::PermissionsExt, path::PathBuf};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "gust-strict-snapshot-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(p.join("tree/locked")).unwrap();
        std::fs::write(p.join("tree/locked/file"), b"keep").unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(
            self.0.join("tree/locked"),
            std::fs::Permissions::from_mode(0o700),
        );
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn a_root_symlink_is_one_object_and_never_its_targets_descendants() {
    let f = Fixture::new();
    let link = f.0.join("link");
    std::os::unix::fs::symlink(f.0.join("tree"), &link).unwrap();
    let snapshot = try_snapshot_subtree(&link, &CancelToken::new()).unwrap();
    assert_eq!(snapshot.len(), 1);
    assert!(snapshot.contains_key(&link));
}
#[test]
fn unreadable_descendants_fail_the_whole_inventory() {
    let f = Fixture::new();
    let root = f.0.join("tree");
    let before = try_snapshot_subtree(&root, &CancelToken::new()).unwrap();
    std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o0)).unwrap();
    let result = try_snapshot_subtree(&root, &CancelToken::new());
    let refusal = verify_subtree_unchanged(&root, &before);
    std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    assert!(matches!(refusal, Some(UndoRefusal::IdentityUnknown(_))));
}
#[test]
fn cancellation_and_symlinked_store_parents_have_no_side_effects() {
    let f = Fixture::new();
    let cancel = CancelToken::new();
    cancel.cancel();
    assert_eq!(
        try_snapshot_subtree(&f.0.join("tree"), &cancel)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::Interrupted
    );
    std::os::unix::fs::symlink(f.0.join("tree"), f.0.join("redirect")).unwrap();
    assert!(gust_ops::history::History::open(f.0.join("redirect/new-state/history")).is_err());
    assert!(!f.0.join("tree/new-state").exists());
}
