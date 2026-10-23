//! The Trash view: list what is in the Trash (from the freedesktop
//! `.trashinfo` records, which other file managers also write), and restore
//! an item to where it came from — by name, days later, not just Ctrl+Z.

use gust_ops::trash::{list, restore_named, trash_path, TrashDir};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

fn setup(tag: &str) -> (PathBuf, TrashDir) {
    let d = std::env::temp_dir().join(format!("gust-trashview-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(d.join("work")).unwrap();
    let t = TrashDir::at(d.join("trash"));
    (d, t)
}

#[test]
fn listing_shows_name_origin_and_date() {
    let (d, t) = setup("list");
    fs::write(d.join("work/report final.pdf"), b"pdf").unwrap();
    trash_path(&d.join("work/report final.pdf"), &t).unwrap();
    let items = list(&t);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].original, d.join("work/report final.pdf"));
    assert_eq!(items[0].trashed_name, "report final.pdf");
    assert!(items[0].deleted.len() >= 16, "{}", items[0].deleted);
}

#[test]
fn entries_written_by_other_file_managers_are_listed() {
    // Nautilus writes the same format; a percent-encoded Path must decode.
    let (d, t) = setup("foreign");
    fs::create_dir_all(t.root().join("files")).unwrap();
    fs::create_dir_all(t.root().join("info")).unwrap();
    for path in [t.root(), &t.files, &t.info] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(t.root().join("files/café.txt"), b"x").unwrap();
    fs::write(
        t.root().join("info/café.txt.trashinfo"),
        format!(
            "[Trash Info]\nPath={}/work/caf%C3%A9.txt\nDeletionDate=2026-09-01T10:00:00\n",
            d.display()
        ),
    )
    .unwrap();
    let items = list(&t);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].original, d.join("work/café.txt"));
    assert_eq!(items[0].deleted, "2026-09-01T10:00:00");
}

#[test]
fn a_record_without_its_file_or_a_file_without_a_record_is_not_listed_as_restorable() {
    let (_d, t) = setup("orphans");
    fs::create_dir_all(t.root().join("files")).unwrap();
    fs::create_dir_all(t.root().join("info")).unwrap();
    for path in [t.root(), &t.files, &t.info] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(
        t.root().join("info/gone.trashinfo"),
        "[Trash Info]\nPath=/x/gone\nDeletionDate=2026-01-01T00:00:00\n",
    )
    .unwrap();
    fs::write(t.root().join("files/stray"), b"x").unwrap();
    assert!(list(&t).is_empty());
}

#[test]
fn restoring_by_name_puts_it_back_and_removes_the_record() {
    let (d, t) = setup("restore");
    fs::write(d.join("work/a.txt"), b"contents").unwrap();
    trash_path(&d.join("work/a.txt"), &t).unwrap();
    restore_named(&t, "a.txt".as_ref()).unwrap();
    assert_eq!(fs::read(d.join("work/a.txt")).unwrap(), b"contents");
    assert!(list(&t).is_empty());
    assert!(!t.root().join("info/a.txt.trashinfo").exists());
}

#[test]
fn restoring_never_overwrites_something_new_at_the_original_path() {
    let (d, t) = setup("occupied");
    fs::write(d.join("work/a.txt"), b"old").unwrap();
    trash_path(&d.join("work/a.txt"), &t).unwrap();
    fs::write(d.join("work/a.txt"), b"new work").unwrap();
    assert!(restore_named(&t, "a.txt".as_ref()).is_err());
    assert_eq!(fs::read(d.join("work/a.txt")).unwrap(), b"new work");
    assert_eq!(list(&t).len(), 1, "still in the Trash, still restorable");
}

#[test]
fn a_restored_folder_whose_parent_was_deleted_gets_its_parent_back() {
    let (d, t) = setup("parent");
    fs::create_dir_all(d.join("work/sub")).unwrap();
    fs::write(d.join("work/sub/f"), b"1").unwrap();
    trash_path(&d.join("work/sub"), &t).unwrap();
    fs::remove_dir_all(d.join("work")).unwrap();
    restore_named(&t, "sub".as_ref()).unwrap();
    assert_eq!(fs::read(d.join("work/sub/f")).unwrap(), b"1");
}

#[test]
fn a_hostile_record_cannot_restore_into_a_relative_path() {
    let (_d, t) = setup("hostile");
    fs::create_dir_all(t.root().join("files")).unwrap();
    fs::create_dir_all(t.root().join("info")).unwrap();
    for path in [t.root(), &t.files, &t.info] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(t.root().join("files/evil"), b"x").unwrap();
    fs::write(
        t.root().join("info/evil.trashinfo"),
        "[Trash Info]\nPath=../../.bashrc\nDeletionDate=2026-01-01T00:00:00\n",
    )
    .unwrap();
    assert!(list(&t).is_empty());
    assert!(restore_named(&t, "evil".as_ref()).is_err());
}

#[test]
fn the_deletion_date_is_local_time_like_every_other_file_manager_writes() {
    // The freedesktop Trash spec says DeletionDate is local time, and GIO
    // (Nautilus) writes it that way. Gust wrote UTC, so in CEST an item
    // deleted at 20:15 was listed as 18:15 — next to Nautilus' correct one.
    let (d, t) = setup("localtime");
    fs::write(d.join("work/x"), b"1").unwrap();
    trash_path(&d.join("work/x"), &t).unwrap();
    let written = list(&t)[0].deleted.clone();
    let offset = gust_core::properties::local_offset_seconds();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let expected = gust_core::properties::full_time(now, offset); // "YYYY-MM-DD HH:MM"
    assert_eq!(written[..16].replace('T', " "), expected, "wrote {written}");
}
