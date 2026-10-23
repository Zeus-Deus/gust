use gust_core::lister::CancelToken;
use gust_ops::{
    anchor::FileIdentity,
    history::{History, HistoryError},
    trash::{self, TrashDir, TrashLocation},
};
use std::{
    ffi::{OsStr, OsString},
    fs,
    os::unix::{
        ffi::OsStringExt,
        fs::{symlink, PermissionsExt},
    },
    path::PathBuf,
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "gust-volume-trash-{}-{}",
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
    fn volume(&self) -> TrashLocation {
        TrashLocation {
            trash: TrashDir::at(
                self.0
                    .join(format!(".Trash-{}", rustix::process::geteuid().as_raw())),
            ),
            topdir: Some(self.0.clone()),
        }
    }
    fn file(&self) -> PathBuf {
        let p = self.0.join("work/file");
        fs::write(&p, b"original").unwrap();
        p
    }
    fn history(&self) -> History {
        History::open(self.0.join("state/history.json")).unwrap()
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
#[test]
fn selection_is_read_only_and_prefers_same_volume_home() {
    let f = Fixture::new();
    let file = f.file();
    let home = f.location().trash;
    let chosen = trash::select_location(&file, &home).unwrap();
    assert_eq!(chosen.trash, home);
    assert!(chosen.topdir.is_none());
    assert!(!home.root().exists());
    assert!(!trash::locations(&home)
        .unwrap()
        .iter()
        .any(|l| l.trash == home));
    assert!(!home.root().exists());
}
#[test]
fn volume_record_preserves_raw_relative_bytes_and_private_modes() {
    let f = Fixture::new();
    let l = f.volume();
    let name = OsString::from_vec(b"raw \xff\n".to_vec());
    let file = f.0.join("work").join(&name);
    fs::write(&file, b"bytes").unwrap();
    let r = l.trash_path(&file, &cancel()).unwrap();
    let info = fs::read(&r.item.info_file).unwrap();
    assert!(String::from_utf8(info)
        .unwrap()
        .contains("Path=work/raw%20%FF%0A\n"));
    for p in [l.trash.root(), &l.trash.files, &l.trash.info] {
        assert_eq!(fs::metadata(p).unwrap().permissions().mode() & 0o777, 0o700);
    }
    assert_eq!(l.list().unwrap()[0].original, file);
    let admitted = l.receipt_named(&name, &cancel()).unwrap();
    let id = FileIdentity::of_path(&l.trash.files.join(&name)).unwrap();
    let info_id = FileIdentity::of_path(&admitted.item.info_file).unwrap();
    let restored = l.restore(&admitted, &cancel()).unwrap();
    assert!(restored.history_supported());
    assert_eq!(FileIdentity::of_path(&file).unwrap(), id);
    assert!(!admitted.item.info_file.exists());
    let retained = fs::read_dir(&l.trash.info)
        .unwrap()
        .filter_map(Result::ok)
        .find(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".gust-restore-")
        })
        .unwrap()
        .path()
        .join("info");
    assert_eq!(FileIdentity::of_path(&retained).unwrap(), info_id);
}
#[test]
fn unsafe_shared_and_private_trash_are_refused() {
    let f = Fixture::new();
    let file = f.file();
    let l = f.volume();
    fs::create_dir(l.trash.root()).unwrap();
    fs::set_permissions(l.trash.root(), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(l.trash_path(&file, &cancel()).is_err());
    assert!(file.exists());
    let shared = f.0.join(".Trash");
    fs::create_dir(&shared).unwrap();
    let location = TrashLocation {
        trash: TrashDir::at(shared.join(rustix::process::geteuid().as_raw().to_string())),
        topdir: Some(f.0.clone()),
    };
    assert!(location.trash_path(&file, &cancel()).is_err());
    assert!(!location.trash.root().exists());
}
#[test]
fn sticky_shared_layout_and_existing_inventory_work_without_extra_creation() {
    let f = Fixture::new();
    let shared = f.0.join(".Trash");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o1777)).unwrap();
    let l = TrashLocation {
        trash: TrashDir::at(shared.join(rustix::process::geteuid().as_raw().to_string())),
        topdir: Some(f.0.clone()),
    };
    let file = f.file();
    l.trash_path(&file, &cancel()).unwrap();
    assert_eq!(l.list().unwrap().len(), 1);
}
#[test]
fn duplicate_names_keep_distinct_inodes_and_info_records() {
    let f = Fixture::new();
    let l = f.location();
    let file = f.file();
    let one = l.trash_path(&file, &cancel()).unwrap();
    fs::write(&file, b"later").unwrap();
    let two = l.trash_path(&file, &cancel()).unwrap();
    assert_ne!(one.item.trashed_name, two.item.trashed_name);
    assert_eq!(l.list().unwrap().len(), 2);
    assert_eq!(
        fs::read(l.trash.files.join(&one.item.trashed_name)).unwrap(),
        b"original"
    );
}
#[test]
fn occupied_and_dangling_restore_names_never_replace_work() {
    for link in [false, true] {
        let f = Fixture::new();
        let l = f.location();
        let file = f.file();
        let r = l.trash_path(&file, &cancel()).unwrap();
        if link {
            symlink("missing", &file).unwrap();
        } else {
            fs::write(&file, b"later").unwrap();
        }
        assert_eq!(
            l.restore(&r, &cancel()).unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert!(r.item.info_file.exists());
        assert!(l.trash.files.join(&r.item.trashed_name).exists());
        if !link {
            assert_eq!(fs::read(file).unwrap(), b"later");
        }
    }
}
#[test]
fn modified_projection_never_redirects_restore() {
    let f = Fixture::new();
    let l = f.location();
    let file = f.file();
    let r = l.trash_path(&file, &cancel()).unwrap();
    let mut changed = r.clone();
    changed.item.original = f.0.join("redirect");
    assert!(l.restore(&changed, &cancel()).is_err());
    let mut changed = r.clone();
    changed.location.topdir = Some(f.0.clone());
    assert!(l.restore(&changed, &cancel()).is_err());
    let mut changed = r;
    changed.item.trashed_name = OsString::from("other");
    assert!(l.restore(&changed, &cancel()).is_err());
    assert!(!f.0.join("redirect").exists());
}
#[test]
fn edited_info_replaced_data_and_replaced_parent_refuse() {
    for kind in 0..3 {
        let f = Fixture::new();
        let l = f.location();
        let file = f.file();
        let r = l.trash_path(&file, &cancel()).unwrap();
        let data = l.trash.files.join(&r.item.trashed_name);
        match kind {
            0 => fs::write(&r.item.info_file, b"changed").unwrap(),
            1 => {
                fs::rename(&data, f.0.join("kept")).unwrap();
                fs::write(&data, b"replacement").unwrap();
            }
            _ => {
                fs::rename(&l.trash.files, f.0.join("files-kept")).unwrap();
                fs::create_dir(&l.trash.files).unwrap();
                fs::set_permissions(&l.trash.files, fs::Permissions::from_mode(0o700)).unwrap();
                fs::write(&data, b"replacement").unwrap();
            }
        }
        assert!(l.restore(&r, &cancel()).is_err());
        assert!(!file.exists());
        assert!(data.exists());
    }
}
#[test]
fn new_descendants_and_deep_edits_refuse_restore_receipt() {
    for new in [false, true] {
        let f = Fixture::new();
        let l = f.location();
        let dir = f.0.join("work/tree");
        fs::create_dir_all(dir.join("deep")).unwrap();
        fs::write(dir.join("deep/file"), b"before").unwrap();
        let r = l.trash_path(&dir, &cancel()).unwrap();
        let data = l.trash.files.join(&r.item.trashed_name);
        let p = data.join(if new { "deep/new" } else { "deep/file" });
        fs::write(&p, b"later").unwrap();
        assert!(l.restore(&r, &cancel()).is_err());
        assert_eq!(fs::read(p).unwrap(), b"later");
        assert!(!dir.exists());
    }
}
#[test]
fn cancelled_admission_has_no_storage_effect() {
    let f = Fixture::new();
    let l = f.location();
    let file = f.file();
    let c = cancel();
    c.cancel();
    assert_eq!(
        l.trash_path(&file, &c).unwrap_err().kind(),
        std::io::ErrorKind::Interrupted
    );
    assert!(file.exists());
    assert!(!l.trash.root().exists());
}
#[test]
fn missing_parent_restore_is_recoverable_but_requires_history_barrier() {
    let f = Fixture::new();
    let l = f.location();
    let file = f.file();
    let r = l.trash_path(&file, &cancel()).unwrap();
    assert!(r.restore_history_supported(&cancel()).unwrap());
    fs::remove_dir(file.parent().unwrap()).unwrap();
    assert!(!r.restore_history_supported(&cancel()).unwrap());
    let restored = l.restore(&r, &cancel()).unwrap();
    assert!(!restored.history_supported());
    assert_eq!(fs::read(file).unwrap(), b"original");
    assert!(matches!(
        f.history().record_restored("restore", &[restored]),
        Err(HistoryError::Unsupported(_))
    ));
}
#[test]
fn trash_history_roundtrips_both_inodes_across_reopen() {
    let f = Fixture::new();
    let l = f.location();
    let file = f.file();
    let original = FileIdentity::of_path(&file).unwrap();
    let r = l.trash_path(&file, &cancel()).unwrap();
    let info = FileIdentity::of_path(&r.item.info_file).unwrap();
    let mut h = f.history();
    h.record_trash("Trashed", std::slice::from_ref(&r)).unwrap();
    h.undo(&cancel()).unwrap();
    assert_eq!(FileIdentity::of_path(&file).unwrap(), original);
    assert!(!r.item.info_file.exists());
    drop(h);
    let mut h = f.history();
    h.redo(&cancel()).unwrap();
    assert!(!file.exists());
    assert_eq!(FileIdentity::of_path(&r.item.info_file).unwrap(), info);
    h.undo(&cancel()).unwrap();
    h.redo(&cancel()).unwrap();
    assert_eq!(
        FileIdentity::of_path(&l.trash.files.join(&r.item.trashed_name)).unwrap(),
        original
    );
}
#[test]
fn explicit_restore_history_roundtrips_data_and_original_record() {
    let f = Fixture::new();
    let l = f.location();
    let file = f.file();
    let r = l.trash_path(&file, &cancel()).unwrap();
    let info = FileIdentity::of_path(&r.item.info_file).unwrap();
    let data = FileIdentity::of_path(&l.trash.files.join(&r.item.trashed_name)).unwrap();
    let restored = l.restore(&r, &cancel()).unwrap();
    let mut h = f.history();
    h.record_restored("Restored", &[restored]).unwrap();
    h.undo(&cancel()).unwrap();
    assert!(!file.exists());
    assert_eq!(FileIdentity::of_path(&r.item.info_file).unwrap(), info);
    drop(h);
    let mut h = f.history();
    h.redo(&cancel()).unwrap();
    assert_eq!(FileIdentity::of_path(&file).unwrap(), data);
    assert!(!r.item.info_file.exists());
    h.undo(&cancel()).unwrap();
    h.redo(&cancel()).unwrap();
}
#[test]
fn restored_trash_root_excludes_retained_child_vault_across_reopen() {
    let f = Fixture::new();
    let location = f.location();
    let folder = f.0.join("work/tree");
    fs::create_dir(&folder).unwrap();
    let created =
        gust_ops::create::create_from_bytes(&folder, OsStr::new("file"), b"original").unwrap();
    let mut history = f.history();
    history.record_created("Create child", &[created]).unwrap();
    let receipt = location.trash_path(&folder, &cancel()).unwrap();
    let info = FileIdentity::of_path(&receipt.item.info_file).unwrap();
    let bytes = fs::read(&receipt.item.info_file).unwrap();
    history
        .record_trash("Trash tree", std::slice::from_ref(&receipt))
        .unwrap();
    history.undo(&cancel()).unwrap();
    drop(history);
    let mut history = f.history();
    history.undo(&cancel()).unwrap();
    assert!(
        fs::read_dir(&folder).unwrap().next().is_none(),
        "owned vault belongs outside the restored root"
    );
    drop(history);
    let mut history = f.history();
    history.redo(&cancel()).unwrap();
    history.redo(&cancel()).unwrap();
    assert!(!folder.exists());
    assert_eq!(
        FileIdentity::of_path(&receipt.item.info_file).unwrap(),
        info
    );
    assert_eq!(fs::read(&receipt.item.info_file).unwrap(), bytes);
    assert!(fs::read_dir(f.0.join("work")).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".gust-undo-")));
}
fn refreshed_directory_cycle(
    restoration: bool,
) -> (
    Fixture,
    TrashLocation,
    gust_ops::trash::TrashReceipt,
    FileIdentity,
    Vec<u8>,
) {
    refreshed_directory_cycle_with_legacy(restoration, false)
}
fn refreshed_directory_cycle_with_legacy(
    restoration: bool,
    legacy: bool,
) -> (
    Fixture,
    TrashLocation,
    gust_ops::trash::TrashReceipt,
    FileIdentity,
    Vec<u8>,
) {
    let f = Fixture::new();
    let location = f.location();
    let folder = f.0.join("work/tree");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("before"), b"original").unwrap();
    let mut history = f.history();
    if !restoration {
        let renamed =
            gust_ops::attrs::rename_recorded(&folder.join("before"), OsStr::new("after")).unwrap();
        history.record_renames("Rename child", &[renamed]).unwrap();
    }
    let receipt = location.trash_path(&folder, &cancel()).unwrap();
    let info = FileIdentity::of_path(&receipt.item.info_file).unwrap();
    let bytes = fs::read(&receipt.item.info_file).unwrap();
    if restoration {
        let restored = location.restore(&receipt, &cancel()).unwrap();
        history
            .record_restored("Restore tree", &[restored])
            .unwrap();
        let renamed =
            gust_ops::attrs::rename_recorded(&folder.join("before"), OsStr::new("after")).unwrap();
        history.record_renames("Rename child", &[renamed]).unwrap();
    } else {
        history
            .record_trash("Trash tree", std::slice::from_ref(&receipt))
            .unwrap();
        history.undo(&cancel()).unwrap();
    }
    drop(history);
    if legacy {
        let store = f.0.join("state/history.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&store).unwrap()).unwrap();
        for entry in value["records"].as_array_mut().unwrap() {
            entry.as_object_mut().unwrap().remove("native_trash");
        }
        fs::write(store, serde_json::to_vec(&value).unwrap()).unwrap();
    }
    let mut history = f.history();
    history.undo(&cancel()).unwrap();
    drop(history);
    let mut history = f.history();
    if restoration {
        history.undo(&cancel()).unwrap();
    } else {
        history.redo(&cancel()).unwrap();
        drop(history);
        history = f.history();
        history.redo(&cancel()).unwrap();
    }
    assert_eq!(
        FileIdentity::of_path(&receipt.item.info_file).unwrap(),
        info
    );
    assert_eq!(fs::read(&receipt.item.info_file).unwrap(), bytes);
    drop(history);
    (f, location, receipt, info, bytes)
}
#[test]
fn trash_directory_lineage_remains_native_acquirable_after_history_reopen() {
    let (_f, location, old, info, bytes) = refreshed_directory_cycle(false);
    assert_eq!(
        location.list().unwrap().len(),
        1,
        "validated Trash lineage must remain visible"
    );
    let receipt = location
        .receipt_named(&old.item.trashed_name, &cancel())
        .unwrap();
    location.restore(&receipt, &cancel()).unwrap();
    assert_eq!(
        fs::read(old.item.original.join("after")).unwrap(),
        b"original"
    );
    assert_eq!(
        info,
        FileIdentity::of_path(
            &location
                .trash
                .info
                .read_dir()
                .unwrap()
                .filter_map(Result::ok)
                .find(|entry| entry.path().join("info").exists())
                .unwrap()
                .path()
                .join("info")
        )
        .unwrap()
    );
    assert_eq!(bytes, receipt_bytes_after_restore(&location));
}
fn receipt_bytes_after_restore(location: &TrashLocation) -> Vec<u8> {
    let retained = fs::read_dir(&location.trash.info)
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| entry.path().join("info").exists())
        .unwrap();
    fs::read(retained.path().join("info")).unwrap()
}
#[test]
fn restored_directory_lineage_remains_native_acquirable_after_history_reopen() {
    let (_f, location, old, _, _) = refreshed_directory_cycle(true);
    assert_eq!(
        location.list().unwrap().len(),
        1,
        "validated Restore lineage must remain visible"
    );
    let receipt = location
        .receipt_named(&old.item.trashed_name, &cancel())
        .unwrap();
    location.restore(&receipt, &cancel()).unwrap();
    assert_eq!(
        fs::read(old.item.original.join("before")).unwrap(),
        b"original"
    );
}
#[test]
fn native_refreshed_lineage_rejects_untracked_changes() {
    for kind in 0..3 {
        let (_f, location, old, _, _) = refreshed_directory_cycle(false);
        let data = location.trash.files.join(&old.item.trashed_name);
        match kind {
            0 => fs::write(data.join("after"), b"later work").unwrap(),
            1 => fs::write(data.join("new"), b"later work").unwrap(),
            _ => fs::write(&old.item.info_file, b"later record").unwrap(),
        }
        assert!(location
            .receipt_named(&old.item.trashed_name, &cancel())
            .is_err());
        assert!(location.list().unwrap().is_empty());
        assert!(data.exists());
        assert!(old.item.info_file.exists());
    }
}
#[test]
fn legacy_typed_trash_inventory_keeps_its_native_binding_across_reopen() {
    for restoration in [false, true] {
        let (_f, location, old, _, _) = refreshed_directory_cycle_with_legacy(restoration, true);
        assert_eq!(location.list().unwrap().len(), 1);
        let receipt = location
            .receipt_named(&old.item.trashed_name, &cancel())
            .unwrap();
        location.restore(&receipt, &cancel()).unwrap();
    }
}
#[test]
fn changed_lineage_certificate_never_adopts_data_or_replaces_evidence() {
    let (f, location, old, info, bytes) = refreshed_directory_cycle(false);
    let receipt = location
        .receipt_named(&old.item.trashed_name, &cancel())
        .unwrap();
    let certificate = fs::read_dir(location.trash.info.join(".gust-lineage"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(&certificate, b"later certificate").unwrap();
    assert!(location.restore(&receipt, &cancel()).is_err());
    assert!(location
        .receipt_named(&old.item.trashed_name, &cancel())
        .is_err());
    assert!(location.list().is_err());
    let mut history = f.history();
    history.undo(&cancel()).unwrap();
    assert!(history.redo(&cancel()).is_err());
    drop(history);
    let mut history = f.history();
    assert!(history
        .entries()
        .iter()
        .any(|entry| entry.status == gust_ops::history::HistoryStatus::Interrupted));
    assert!(matches!(
        history.undo(&cancel()),
        Err(HistoryError::Barrier(_))
    ));
    assert_eq!(
        fs::read(old.item.original.join("after")).unwrap(),
        b"original"
    );
    assert_eq!(fs::read(&certificate).unwrap(), b"later certificate");
    assert_eq!(FileIdentity::of_path(&old.item.info_file).unwrap(), info);
    assert_eq!(fs::read(&old.item.info_file).unwrap(), bytes);
}
#[test]
fn retained_lineage_limit_fences_before_data_returns_to_trash() {
    let (f, location, old, info, bytes) = refreshed_directory_cycle(false);
    let mut history = f.history();
    history.undo(&cancel()).unwrap();
    history.undo(&cancel()).unwrap();
    history.redo(&cancel()).unwrap();
    let certificates = location.trash.info.join(".gust-lineage");
    for i in 0..4095 {
        fs::write(certificates.join(format!("retained-{i}")), b"").unwrap();
    }
    assert!(history.redo(&cancel()).is_err());
    drop(history);
    assert!(f
        .history()
        .entries()
        .iter()
        .any(|entry| entry.status == gust_ops::history::HistoryStatus::Interrupted));
    assert_eq!(
        fs::read(old.item.original.join("after")).unwrap(),
        b"original"
    );
    assert!(!location.trash.files.join(&old.item.trashed_name).exists());
    assert_eq!(FileIdentity::of_path(&old.item.info_file).unwrap(), info);
    assert_eq!(fs::read(&old.item.info_file).unwrap(), bytes);
    assert_eq!(fs::read_dir(certificates).unwrap().count(), 4096);
}
#[test]
fn history_occupied_original_and_info_refuse_before_any_step() {
    for occupied_info in [false, true] {
        let f = Fixture::new();
        let l = f.location();
        let file = f.file();
        let r = l.trash_path(&file, &cancel()).unwrap();
        let mut h = f.history();
        h.record_trash("Trashed", std::slice::from_ref(&r)).unwrap();
        if occupied_info {
            h.undo(&cancel()).unwrap();
            fs::write(&r.item.info_file, b"later record").unwrap();
            assert!(h.redo(&cancel()).is_err());
            assert!(file.exists());
            assert_eq!(fs::read(&r.item.info_file).unwrap(), b"later record");
        } else {
            fs::write(&file, b"new").unwrap();
            assert!(h.undo(&cancel()).is_err());
            assert_eq!(fs::read(&file).unwrap(), b"new");
            assert!(r.item.info_file.exists());
        }
    }
}
#[test]
fn history_new_work_and_unrecorded_info_edit_are_not_adopted() {
    let f = Fixture::new();
    let l = f.location();
    let dir = f.0.join("work/tree");
    fs::create_dir(&dir).unwrap();
    let r = l.trash_path(&dir, &cancel()).unwrap();
    fs::write(
        l.trash.files.join(&r.item.trashed_name).join("new"),
        b"later",
    )
    .unwrap();
    assert!(f
        .history()
        .record_trash("Trashed", std::slice::from_ref(&r))
        .is_err());
    assert!(r.item.info_file.exists());
    let f = Fixture::new();
    let l = f.location();
    let file = f.file();
    let r = l.trash_path(&file, &cancel()).unwrap();
    fs::write(&r.item.info_file, b"later record").unwrap();
    assert!(f.history().record_trash("Trashed", &[r]).is_err());
}
#[test]
fn legacy_home_wrapper_still_uses_safe_receipts() {
    let f = Fixture::new();
    let file = f.file();
    let l = f.location();
    let item = trash::trash_path(&file, &l.trash).unwrap();
    trash::restore(&item, &l.trash).unwrap();
    assert_eq!(fs::read(file).unwrap(), b"original");
}
#[test]
fn symlink_trash_preserves_link_and_never_follows_target() {
    let f = Fixture::new();
    let target = f.file();
    let link = f.0.join("work/link");
    symlink(&target, &link).unwrap();
    let l = f.location();
    let r = l.trash_path(&link, &cancel()).unwrap();
    l.restore(&r, &cancel()).unwrap();
    assert_eq!(fs::read_link(link).unwrap(), target);
    assert_eq!(fs::read(target).unwrap(), b"original");
}
#[test]
fn orphan_intent_is_retained_and_not_offered_as_restorable() {
    let f = Fixture::new();
    let l = f.location();
    let file = f.file();
    let r = l.trash_path(&file, &cancel()).unwrap();
    fs::rename(l.trash.files.join(&r.item.trashed_name), &file).unwrap();
    assert!(l.list().unwrap().is_empty());
    assert!(r.item.info_file.exists());
    assert!(l.receipt_named(OsStr::new("file"), &cancel()).is_err());
}
#[test]
fn home_records_reject_plain_relative_paths_too() {
    let f = Fixture::new();
    let l = f.location();
    let file = f.file();
    let r = l.trash_path(&file, &cancel()).unwrap();
    fs::write(
        &r.item.info_file,
        b"[Trash Info]\nPath=relative\nDeletionDate=2026-01-01T00:00:00\n",
    )
    .unwrap();
    assert!(l.list().unwrap().is_empty());
    assert!(l.receipt_named(&r.item.trashed_name, &cancel()).is_err());
    assert!(!l.trash.root().parent().unwrap().join("relative").exists());
}
#[test]
fn real_volume_selection_does_not_create_a_shared_trash() {
    let f = Fixture::new();
    let scratch = PathBuf::from("/dev/shm").join(f.0.file_name().unwrap());
    fs::create_dir(&scratch).unwrap();
    fs::set_permissions(&scratch, fs::Permissions::from_mode(0o700)).unwrap();
    let guard = Fixture(scratch);
    let file = guard.0.join("file");
    fs::write(&file, b"source").unwrap();
    let home = f.location().trash;
    let uid = rustix::process::geteuid().as_raw();
    let private = PathBuf::from(format!("/dev/shm/.Trash-{uid}"));
    let shared = PathBuf::from(format!("/dev/shm/.Trash/{uid}"));
    let before = (
        private.symlink_metadata().ok(),
        shared.symlink_metadata().ok(),
    );
    let location = trash::select_location(&file, &home).unwrap();
    if FileIdentity::of_path(&file).unwrap().dev != FileIdentity::of_path(&f.0).unwrap().dev {
        assert_eq!(
            location.topdir.as_deref(),
            Some(std::path::Path::new("/dev/shm"))
        );
        assert_ne!(location.trash, home);
    } else {
        assert_eq!(location.trash, home);
    }
    let after = (
        private.symlink_metadata().ok(),
        shared.symlink_metadata().ok(),
    );
    assert_eq!(before.0.is_some(), after.0.is_some());
    assert_eq!(before.1.is_some(), after.1.is_some());
    assert_eq!(fs::read(file).unwrap(), b"source");
    assert!(!home.root().exists());
}
#[test]
fn existing_home_inventory_is_read_only_and_returns_the_validated_location() {
    let f = Fixture::new();
    let l = f.location();
    let file = f.file();
    l.trash_path(&file, &cancel()).unwrap();
    let before = FileIdentity::of_path(l.trash.root()).unwrap();
    let inventory = trash::locations(&l.trash).unwrap();
    assert!(inventory.contains(&l));
    assert_eq!(FileIdentity::of_path(l.trash.root()).unwrap(), before);
    assert_eq!(l.list().unwrap().len(), 1);
}
#[test]
fn receipt_cache_charge_counts_private_inventory_and_never_accesses_disk() {
    let f = Fixture::new();
    let l = f.location();
    let tree = f.0.join("work/tree");
    fs::create_dir(&tree).unwrap();
    for i in 0..10 {
        fs::write(tree.join(format!("file-{i}")), b"data").unwrap();
    }
    let r = l.trash_path(&tree, &cancel()).unwrap();
    assert_eq!(r.inventory_len(), 11);
    let bytes = r.retained_bytes();
    assert!(bytes >= 11 * 2048);
    assert!(r.clone().retained_bytes() >= 11 * 2048);
    fs::remove_dir_all(&f.0).unwrap();
    assert_eq!(r.inventory_len(), 11);
    assert_eq!(r.retained_bytes(), bytes);
}

#[test]
fn legacy_upgrade_is_explicit_private_and_preserves_original_data_and_info() {
    let f = Fixture::new();
    let location = f.location();
    for dir in [
        location.trash.root(),
        &location.trash.files,
        &location.trash.info,
    ] {
        fs::create_dir_all(dir).unwrap();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let old = f.0.join("work/legacy");
    fs::write(location.trash.files.join("legacy"), b"old data").unwrap();
    let bytes = format!(
        "[Trash Info]\nPath={}\nDeletionDate=2026-01-01T12:00:00\n",
        old.display()
    )
    .into_bytes();
    let info = location.trash.info.join("legacy.trashinfo");
    fs::write(&info, &bytes).unwrap();
    let data_id = FileIdentity::of_path(&location.trash.files.join("legacy")).unwrap();
    let info_id = FileIdentity::of_path(&info).unwrap();
    let source = f.file();
    let selection_error = trash::select_location(&source, &location.trash).unwrap_err();
    assert!(selection_error
        .get_ref()
        .unwrap()
        .downcast_ref::<trash::LegacyTrashNeedsRepair>()
        .is_some());
    let error = trash::list_checked(&location.trash).unwrap_err();
    assert!(error
        .get_ref()
        .unwrap()
        .downcast_ref::<trash::LegacyTrashNeedsRepair>()
        .is_some());
    for dir in [
        location.trash.root(),
        &location.trash.files,
        &location.trash.info,
    ] {
        assert_eq!(
            fs::metadata(dir).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
    location.repair_legacy_permissions(&cancel()).unwrap();
    for dir in [
        location.trash.root(),
        &location.trash.files,
        &location.trash.info,
    ] {
        assert_eq!(
            fs::metadata(dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    assert_eq!(trash::list_checked(&location.trash).unwrap().len(), 1);
    assert_eq!(
        FileIdentity::of_path(&location.trash.files.join("legacy")).unwrap(),
        data_id
    );
    assert_eq!(FileIdentity::of_path(&info).unwrap(), info_id);
    assert_eq!(fs::read(&info).unwrap(), bytes);
    let receipt = location
        .receipt_named(OsStr::new("legacy"), &cancel())
        .unwrap();
    location.restore(&receipt, &cancel()).unwrap();
    assert_eq!(fs::read(&old).unwrap(), b"old data");
    location.repair_legacy_permissions(&cancel()).unwrap();
    let file = f.file();
    location.trash_path(&file, &cancel()).unwrap();
}
#[test]
fn legacy_repair_refuses_writable_and_symlinked_layouts_without_changing_them() {
    for unsafe_link in [false, true] {
        let f = Fixture::new();
        let location = f.location();
        fs::create_dir_all(&location.trash.info).unwrap();
        fs::set_permissions(location.trash.root(), fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&location.trash.info, fs::Permissions::from_mode(0o755)).unwrap();
        if unsafe_link {
            let outside = f.0.join("outside");
            fs::create_dir(&outside).unwrap();
            fs::set_permissions(&outside, fs::Permissions::from_mode(0o755)).unwrap();
            symlink(&outside, &location.trash.files).unwrap();
        } else {
            fs::create_dir(&location.trash.files).unwrap();
            fs::set_permissions(&location.trash.files, fs::Permissions::from_mode(0o777)).unwrap();
        }
        assert!(location.repair_legacy_permissions(&cancel()).is_err());
        assert_eq!(
            fs::metadata(location.trash.root())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(
            fs::metadata(&location.trash.info)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }
}

#[test]
fn cancelled_legacy_repair_has_no_permission_effects() {
    let f = Fixture::new();
    let location = f.location();
    for dir in [
        location.trash.root(),
        &location.trash.files,
        &location.trash.info,
    ] {
        fs::create_dir_all(dir).unwrap();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let cancel = cancel();
    cancel.cancel();
    let error = location.repair_legacy_permissions(&cancel).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    assert!(error
        .get_ref()
        .and_then(|error| error.downcast_ref::<trash::TrashPartial>())
        .is_none());
    for dir in [
        location.trash.root(),
        &location.trash.files,
        &location.trash.info,
    ] {
        assert_eq!(
            fs::metadata(dir).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
}
