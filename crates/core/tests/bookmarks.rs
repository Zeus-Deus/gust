//! Bookmarks, shared with the rest of the desktop.
//!
//! Stored in `~/.config/gtk-3.0/bookmarks` — the file Nautilus, Thunar and
//! every GTK Open/Save dialog already read — so a folder pinned in Gust
//! shows up in the browser's upload dialog and vice versa.

use gust_core::bookmarks::{add, parse, remove, render, Bookmark};
use std::path::PathBuf;

fn bm(path: &str, label: Option<&str>) -> Bookmark {
    Bookmark {
        path: PathBuf::from(path),
        label: label.map(String::from),
        uri: None,
    }
}

#[test]
fn the_gtk_format_is_read() {
    let text = "file:///home/u/Downloads Downloads\nfile:///home/u/My%20Projects\n\nfile:///home/u/a%23b Work stuff\n";
    let got = parse(text);
    assert_eq!(got.len(), 3);
    assert_eq!(got[0].path, PathBuf::from("/home/u/Downloads"));
    assert_eq!(got[0].label.as_deref(), Some("Downloads"));
    assert_eq!(got[1].path, PathBuf::from("/home/u/My Projects"));
    assert_eq!(got[1].label, None);
    assert_eq!(got[2].path, PathBuf::from("/home/u/a#b"));
    assert_eq!(got[2].label.as_deref(), Some("Work stuff"));
}

#[test]
fn other_apps_entries_survive_a_round_trip_untouched() {
    // Nautilus stores sftp:// and smb:// bookmarks here too. Gust cannot
    // open them, but must never delete them when it saves.
    let text = "sftp://nas/share Backups\nfile:///home/u/Docs\nsmb://srv/x\n";
    let list = parse(text);
    assert_eq!(render(&list), text);
    let names: Vec<_> = list
        .iter()
        .filter(|b| b.uri.is_none())
        .map(|b| &b.path)
        .collect();
    assert_eq!(names, vec![&PathBuf::from("/home/u/Docs")]);
}

#[test]
fn adding_appends_once_and_removing_removes_only_that() {
    let mut list = parse("file:///a A\nsftp://h/x\n");
    add(&mut list, PathBuf::from("/b c"));
    add(&mut list, PathBuf::from("/b c")); // pinned twice: kept once
    assert_eq!(render(&list), "file:///a A\nsftp://h/x\nfile:///b%20c\n");
    remove(&mut list, &PathBuf::from("/a"));
    assert_eq!(render(&list), "sftp://h/x\nfile:///b%20c\n");
}

#[test]
fn a_label_is_shown_else_the_folder_name() {
    assert_eq!(bm("/home/u/Downloads", Some("DL")).display(), "DL");
    assert_eq!(bm("/home/u/My Projects", None).display(), "My Projects");
    assert_eq!(bm("/", None).display(), "/");
}

#[test]
fn saving_is_atomic_and_keeps_a_missing_file_missing_until_needed() {
    let d = std::env::temp_dir().join(format!("gust-bm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let f = d.join("gtk-3.0/bookmarks");
    assert!(gust_core::bookmarks::load(&f).is_empty());
    let mut list = Vec::new();
    add(&mut list, PathBuf::from("/x"));
    gust_core::bookmarks::save(&f, &list).unwrap();
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "file:///x\n");
    // No temporary file left beside it.
    let n = std::fs::read_dir(f.parent().unwrap()).unwrap().count();
    assert_eq!(n, 1);
}

#[test]
fn a_name_that_is_not_valid_utf8_survives_as_bytes() {
    // Linux names are bytes. Old camera cards and archives from other
    // systems produce Latin-1 names; a lossy conversion turned them into
    // U+FFFD, and the copied or bookmarked path then named no file at all.
    use std::os::unix::ffi::OsStrExt;
    let raw = std::ffi::OsStr::from_bytes(b"/tmp/caf\xe9 photos");
    let p = PathBuf::from(raw);
    let uri = gust_core::clipboard::path_to_uri(&p);
    assert_eq!(uri, "file:///tmp/caf%E9%20photos");
    assert_eq!(gust_core::clipboard::uri_to_path(&uri), Some(p.clone()));
    let mut list = Vec::new();
    add(&mut list, p.clone());
    assert_eq!(parse(&render(&list))[0].path, p);
}
