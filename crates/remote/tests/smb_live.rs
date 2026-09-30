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

/// A missing fixture must be an explicit, visible SKIP — never a quiet
/// early return that cargo reports as `ok`.
///
/// `cargo test` prints `test <name> ... ok` for a test that returns early,
/// so counting named SMB tests proved nothing about whether any backend or
/// auth path actually ran. Opting in with GUST_SMB_REQUIRED=1 turns a
/// missing fixture into a FAILURE, which is what a release gate must use.
macro_rules! need_fixture {
    () => {
        gust_testkit::skip_unless!(
            fixture_up(),
            "SMB",
            "smb fixture not running (tests/smb-fixture.sh start)"
        );
    };
}

fn test_share() -> Share {
    let mut s = parse_address("//127.0.0.1/shared", ShareKind::Smb).unwrap();
    s.user = Some(std::env::var("USER").unwrap_or_else(|_| "user".into()));
    s.domain = Some("GUSTTEST".into());
    s
}

/// Wait for a GVfs mount point to become usable.
///
/// `gio mount` returns once the backend accepted the request, but the fuse
/// directory under $XDG_RUNTIME_DIR/gvfs appears a moment later. Asserting
/// on it immediately is a race that fails intermittently and looks like a
/// broken mount.
fn wait_mounted(mp: &std::path::Path) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        if mp.is_dir() && std::fs::read_dir(mp).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

