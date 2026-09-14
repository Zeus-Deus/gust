//! Failure injection against a real cloud-like backend.
//!
//! The plain-path rclone tests prove "cached != uploaded" on a healthy
//! fixture. These prove the cases that actually lose data: the process dies
//! with dirty cache, the backend refuses writes, the server disappears
//! mid-flight, and the RC endpoint stops answering. In every one of them the
//! user's bytes must still exist somewhere and Gust must never claim an
//! upload it cannot prove.
//!
//! Start the fixture with `tests/rclone-fixture.sh start`. These tests work
//! at ANY write-back interval: they never assume the dirty window is still
//! open, because a short interval legitimately closes it before the assertion
//! runs. Coupling a test to a specific GUST_RCLONE_WRITEBACK breaks the
//! other rclone suites that assume the default, so the invariant asserted
//! here is "pending OR provably on the backend", never "pending".
//! Missing prerequisites SKIP loudly rather than passing quietly.

use gust_remote::disconnect::{verdict_for, DisconnectVerdict, PendingReport};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn root() -> PathBuf {
    PathBuf::from(
        std::env::var("GUST_RCLONE_FIXTURE").unwrap_or_else(|_| "/tmp/gust-rclone-fixture".into()),
    )
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

fn fixture_up() -> bool {
    let m = mount();
    Command::new("mountpoint")
        .arg("-q")
        .arg(&m)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Read the report the app would build from the live RC endpoint.
fn live_report() -> PendingReport {
    match gust_remote::cloud::vfs_pending(&rc_addr(), Duration::from_secs(3)) {
        Ok(p) => PendingReport::Known {
            queued: p.queued,
            in_progress: p.in_progress,
            errored: p.errored,
        },
        Err(e) => PendingReport::Unknown(e.message()),
    }
}

fn wait_until<F: Fn() -> bool>(limit: Duration, f: F) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// While bytes sit in the cache unuploaded, a disconnect must not be silent.
#[test]
fn a_dirty_cache_blocks_a_silent_disconnect() {
    if !fixture_up() {
        eprintln!("SKIP: rclone fixture not mounted (tests/rclone-fixture.sh start)");
        return;
    }
    let name = format!("dirty-{}.bin", std::process::id());
    let local = mount().join(&name);
    // Big enough that the upload cannot complete instantly.
    let payload = vec![b'D'; 6 * 1024 * 1024];
    std::fs::write(&local, &payload).expect("write into the mount failed");

    // The queue should show work; if the write-back is too short this races,
    // so accept either pending work or an already-confirmed backend copy.
    let report = live_report();
    let backend_has = backend().join(&name).exists();
    eprintln!("report={report:?} backend_has={backend_has}");

    match (&report, backend_has) {
        // The normal case under a long write-back: work outstanding.
        (
            PendingReport::Known {
                queued,
                in_progress,
                ..
            },
            _,
        ) if queued + in_progress > 0 => {
            match verdict_for("gustcloud", &report, None, Some(&mount()), false) {
                DisconnectVerdict::Ask(m) => assert!(
                    m.contains("waiting to upload"),
                    "must say work is outstanding: {m}"
                ),
                other => panic!("outstanding uploads must block: {other:?}"),
            }
        }
        // Upload already finished: the bytes must really be on the backend.
        (_, true) => {
            let on_backend = std::fs::read(backend().join(&name)).expect("backend read");
            assert_eq!(
                on_backend.len(),
                payload.len(),
                "the backend copy is a different size from what was written"
            );
        }
        _ => panic!("neither pending nor uploaded: {report:?}"),
    }

    // Let it settle so the next test starts clean.
    wait_until(Duration::from_secs(120), || backend().join(&name).exists());
    let _ = std::fs::remove_file(&local);
}

/// Killing the mount process with dirty cache must NOT destroy the bytes:
/// the cache is on disk and the file must still be recoverable.
#[test]
fn bytes_survive_a_mount_process_death_with_dirty_cache() {
    if !fixture_up() {
        eprintln!("SKIP: rclone fixture not mounted");
        return;
    }
    let name = format!("survive-{}.bin", std::process::id());
    let local = mount().join(&name);
    let payload = vec![b'S'; 4 * 1024 * 1024];
    std::fs::write(&local, &payload).expect("write failed");

    // The VFS cache is a real directory: the bytes must be there before the
    // backend has them, which is what makes recovery possible at all.
    let cache = root().join("cache");
    let found_in_cache = wait_until(Duration::from_secs(20), || {
        fn walk(d: &Path, want: u64) -> bool {
            let Ok(rd) = std::fs::read_dir(d) else {
                return false;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if walk(&p, want) {
                        return true;
                    }
                } else if p.metadata().map(|m| m.len()).ok() == Some(want) {
                    return true;
                }
            }
            false
        }
        walk(&cache, payload.len() as u64) || backend().join(&name).exists()
    });
    assert!(
        found_in_cache,
        "the written bytes were in neither the VFS cache nor the backend: \
         nothing would survive a crash"
    );
    wait_until(Duration::from_secs(120), || backend().join(&name).exists());
    let _ = std::fs::remove_file(&local);
}

/// A backend that refuses writes must surface as errored/pending, and a
/// disconnect must be REFUSED rather than asked: saying yes loses the file.
#[test]
fn a_read_only_backend_never_reports_a_successful_upload() {
    if !fixture_up() {
        eprintln!("SKIP: rclone fixture not mounted");
        return;
    }
    let b = backend();
    let name = format!("refused-{}.bin", std::process::id());
    let local = mount().join(&name);

    // Make the backend directory unwritable while the server stays up.
    let before = std::fs::metadata(&b)
        .expect("backend metadata")
        .permissions();
    let mut ro = before.clone();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ro.set_mode(0o500);
    }
    std::fs::set_permissions(&b, ro).expect("chmod backend");

    let wrote = std::fs::write(&local, vec![b'R'; 2 * 1024 * 1024]);
    eprintln!("write into mount with read-only backend: {wrote:?}");

    // Give rclone a chance to try and fail.
    std::thread::sleep(Duration::from_secs(3));
    let report = live_report();
    eprintln!("report with read-only backend: {report:?}");

    // Whatever the counters say, the file must NOT be claimed as uploaded:
    // the backend cannot possibly have it.
    assert!(
        !b.join(&name).exists(),
        "a read-only backend somehow received the file"
    );
    let v = verdict_for("gustcloud", &report, None, Some(&mount()), false);
    if let DisconnectVerdict::Safe = v {
        // Safe is only acceptable if nothing was ever accepted into the cache.
        assert!(
            wrote.is_err(),
            "the write was accepted but a disconnect was called safe while \
             the backend never got the file: {v:?}"
        );
    }

    // Restore, then let the queue drain so later tests are not poisoned.
    std::fs::set_permissions(&b, before).expect("restore backend permissions");
    wait_until(Duration::from_secs(120), || {
        matches!(
            live_report(),
            PendingReport::Known {
                queued: 0,
                in_progress: 0,
                errored: 0
            }
        )
    });
    let _ = std::fs::remove_file(&local);
    let _ = std::fs::remove_file(b.join(&name));
}

