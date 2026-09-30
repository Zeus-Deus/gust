//! Live rclone tests against a real mount.
//!
//! These do not run against a cloud account: `tests/rclone-fixture.sh`
//! publishes a local directory over WebDAV and mounts it back with a VFS
//! write cache, which reproduces the one behaviour that matters here — a
//! write returns before the bytes reach the backend.
//!
//! Run with the fixture up:
//!   ./tests/rclone-fixture.sh start
//!   cargo test -p gust-remote --test rclone_live -- --test-threads=1

use gust_remote::cloud::*;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn root() -> PathBuf {
    PathBuf::from(
        std::env::var("GUST_RCLONE_FIXTURE").unwrap_or_else(|_| "/tmp/gust-rclone-fixture".into()),
    )
}
fn config() -> PathBuf {
    root().join("rclone.conf")
}
fn mount() -> PathBuf {
    root().join("mnt")
}
fn backend() -> PathBuf {
    root().join("backend")
}
fn rc_addr() -> String {
    let port: u16 = std::env::var("GUST_RCLONE_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8188);
    format!("http://127.0.0.1:{}", port + 1)
}

/// Skip rather than fail when the fixture is not running: these tests need a
/// live mount, and a missing fixture is a setup fact, not a defect.
fn fixture_up() -> bool {
    mount().join(".").exists() && config().exists()
}

macro_rules! need_fixture {
    () => {
        gust_testkit::skip_unless!(
            fixture_up(),
            "RCLONE",
            "rclone fixture not running (tests/rclone-fixture.sh start)"
        );
    };
}

#[test]
fn rclone_is_here() {
    gust_testkit::skip_unless!(
        gust_testkit::have("rclone"),
        "RCLONE",
        "rclone is not installed"
    );
    assert!(
        rclone_available(),
        "these tests require the pinned rclone to be installed"
    );
}

#[test]
fn listing_remotes_finds_the_fixture() {
    need_fixture!();
    let remotes = list_remotes(Some(&config())).expect("listremotes failed");
    assert!(
        remotes.iter().any(|r| r.name == "gustcloud"),
        "expected the fixture remote, got {remotes:?}"
    );
    let r = remotes.iter().find(|r| r.name == "gustcloud").unwrap();
    assert_eq!(
        r.backend, "webdav",
        "the backend type must come from rclone"
    );
}

#[test]
fn listing_remotes_opens_no_socket() {
    need_fixture!();
    // Discovery reads a file. If listremotes were connecting, pointing it at
    // a config whose URL cannot be reached would still succeed — which is
    // the assertion: parsing is independent of reachability.
    let dir = std::env::temp_dir().join("gust-cloud-noconnect");
    std::fs::create_dir_all(&dir).unwrap();
    let conf = dir.join("rclone.conf");
    std::fs::write(
        &conf,
        "[unreachable]\ntype = webdav\nurl = http://127.0.0.1:1\nvendor = other\n",
    )
    .unwrap();

    let remotes = list_remotes(Some(&conf)).expect("listing a dead remote must still work");
    assert_eq!(remotes.len(), 1);
    assert_eq!(remotes[0].name, "unreachable");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_missing_remote_says_which_one() {
    need_fixture!();
    let err = backend_listing("definitely-not-configured", "", Some(&config()))
        .expect_err("a missing remote must be refused");
    assert!(
        err.message().contains("definitely-not-configured"),
        "the message must name the remote: {}",
        err.message()
    );
}

/// The rule the plan calls non-negotiable, proven end to end against a real
/// VFS cache: a file exists in the mount while the backend does not have it.
#[test]
fn cached_is_not_uploaded() {
    need_fixture!();
    let name = "cached-not-uploaded.txt";
    let body = b"this has not reached the backend yet";
    let file = mount().join(name);
    let _ = std::fs::remove_file(&file);

    std::fs::write(&file, body).expect("write into the mount");

    // Immediately: the mount shows it...
    assert!(file.exists(), "the file must be visible in the mount");

    // ...and the backend does not.
    let on_backend = backend_listing("gustcloud", "", Some(&config())).expect("backend listing");
    assert!(
        !confirmed_on_backend(&on_backend, name, body.len() as u64),
        "the backend already has {name} — the write-back window was missed, \
         raise GUST_RCLONE_WRITEBACK and re-run"
    );

    // And rclone agrees there is unfinished work.
    let pending = vfs_pending(&rc_addr(), Duration::from_secs(5)).expect("vfs/stats");
    assert!(
        pending.has_pending(),
        "rclone reported nothing pending while an upload was outstanding"
    );

    // After the write-back interval, the backend really does have it.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut confirmed = false;
    while std::time::Instant::now() < deadline {
        let now = backend_listing("gustcloud", "", Some(&config())).expect("backend listing");
        if confirmed_on_backend(&now, name, body.len() as u64) {
            confirmed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    assert!(confirmed, "the upload never reached the backend");

    std::fs::remove_file(&file).ok();
}

/// An empty queue reads identically before anything is written and after
/// everything is uploaded. This is why confirmation must come from the
/// backend, and the test that stops anyone "simplifying" that away.
#[test]
fn an_empty_queue_proves_nothing() {
    need_fixture!();
    // Let any earlier work drain.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        let p = vfs_pending(&rc_addr(), Duration::from_secs(5)).expect("vfs/stats");
        if !p.has_pending() {
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }

    let quiet = vfs_pending(&rc_addr(), Duration::from_secs(5)).expect("vfs/stats");
    assert!(!quiet.has_pending(), "the queue should be drained by now");

    // A file that was never written to this mount at all. The queue says
    // exactly the same thing about it as about a fully uploaded file.
    let listing = backend_listing("gustcloud", "", Some(&config())).expect("backend listing");
    assert!(
        !confirmed_on_backend(&listing, "never-existed.txt", 10),
        "a file nobody wrote cannot be on the backend"
    );
    // ...yet the queue is empty either way. has_pending() is therefore not a
    // receipt for any individual file, only a statement about the queue.
    assert!(!quiet.has_pending());
}

/// A failed upload must not read as uploaded. The backend is taken away
/// underneath the mount, so the upload cannot complete.
#[test]
fn an_upload_that_cannot_finish_is_never_confirmed() {
    need_fixture!();
    let name = "doomed.txt";
    let body = b"this upload will not succeed";
    let file = mount().join(name);
    let _ = std::fs::remove_file(&file);

    // Make the backend directory unwritable so the WebDAV server cannot
    // store the object. The mount still accepts the write into its cache.
    let b = backend();
    let orig = std::fs::metadata(&b).unwrap().permissions();
    let mut ro = orig.clone();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ro.set_mode(0o500);
    }
    std::fs::set_permissions(&b, ro).unwrap();

    let wrote = std::fs::write(&file, body);
    std::thread::sleep(Duration::from_secs(8));

    // Whatever happened locally, the backend must not claim to hold it.
    let restored = {
        std::fs::set_permissions(&b, orig).unwrap();
        backend_listing("gustcloud", "", Some(&config()))
    };
    if let Ok(entries) = restored {
        assert!(
            !confirmed_on_backend(&entries, name, body.len() as u64),
            "a file that could not be uploaded was reported as on the backend"
        );
    }
    // The local write may itself have failed, which is also honest.
    let _ = wrote;
    std::fs::remove_file(&file).ok();
}

#[test]
fn a_path_outside_the_mount_is_refused() {
    need_fixture!();
    let outside = Path::new("/etc/hostname");
    assert!(relative_to_mount(&mount(), outside).is_err());
    let inside = mount().join("sub/thing.txt");
    assert_eq!(
        relative_to_mount(&mount(), &inside).unwrap(),
        PathBuf::from("sub/thing.txt")
    );
}

/// No secret may ever appear in a command line, because anyone on the
/// machine can read `/proc/*/cmdline`.
#[test]
fn no_credential_reaches_a_command_line() {
    need_fixture!();
    let ps = std::process::Command::new("ps")
        .args(["-eo", "args"])
        .output()
        .expect("ps");
    let text = String::from_utf8_lossy(&ps.stdout);
    for line in text.lines().filter(|l| l.contains("rclone")) {
        for bad in [
            "--password",
            "--pass=",
            "RCLONE_CONFIG_PASS=",
            "--client-secret",
            "token=",
        ] {
            assert!(
                !line.contains(bad),
                "a credential appeared in a command line: {bad}"
            );
        }
    }
}
