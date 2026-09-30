//! Folder sizes, computed in the background.
//!
//! A folder's size is what it would take to copy it: the apparent size of
//! every regular file under it, each hard-linked file counted once. The walk
//! must be safe on anything a user's disk can hold.

use gust_core::dirsize::{dir_size, SizeLimits, Sized};
use gust_core::lister::CancelToken;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("gust-dirsize-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn size(d: &Path) -> Sized {
    dir_size(d, &CancelToken::new(), SizeLimits::default())
}

#[test]
fn nested_files_are_added_up() {
    let d = tmp("nested");
    fs::create_dir_all(d.join("a/b/c")).unwrap();
    fs::write(d.join("x"), vec![0u8; 1000]).unwrap();
    fs::write(d.join("a/y"), vec![0u8; 20]).unwrap();
    fs::write(d.join("a/b/c/z"), vec![0u8; 3]).unwrap();
    let s = size(&d);
    assert_eq!(s.bytes, 1023);
    assert_eq!(s.files, 3);
    assert!(s.complete);
}

#[test]
fn symlinks_are_not_followed() {
    // A link to / or to a parent would make the size meaningless or endless.
    let d = tmp("links");
    fs::write(d.join("real"), vec![0u8; 10]).unwrap();
    symlink("/", d.join("root")).unwrap();
    symlink(&d, d.join("loop")).unwrap();
    symlink(d.join("real"), d.join("alias")).unwrap();
    let s = size(&d);
    assert_eq!(s.bytes, 10);
    assert!(s.complete);
}

#[test]
fn a_hard_linked_file_counts_once() {
    let d = tmp("hard");
    fs::write(d.join("one"), vec![0u8; 500]).unwrap();
    fs::hard_link(d.join("one"), d.join("two")).unwrap();
    assert_eq!(size(&d).bytes, 500);
}

#[test]
fn another_filesystem_mounted_inside_is_not_entered() {
    // /proc under a chroot, a network share under ~/mnt: its size is not the
    // folder's, and walking it can hang. Checked with the real /proc, whose
    // device differs from any temp directory's.
    let s = dir_size(
        std::path::Path::new("/"),
        &CancelToken::new(),
        SizeLimits { max_entries: 2_000 },
    );
    // Not a size assertion: the walk must simply not wander into /proc,
    // where file sizes are fictional (kcore is terabytes).
    assert!(s.bytes < 1 << 50, "walked into /proc: {} bytes", s.bytes);
}

#[test]
fn an_unreadable_subfolder_makes_the_answer_a_lower_bound() {
    let d = tmp("denied");
    fs::create_dir_all(d.join("locked")).unwrap();
    fs::write(d.join("locked/secret"), vec![0u8; 99]).unwrap();
    fs::write(d.join("open"), vec![0u8; 1]).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(d.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let s = size(&d);
    fs::set_permissions(d.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
    if s.bytes == 100 {
        // Running as root: permissions do not apply. Nothing to check.
        return;
    }
    assert_eq!(s.bytes, 1);
    assert!(
        !s.complete,
        "a size that skipped a folder must not claim to be exact"
    );
}

#[test]
fn a_huge_tree_stops_at_the_limit_and_says_so() {
    let d = tmp("huge");
    for i in 0..300 {
        fs::write(d.join(format!("f{i}")), b"x").unwrap();
    }
    let s = dir_size(&d, &CancelToken::new(), SizeLimits { max_entries: 100 });
    assert!(!s.complete);
    assert!(s.files <= 100);
}

#[test]
fn cancelling_stops_promptly() {
    let d = tmp("cancel");
    for i in 0..50 {
        fs::create_dir_all(d.join(format!("d{i}"))).unwrap();
        for j in 0..50 {
            fs::write(d.join(format!("d{i}/f{j}")), b"x").unwrap();
        }
    }
    let c = CancelToken::new();
    c.cancel();
    let t = Instant::now();
    let s = dir_size(&d, &c, SizeLimits::default());
    assert!(t.elapsed() < Duration::from_millis(50));
    assert!(!s.complete);
}

#[test]
fn an_empty_folder_is_zero_and_exact() {
    let d = tmp("empty");
    let s = size(&d);
    assert_eq!((s.bytes, s.files, s.complete), (0, 0, true));
}