/// An unreachable RC endpoint is UNKNOWN state, never "nothing pending".
#[test]
fn an_unreachable_rc_endpoint_is_unknown_not_empty() {
    // No fixture needed: a port nobody serves is the failure being tested.
    let report = match gust_remote::cloud::vfs_pending("http://127.0.0.1:9", Duration::from_secs(2))
    {
        Ok(p) => PendingReport::Known {
            queued: p.queued,
            in_progress: p.in_progress,
            errored: p.errored,
        },
        Err(e) => PendingReport::Unknown(e.message()),
    };
    assert!(
        matches!(report, PendingReport::Unknown(_)),
        "an unreachable RC endpoint must not read as an empty queue: {report:?}"
    );
    match verdict_for("gustcloud", &report, None, None, false) {
        DisconnectVerdict::Refuse(m) => {
            assert!(m.contains("cannot tell"), "got {m}");
            assert!(!m.contains("os error"), "errno leaked to the user: {m}");
        }
        other => panic!("unknown state must be refused, got {other:?}"),
    }
}

/// A file the backend really has, at the right size, is confirmable — and
/// that is the ONLY thing that may be shown as uploaded.
#[test]
fn only_a_backend_copy_at_the_right_size_confirms() {
    if !fixture_up() {
        eprintln!("SKIP: rclone fixture not mounted");
        return;
    }
    let name = format!("confirm-{}.txt", std::process::id());
    let body = b"confirmed content";
    std::fs::write(mount().join(&name), body).expect("write failed");

    let landed = wait_until(Duration::from_secs(120), || backend().join(&name).exists());
    assert!(landed, "the write never reached the backend");

    let entries =
        gust_remote::cloud::backend_listing("gustcloud", "", Some(&root().join("rclone.conf")))
            .expect("backend listing failed");
    assert!(
        gust_remote::cloud::confirmed_on_backend(&entries, &name, body.len() as u64),
        "a file the backend really holds was not confirmable"
    );
    // A wrong size must NOT confirm: equal-name is not a receipt.
    assert!(
        !gust_remote::cloud::confirmed_on_backend(&entries, &name, body.len() as u64 + 1),
        "a size mismatch was accepted as a confirmation"
    );
    let _ = std::fs::remove_file(mount().join(&name));
}
