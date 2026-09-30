//! What the Columns view shows beside the current folder: the parent
//! folder's entries (so where you are is visible in context) and a bounded
//! preview of the item under the cursor — the first entries of a folder.

use gust_core::columns::{list_names_bounded, Bounded};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::time::Duration;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("gust-cols-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn folders_first_then_natural_order() {
    let d = tmp("order");
    for f in ["b.txt", "file10", "file9", "A.md"] {
        fs::write(d.join(f), b"x").unwrap();
    }
    fs::create_dir(d.join("zeta")).unwrap();
    fs::create_dir(d.join("alpha")).unwrap();
    let Bounded { names, more, .. } = list_names_bounded(&d, 100, false, Duration::from_secs(2));
    let shown: Vec<String> = names
        .iter()
        .map(|(n, _)| n.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        shown,
        vec!["alpha", "zeta", "A.md", "b.txt", "file9", "file10"]
    );
    assert_eq!(more, 0);
}

#[test]
fn a_huge_folder_is_cut_off_and_says_how_many_more() {
    let d = tmp("huge");
    for i in 0..500 {
        fs::write(d.join(format!("f{i:03}")), b"x").unwrap();
    }
    let b = list_names_bounded(&d, 50, false, Duration::from_secs(2));
    assert_eq!(b.names.len(), 50);
    assert_eq!(b.more, 450);
}

#[test]
fn hidden_entries_follow_the_setting() {
    let d = tmp("hidden");
    fs::write(d.join(".secret"), b"x").unwrap();
    fs::write(d.join("seen"), b"x").unwrap();
    assert_eq!(
        list_names_bounded(&d, 10, false, Duration::from_secs(2))
            .names
            .len(),
        1
    );
    assert_eq!(
        list_names_bounded(&d, 10, true, Duration::from_secs(2))
            .names
            .len(),
        2
    );
}

#[test]
fn a_symlinked_folder_is_marked_but_not_followed() {
    let d = tmp("link");
    symlink("/", d.join("root")).unwrap();
    let b = list_names_bounded(&d, 10, false, Duration::from_secs(2));
    assert_eq!(b.names.len(), 1);
    assert!(
        !b.names[0].1,
        "a link to a folder is not listed as a folder to walk into"
    );
}

#[test]
fn an_unreadable_folder_says_so() {
    let d = tmp("denied");
    fs::create_dir(d.join("locked")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(d.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let b = list_names_bounded(&d.join("locked"), 10, false, Duration::from_secs(2));
    fs::set_permissions(d.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
    if b.error.is_none() {
        return; // running as root
    }
    assert!(b.names.is_empty());
}
