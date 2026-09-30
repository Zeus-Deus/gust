//! The file-chooser portal: other apps' Open and Save dialogs answered by
//! Gust. These are the pure parts — what a request asks for, and turning
//! what the user picked into the portal's answer.

use gust_core::chooser::{answer_uris, parse_result, write_result, Mode, Request};
use std::path::PathBuf;

#[test]
fn an_open_request_with_a_folder_and_multiple() {
    let r = Request::new(
        "open",
        false,
        true,
        Some(b"/home/u/Downloads\0".to_vec()),
        None,
    );
    assert_eq!(r.mode, Mode::OpenMany);
    assert_eq!(
        r.folder.as_deref(),
        Some(std::path::Path::new("/home/u/Downloads"))
    );
}

#[test]
fn a_directory_request_selects_folders() {
    let r = Request::new("open", true, false, None, None);
    assert_eq!(r.mode, Mode::OpenFolder);
}

#[test]
fn a_save_request_keeps_the_suggested_name_but_not_a_path_in_it() {
    let r = Request::new("save", false, false, None, Some("report.pdf".into()));
    assert_eq!(r.mode, Mode::Save);
    assert_eq!(r.name.as_deref(), Some("report.pdf"));
    // An app cannot smuggle a directory through the name.
    let r = Request::new("save", false, false, None, Some("../../.bashrc".into()));
    assert_eq!(r.name.as_deref(), Some(".bashrc"));
}

#[test]
fn a_current_folder_that_is_not_absolute_is_ignored() {
    let r = Request::new("open", false, false, Some(b"relative/dir\0".to_vec()), None);
    assert_eq!(r.folder, None);
}

#[test]
fn the_result_round_trips_any_bytes() {
    use std::os::unix::ffi::OsStrExt;
    let d = std::env::temp_dir().join(format!("gust-chooser-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let out = d.join("result");
    let picked = vec![
        PathBuf::from("/tmp/a b.txt"),
        PathBuf::from(std::ffi::OsStr::from_bytes(b"/tmp/caf\xe9\nnewline")),
    ];
    write_result(&out, &picked).unwrap();
    assert_eq!(parse_result(&out), picked);
    assert_eq!(
        answer_uris(&picked),
        vec![
            "file:///tmp/a%20b.txt".to_string(),
            "file:///tmp/caf%E9%0Anewline".to_string()
        ]
    );
}

#[test]
fn no_result_file_means_cancelled() {
    let missing = std::env::temp_dir().join("gust-chooser-does-not-exist");
    assert!(parse_result(&missing).is_empty());
}

#[test]
fn relative_or_empty_picks_are_dropped() {
    let d = std::env::temp_dir().join(format!("gust-chooser-rel-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let out = d.join("result");
    std::fs::write(&out, b"relative\0\0/ok\0").unwrap();
    assert_eq!(parse_result(&out), vec![PathBuf::from("/ok")]);
}
