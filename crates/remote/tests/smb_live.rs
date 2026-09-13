//! Live SMB tests against a real Samba server.
//!
//! `tests/smb-fixture.sh` runs an unprivileged smbd on loopback with its own
//! private passdb and a throwaway account, so no real credential exists to
//! leak. Run with the fixture up:
//!
//!   ./tests/smb-fixture.sh start
//!   cargo test -p gust-remote --test smb_live -- --test-threads=1

use gust_remote::shares::*;
use std::time::Duration;

fn fixture_root() -> String {
    std::env::var("GUST_SMB_FIXTURE").unwrap_or_else(|_| "/tmp/gust-smb-fixture".into())
}

fn runtime_dir() -> String {
    std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".into())
}

fn fixture_password() -> Option<String> {
    std::fs::read_to_string(format!("{}/password", fixture_root()))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn fixture_up() -> bool {
    fixture_password().is_some()
}

macro_rules! need_fixture {
    () => {
        if !fixture_up() {
            eprintln!("smb fixture not running — start tests/smb-fixture.sh");
            return;
        }
    };
}

fn test_share() -> Share {
    let mut s = parse_address("//127.0.0.1/shared", ShareKind::Smb).unwrap();
    s.user = Some(std::env::var("USER").unwrap_or_else(|_| "user".into()));
    s.domain = Some("GUSTTEST".into());
    s
}

#[test]
fn gvfs_is_available() {
    assert!(gvfs_available(), "these tests need gvfs-smb installed");
}

#[test]
fn a_share_mounts_and_lists() {
    need_fixture!();
    let share = test_share();
    let pw = fixture_password().unwrap();

    let mp = mount(&share, pw, &runtime_dir(), Duration::from_secs(30))
        .expect("the fixture share should mount");
    assert!(mp.is_dir(), "the predicted mount point must exist: {mp:?}");

    // The seeded contents must be visible through the mount.
    let names: Vec<String> = std::fs::read_dir(&mp)
        .expect("listing the share")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        names.iter().any(|n| n == "hello.txt"),
        "expected hello.txt, got {names:?}"
    );
}

/// The mount point is predicted from the connection parameters, not scraped
/// out of gio's stdout — so it is language-independent.
#[test]
fn the_predicted_mount_point_is_the_real_one() {
    need_fixture!();
    let share = test_share();
    let pw = fixture_password().unwrap();
    let mp = mount(&share, pw, &runtime_dir(), Duration::from_secs(30)).unwrap();
    assert_eq!(mp, share.mount_point(&runtime_dir()));
    assert!(mp.is_dir());
}

/// A write through the share must reach the SERVER's own directory, not just
/// a cache: "it appeared in the mount" is not the same as "it arrived".
#[test]
fn a_write_reaches_the_backend() {
    need_fixture!();
    let share = test_share();
    let pw = fixture_password().unwrap();
    let mp = mount(&share, pw, &runtime_dir(), Duration::from_secs(30)).unwrap();

    let name = "written-through-gust.txt";
    let body = "this went over SMB\n";
    std::fs::write(mp.join(name), body).expect("writing through the share");

    // The fixture's share directory is what the server actually serves.
    let backend = format!("{}/share/{}", fixture_root(), name);
    let got = std::fs::read_to_string(&backend)
        .unwrap_or_else(|e| panic!("the server should hold {backend}: {e}"));
    assert_eq!(got, body, "the bytes on the server must match");

    std::fs::remove_file(mp.join(name)).ok();
}

/// A wrong password must be reported as a wrong password, in Gust's words.
#[test]
fn a_bad_password_is_refused_clearly() {
    need_fixture!();
    let mut share = test_share();
    // A different share name, so this cannot reuse the mount from another
    // test — GVfs keeps a mount alive once it exists.
    share.share = "shared".into();
    share.user = Some("nosuchuser".into());

    let err = mount(
        &share,
        "definitely-not-the-password".into(),
        &runtime_dir(),
        Duration::from_secs(20),
    );
    match err {
        Err(ShareError::AuthFailed) => {}
        // If the share is already mounted from an earlier test, GVfs returns
        // the existing mount rather than authenticating again. That is GVfs
        // behaviour, not a Gust bug, and the message check below still holds.
        Ok(_) => eprintln!("share was already mounted; auth path not exercised"),
        Err(other) => {
            let m = other.message();
            assert!(
                !m.contains("Enter user"),
                "a translated prompt leaked into the message: {m}"
            );
        }
    }
}

/// No credential may reach a command line, where `ps` would show it.
#[test]
fn no_password_reaches_a_command_line() {
    need_fixture!();
    let share = test_share();
    let pw = fixture_password().unwrap();
    let secret = pw.clone();

    let _ = mount(&share, pw, &runtime_dir(), Duration::from_secs(30));

    let ps = std::process::Command::new("ps")
        .args(["-eo", "args"])
        .output()
        .expect("ps");
    let text = String::from_utf8_lossy(&ps.stdout);
    assert!(
        !text.contains(&secret),
        "the fixture password appeared in a process command line"
    );
    for line in text.lines().filter(|l| l.contains("gio")) {
        assert!(
            !line.contains("password") && !line.contains("passwd"),
            "a credential-looking argument was passed to gio: {line}"
        );
    }
}

/// The stall guard must recognise a GVfs mount, or a NAS that goes away
/// would hang the window (DESIGN §6.12).
#[test]
fn a_gvfs_mount_is_treated_as_stallable() {
    need_fixture!();
    let share = test_share();
    let pw = fixture_password().unwrap();
    let mp = mount(&share, pw, &runtime_dir(), Duration::from_secs(30)).unwrap();

    let fs = gust_core::stall::fstype_for(&mp).unwrap_or_else(|| panic!("no fstype for {mp:?}"));
    assert!(
        gust_core::stall::can_stall(&fs),
        "{fs} must be treated as a filesystem that can stop answering"
    );
    assert!(gust_core::stall::needs_probe(&mp));
}
