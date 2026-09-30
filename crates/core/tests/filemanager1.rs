//! `org.freedesktop.FileManager1`: how "Show in folder" from a browser,
//! a download notification or an editor reaches a file manager.
//!
//! The pure part: a list of URIs becomes the windows to open and the item
//! to select in each.

use gust_core::filemanager1::{claim_service, plan, release_service, service_text, Open};
use std::path::PathBuf;

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

#[test]
fn show_items_opens_the_parent_and_selects_the_file() {
    let got = plan(
        &["file:///home/u/Downloads/report%20final.pdf".into()],
        true,
    );
    assert_eq!(
        got,
        vec![Open {
            folder: p("/home/u/Downloads"),
            select: Some(p("report final.pdf"))
        }]
    );
}

#[test]
fn show_folders_opens_the_folder_itself() {
    let got = plan(&["file:///home/u/Music/".into()], false);
    assert_eq!(
        got,
        vec![Open {
            folder: p("/home/u/Music"),
            select: None
        }]
    );
}

#[test]
fn several_items_in_one_folder_open_one_window() {
    let got = plan(
        &[
            "file:///d/a.txt".into(),
            "file:///d/b.txt".into(),
            "file:///e/c.txt".into(),
        ],
        true,
    );
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0].folder, p("/d"));
    assert_eq!(got[1].folder, p("/e"));
}

#[test]
fn remote_and_malformed_uris_are_ignored() {
    let got = plan(
        &[
            "https://evil.example/x".into(),
            "smb://srv/share/x".into(),
            "file://host/x".into(),
            "".into(),
        ],
        true,
    );
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn a_relative_or_dot_dot_path_cannot_escape_where_it_says() {
    // file:///a/b/../../etc/passwd names /etc/passwd; the window opens there
    // honestly (lexically normalised), not somewhere it did not say.
    let got = plan(&["file:///a/b/../../etc/passwd".into()], true);
    assert_eq!(
        got,
        vec![Open {
            folder: p("/etc"),
            select: Some(p("passwd"))
        }]
    );
}

#[test]
fn a_huge_request_is_bounded() {
    let many: Vec<String> = (0..500).map(|i| format!("file:///d{i}/x")).collect();
    assert!(
        plan(&many, true).len() <= 8,
        "one call must not open hundreds of windows"
    );
}

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("gust-fm1-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn claiming_writes_a_user_activation_file_that_runs_gust() {
    let d = dir("claim");
    claim_service(&d, &p("/usr/bin/gust")).unwrap();
    let f = d.join("org.freedesktop.FileManager1.service");
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        service_text(&p("/usr/bin/gust"))
    );
    assert!(service_text(&p("/usr/bin/gust")).contains("Exec=/usr/bin/gust --dbus-service"));
}

#[test]
fn a_file_someone_else_wrote_is_never_replaced_or_removed() {
    let d = dir("foreign");
    std::fs::create_dir_all(&d).unwrap();
    let f = d.join("org.freedesktop.FileManager1.service");
    let theirs =
        "[D-BUS Service]\nName=org.freedesktop.FileManager1\nExec=/usr/bin/thunar --daemon\n";
    std::fs::write(&f, theirs).unwrap();
    assert!(claim_service(&d, &p("/usr/bin/gust")).is_err());
    release_service(&d).unwrap();
    assert_eq!(std::fs::read_to_string(&f).unwrap(), theirs);
}

#[test]
fn releasing_removes_only_our_file() {
    let d = dir("release");
    claim_service(&d, &p("/usr/bin/gust")).unwrap();
    release_service(&d).unwrap();
    assert!(!d.join("org.freedesktop.FileManager1.service").exists());
    // Releasing again is fine.
    release_service(&d).unwrap();
}

#[test]
fn a_path_with_spaces_is_quoted_for_the_desktop_exec_rules() {
    assert!(
        service_text(&p("/opt/my apps/gust")).contains("Exec=\"/opt/my apps/gust\" --dbus-service")
    );
}