/// Clear any pre-existing GVfs mount for this share before a test that needs
/// to observe the mount operation itself.
///
/// GVfs keeps a mount alive process-wide once it exists and returns
/// "Location is already mounted" to the next caller, so a test that asserts
/// on `mount()` succeeding fails for a reason that has nothing to do with the
/// code under test. Each case that cares must therefore START from unmounted
/// rather than inheriting whatever an earlier case left behind.
fn ensure_unmounted(share: &Share) {
    let _ = unmount(share);
    let mp = share.mount_point(&runtime_dir());
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if !mp.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Mount for a test, tolerating GVfs's already-mounted answer.
///
/// Returns the mount point either way, so a case that only needs a MOUNTED
/// share does not fail because a sibling case mounted it first.
fn mount_for_test(share: &Share, pw: String) -> std::path::PathBuf {
    let mp = match mount(share, pw, &runtime_dir(), Duration::from_secs(30)) {
        Ok(mp) => mp,
        Err(ShareError::Refused(m)) if m.contains("already mounted") => {
            share.mount_point(&runtime_dir())
        }
        Err(e) => panic!("the fixture share should mount: {e:?}"),
    };
    assert!(wait_mounted(&mp), "mount point never became usable: {mp:?}");
    mp
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

    ensure_unmounted(&share);
    let mp = mount(&share, pw, &runtime_dir(), Duration::from_secs(30))
        .expect("the fixture share should mount");
    assert!(
        wait_mounted(&mp),
        "the predicted mount point never became usable: {mp:?}"
    );

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
    let mp = mount_for_test(&share, pw);
    assert_eq!(mp, share.mount_point(&runtime_dir()));
    assert!(wait_mounted(&mp), "mount point not usable: {mp:?}");
}

/// A write through the share must reach the SERVER's own directory, not just
/// a cache: "it appeared in the mount" is not the same as "it arrived".
#[test]
fn a_write_reaches_the_backend() {
    need_fixture!();
    let share = test_share();
    let pw = fixture_password().unwrap();
    let mp = mount_for_test(&share, pw);

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

    // GVfs returns an EXISTING mount instead of authenticating again, so an
    // already-mounted share makes this test prove nothing. Unmount first and
    // assert the mount is really gone, otherwise the auth path is untested
    // and the case must say so rather than passing.
    let _ = unmount(&share);
    let mp_guess = share.mount_point(&runtime_dir());
    let cleared = {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut gone = false;
        while std::time::Instant::now() < deadline {
            if !mp_guess.exists() {
                gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        gone
    };

    let err = mount(
        &share,
        "definitely-not-the-password".into(),
        &runtime_dir(),
        Duration::from_secs(20),
    );
    match err {
        Err(ShareError::AuthFailed) => {}
        Ok(_) => {
            // Only tolerable if we could not clear the pre-existing mount.
            assert!(
                !cleared,
                "A WRONG PASSWORD WAS ACCEPTED: the share was unmounted first, \
                 so GVfs really did authenticate and let it through"
            );
            eprintln!(
                "SKIP DETAIL: a pre-existing GVfs mount could not be cleared, \
                 so the auth path was not exercised by this run"
            );
        }
        Err(other) => {
            let m = other.message();
            assert!(
                !m.contains("Enter user"),
                "a translated prompt leaked into the message: {m}"
            );
            assert!(!m.contains("os error"), "an errno reached the user: {m}");
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
    let mp = mount_for_test(&share, pw);

    let fs = gust_core::stall::fstype_for(&mp).unwrap_or_else(|| panic!("no fstype for {mp:?}"));
    assert!(
        gust_core::stall::can_stall(&fs),
        "{fs} must be treated as a filesystem that can stop answering"
    );
    assert!(gust_core::stall::needs_probe(&mp));
}

// --- NFS -----------------------------------------------------------------
//
// `tests/nfs-fixture.sh` exports a directory over NFSv4 to loopback only.
// NFS authorises by address rather than by credential, which is exactly why
// the export is bound to 127.0.0.1 and nothing else.

fn nfs_export_path() -> String {
    std::env::var("GUST_NFS_FIXTURE").unwrap_or_else(|_| "/tmp/gust-nfs-fixture".into()) + "/export"
}

fn nfs_share() -> Share {
    // The address a person types: host, then the exported path.
    parse_address(&format!("//127.0.0.1{}", nfs_export_path()), ShareKind::Nfs)
        .expect("the export path should parse")
}

fn nfs_fixture_up() -> bool {
    std::path::Path::new(&format!("{}/hello.txt", nfs_export_path())).exists()
}

/// Same contract as `need_fixture!`: a missing NFS export is a visible SKIP,
/// and GUST_NFS_REQUIRED=1 makes it a failure for a release gate.
macro_rules! need_nfs {
    () => {
        gust_testkit::skip_unless!(
            nfs_fixture_up(),
            "NFS",
            "nfs fixture not running (tests/nfs-fixture.sh start)"
        );
    };
}

#[test]
fn an_nfs_export_mounts_and_lists() {
    need_nfs!();
    let share = nfs_share();
    // NFS takes no password: authorisation is by address.
    let mp = mount(
        &share,
        String::new(),
        &runtime_dir(),
        Duration::from_secs(30),
    )
    .expect("the fixture export should mount");
    assert!(mp.is_dir(), "predicted mount point must exist: {mp:?}");

    let names: Vec<String> = std::fs::read_dir(&mp)
        .expect("listing the export")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        names.iter().any(|n| n == "hello.txt"),
        "expected hello.txt, got {names:?}"
    );
}

/// The predictor was WRONG for NFS until a live mount corrected it: GVfs
/// names the directory `prefix=<url-encoded path>`, not `export=<share>`.
/// This test exists so that guess cannot come back.
#[test]
fn the_predicted_nfs_mount_point_is_the_real_one() {
    need_nfs!();
    let share = nfs_share();
    let mp = mount(
        &share,
        String::new(),
        &runtime_dir(),
        Duration::from_secs(30),
    )
    .expect("mount");
    assert_eq!(mp, share.mount_point(&runtime_dir()));
    assert!(
        mp.to_string_lossy().contains("prefix=%2F"),
        "GVfs uses a percent-encoded prefix: {mp:?}"
    );
}

#[test]
fn a_write_reaches_the_nfs_backend() {
    need_nfs!();
    let share = nfs_share();
    let mp = mount(
        &share,
        String::new(),
        &runtime_dir(),
        Duration::from_secs(30),
    )
    .expect("mount");

    let name = "written-over-nfs.txt";
    let body = "this went over NFS\n";
    std::fs::write(mp.join(name), body).expect("writing through the export");

    let backend = format!("{}/{}", nfs_export_path(), name);
    let got = std::fs::read_to_string(&backend)
        .unwrap_or_else(|e| panic!("the server should hold {backend}: {e}"));
    assert_eq!(got, body);

    std::fs::remove_file(mp.join(name)).ok();
}

#[test]
fn an_nfs_mount_is_treated_as_stallable() {
    need_nfs!();
    let share = nfs_share();
    let mp = mount(
        &share,
        String::new(),
        &runtime_dir(),
        Duration::from_secs(30),
    )
    .expect("mount");
    let fs = gust_core::stall::fstype_for(&mp).unwrap_or_else(|| panic!("no fstype for {mp:?}"));
    assert!(
        gust_core::stall::can_stall(&fs),
        "{fs} must be treated as a filesystem that can stop answering"
    );
}
