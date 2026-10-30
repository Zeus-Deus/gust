//! The file-chooser portal: other apps' Open and Save dialogs answered by
//! Gust. These are the pure parts — what a request asks for, and turning
//! what the user picked into the portal's answer.

use gust_core::chooser::{
    answer_uris, check_targets, files_from_env, files_to_env, filter_shows, folder_answer,
    glob_match, parse_result, plain_label, write_result, Filters, Mode, Request, SaveCheck,
};
use std::ffi::OsString;
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

#[test]
fn choosing_a_folder_never_follows_the_cursor() {
    let here = PathBuf::from("/home/u/Downloads");
    assert_eq!(folder_answer(&[], Some(here.clone())), Some(here.clone()));
    let picked = PathBuf::from("/home/u/Downloads/invoices");
    assert_eq!(
        folder_answer(std::slice::from_ref(&picked), Some(here.clone())),
        Some(picked.clone())
    );
    // Two selected folders are not one answer: the folder on screen is.
    assert_eq!(
        folder_answer(&[picked.clone(), picked], Some(here.clone())),
        Some(here)
    );
}

#[test]
fn a_save_over_a_file_is_reported_before_it_is_answered() {
    let d = std::env::temp_dir().join(format!("gust-save-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("folder")).unwrap();
    std::fs::write(d.join("taken.csv"), "x").unwrap();
    std::os::unix::fs::symlink(d.join("nowhere"), d.join("dangling")).unwrap();

    assert_eq!(
        check_targets(&[d.join("free.csv")], || true),
        SaveCheck::Clear
    );
    assert_eq!(
        check_targets(&[d.join("free.csv"), d.join("taken.csv")], || true),
        SaveCheck::Replaces(vec!["taken.csv".to_string()])
    );
    // A dangling link holds the name even though following it finds nothing.
    assert_eq!(
        check_targets(&[d.join("dangling")], || true),
        SaveCheck::Replaces(vec!["dangling".to_string()])
    );
    assert_eq!(
        check_targets(&[d.join("folder")], || true),
        SaveCheck::Folder(d.join("folder"))
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn globs_match_like_a_shell_without_caring_about_ascii_case() {
    assert!(glob_match(b"*.png", b"PHOTO.PNG"));
    assert!(glob_match(b"report-?.csv", b"report-1.csv"));
    assert!(glob_match(b"*.[ch]", b"main.c"));
    assert!(glob_match(b"*.[a-c]", b"x.B"));
    assert!(glob_match(b"[!.]*", b"visible"));
    assert!(!glob_match(b"[!.]*", b".hidden"));
    assert!(!glob_match(b"*.png", b"photo.png.txt"));
    // An unterminated class is a literal, not a crash.
    assert!(glob_match(b"[", b"["));
    assert!(!glob_match(b"[abc", b"a"));
}

#[test]
fn a_hostile_glob_finishes() {
    let glob = "*a".repeat(40);
    assert!(!glob_match(glob.as_bytes(), "a".repeat(39).as_bytes()));
}

#[test]
fn mime_rules_become_globs_and_the_list_keeps_the_apps_preference() {
    let globs2 = "# comment\n50:image/png:*.png\n50:image/jpeg:*.jpg\n50:text/plain:*.txt\n";
    let raw = vec![
        ("Images".to_string(), vec![(1, "image/*".to_string())]),
        ("Text".to_string(), vec![(1, "text/plain".to_string())]),
        ("Rust".to_string(), vec![(0, "*.rs".to_string())]),
    ];
    let wanted = ("Text".to_string(), vec![(1, "text/plain".to_string())]);
    let f = Filters::expand(raw, Some(wanted), globs2);
    assert_eq!(f.current, 1);
    assert_eq!(f.items[0].globs, vec!["*.png", "*.jpg"]);
    assert_eq!(f.items[1].globs, vec!["*.txt"]);
    assert_eq!(f.items[2].globs, vec!["*.rs"]);
    assert_eq!(Filters::from_json(&f.to_json()), f);
}

#[test]
fn a_preferred_filter_missing_from_the_list_is_added_and_chosen() {
    let wanted = ("PDF".to_string(), vec![(0, "*.pdf".to_string())]);
    let f = Filters::expand(Vec::new(), Some(wanted), "");
    assert_eq!(f.items.len(), 1);
    assert_eq!(f.current, 0);
}

#[test]
fn a_filter_without_patterns_hides_nothing() {
    assert!(filter_shows(&[], b"anything"));
    assert!(!filter_shows(&["*.png".to_string()], b"notes.txt"));
}

#[test]
fn garbage_from_the_environment_is_no_filter_at_all() {
    assert_eq!(Filters::from_json("not json"), Filters::default());
}

#[test]
fn gtk_mnemonics_do_not_reach_the_button() {
    assert_eq!(plain_label("_Open"), "Open");
    assert_eq!(plain_label("Save __as"), "Save _as");
    assert_eq!(plain_label("Add"), "Add");
}

#[test]
fn the_file_being_saved_again_sets_folder_and_name() {
    let r = Request::new("save", false, false, Some(b"/tmp\0".to_vec()), None)
        .with_current_file(Some(b"/home/u/notes/draft.md\0".to_vec()));
    assert_eq!(
        r.folder.as_deref(),
        Some(std::path::Path::new("/home/u/notes"))
    );
    assert_eq!(r.name.as_deref(), Some("draft.md"));
}

#[test]
fn save_files_keeps_bare_names_and_refuses_what_cannot_match_one_to_one() {
    let r = Request::new("save-many", false, false, None, None)
        .with_files(vec![b"a.txt\0".to_vec(), b"../../.bashrc\0".to_vec()])
        .unwrap();
    assert_eq!(r.mode, Mode::SaveMany);
    assert_eq!(r.files, vec![OsString::from("a.txt"), ".bashrc".into()]);

    let request = || Request::new("save-many", false, false, None, None);
    assert!(request().with_files(Vec::new()).is_none());
    assert!(request()
        .with_files(vec![b"a\0".to_vec(), b"..\0".to_vec()])
        .is_none());
    assert!(request()
        .with_files(vec![b"x/a\0".to_vec(), b"y/a\0".to_vec()])
        .is_none());
}

#[test]
fn two_names_that_only_differ_in_non_utf8_bytes_stay_two_names() {
    let r = Request::new("save-many", false, false, None, None)
        .with_files(vec![b"raw\xff\0".to_vec(), b"raw\xfe\0".to_vec()])
        .unwrap();
    assert_ne!(r.files[0], r.files[1]);
    assert_eq!(files_from_env(&files_to_env(&r.files)), r.files);
}

#[test]
fn a_name_that_is_not_utf8_is_not_offered_changed() {
    let r = Request::new("save", false, false, None, None)
        .with_current_file(Some(b"/home/u/raw\xff\0".to_vec()));
    assert_eq!(r.folder.as_deref(), Some(std::path::Path::new("/home/u")));
    assert_eq!(r.name, None);
}

#[test]
fn the_preferred_filter_is_found_by_what_it_does_not_only_its_label() {
    let png = ("Files".to_string(), vec![(0, "*.png".to_string())]);
    let pdf = ("Files".to_string(), vec![(0, "*.pdf".to_string())]);
    let f = Filters::expand(vec![png, pdf.clone()], Some(pdf), "");
    assert_eq!(f.current, 1);
}

#[test]
fn a_hostile_filter_is_cut_down_to_a_bounded_amount_of_work() {
    let long = format!("*[{}]", "a".repeat(60_000));
    let many = (0..4096).map(|i| (0, format!("*.{i:060}"))).collect();
    let raw = vec![
        ("Long".to_string(), vec![(0, long)]),
        ("Many".to_string(), many),
    ];
    let f = Filters::expand(raw, None, "");
    assert!(f.items[0].globs.is_empty());
    let bytes: usize = f.items[1].globs.iter().map(String::len).sum();
    assert!(bytes <= 8 * 1024, "{bytes}");
}

#[test]
fn the_filter_list_always_fits_one_environment_string() {
    let raw = (0..64)
        .map(|i| {
            let rules = (0..512).map(|n| (0, format!("*.{i}{n:060}"))).collect();
            (format!("Filter {i}"), rules)
        })
        .collect();
    let f = Filters::expand(raw, None, "");
    assert!(f.to_json().len() < 128 * 1024);
    assert!(!f.items.is_empty());
}

#[test]
fn a_check_nobody_is_waiting_for_stops_at_the_next_target() {
    let check = check_targets(&[PathBuf::from("/")], || false);
    assert!(matches!(check, SaveCheck::Failed(_)));
}

#[test]
fn a_name_no_filesystem_can_hold_refuses_the_whole_call() {
    let request = || Request::new("save-many", false, false, None, None);
    assert!(request().with_files(vec![vec![b'a'; 256]]).is_none());
    assert!(request().with_files(vec![vec![b'a'; 255]]).is_some());
    assert!(request().with_files(vec![b"a\0b\0".to_vec()]).is_none());
}

#[test]
fn one_filter_with_a_huge_name_still_fits_one_environment_string() {
    let raw = vec![("x".repeat(140_000), vec![(0, "*.png".to_string())])];
    let f = Filters::expand(raw, None, "");
    assert!(f.to_json().len() < 128 * 1024);
    assert_eq!(f.items.len(), 1);
}

#[test]
fn the_preferred_filter_survives_being_cut_down() {
    let big = |i: usize| {
        let rules = (0..512).map(|n| (0, format!("*.{i}{n:060}"))).collect();
        (format!("f{i}"), rules)
    };
    let raw: Vec<_> = (0..64).map(big).collect();
    let wanted = ("PDF".to_string(), vec![(0, "*.pdf".to_string())]);
    let f = Filters::expand(raw, Some(wanted), "");
    assert_eq!(f.items[f.current].name, "PDF");
    assert!(f.items.len() <= 64);
    assert!(f.to_json().len() < 128 * 1024);
}

#[test]
fn the_preferred_filter_keeps_its_place_when_others_are_cut() {
    let big = |i: usize| {
        let rules = (0..512).map(|n| (0, format!("*.{i}{n:060}"))).collect();
        (format!("f{i}"), rules)
    };
    let raw: Vec<_> = (0..64).map(big).collect();
    let wanted = big(2);
    let f = Filters::expand(raw, Some(wanted), "");
    assert_eq!(f.items[f.current].name, "f2");
}
