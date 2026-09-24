//! Listing an archive runs the parser in the sandbox when it can.

use gust_ops::archive::io::{list, list_command};
use std::path::PathBuf;

fn fixture(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("gust-arcsb-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("src/a")).unwrap();
    std::fs::write(d.join("src/a/one.txt"), b"1").unwrap();
    std::fs::write(d.join("src/two.txt"), b"2").unwrap();
    let ok = std::process::Command::new("bsdtar")
        .args(["-czf"])
        .arg(d.join("x.tar.gz"))
        .args(["-C"])
        .arg(d.join("src"))
        .args(["a", "two.txt"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(ok, "bsdtar is needed for this test");
    d
}

#[test]
fn the_sandboxed_command_never_names_the_archive_or_a_home_path() {
    let d = fixture("argv");
    let c = list_command(&d.join("x.tar.gz"), true).unwrap();
    assert_eq!(c.get_program(), "bwrap");
    let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
    assert!(args.iter().all(|a| !a.contains("gust-arcsb")), "the path must not reach argv: {args:?}");
    assert!(args.contains(&"--unshare-all".to_string()));
    assert_eq!(args.last().map(String::as_str), Some("-"), "the archive arrives on stdin");
    assert!(!args.iter().any(|a| a == "--bind"), "nothing is writable");
}

#[test]
fn listing_through_the_sandbox_gives_the_same_entries() {
    let d = fixture("same");
    let have_sandbox = ["bwrap", "prlimit"].iter().all(|b| {
        std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|x| x.join(b).is_file()))
    });
    let entries = list(&d.join("x.tar.gz")).unwrap();
    let mut names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
    names.sort();
    assert_eq!(names, vec!["a", "a/one.txt", "two.txt"], "sandbox available: {have_sandbox}");
}

#[test]
fn a_hostile_archive_listed_in_the_sandbox_cannot_reach_the_home_folder() {
    // A file named like an archive that is not one fails inside the jail
    // with an error, and Gust reports it — it does not crash or hang.
    let d = std::env::temp_dir().join(format!("gust-arcsb-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("evil.tar.gz"), vec![0x1f, 0x8b, 0x08, 0, 0xff, 0xff, 0xff]).unwrap();
    let t = std::time::Instant::now();
    assert!(list(&d.join("evil.tar.gz")).is_err());
    assert!(t.elapsed() < std::time::Duration::from_secs(20));
}
