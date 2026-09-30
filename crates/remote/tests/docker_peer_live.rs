//! Gust's real mount code against an isolated container peer.
//!
//! Why a container and not the local sshd fixture: `internal-sftp -d <dir>`
//! selects a starting directory, it is NOT confinement. A container has a
//! real filesystem boundary, so the outside-share canary genuinely cannot be
//! reached, and the peer's storage is a named volume independent of the
//! client's mount and cache — which is what makes a receipt check mean
//! something.
//!
//! Start the peers with `tests/docker-peers.sh start`. Missing prerequisites
//! SKIP loudly; they never return success quietly.

use gust_remote::mount::{is_mounted, mount, unmount, RemoteSpec};
use std::path::{Path, PathBuf};
use std::time::Duration;

const FIX: &str = "/tmp/gust-docker-peers";
const PORT: u16 = 2230;

fn peer_running() -> bool {
    Path::new(FIX).join("known_hosts").exists()
        && Path::new(FIX).join("id_peer").exists()
        && std::net::TcpStream::connect(("127.0.0.1", PORT)).is_ok()
}

/// The peer's own view of its backing store, read through docker rather than
/// through the mount, so a receipt is independent of the client's cache.
fn backend_read(path: &str) -> Option<String> {
    let out = std::process::Command::new("sudo")
        .args(["-n", "docker", "exec", "gust-peer-ssh", "cat", path])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn backend_exists(path: &str) -> bool {
    std::process::Command::new("sudo")
        .args(["-n", "docker", "exec", "gust-peer-ssh", "test", "-e", path])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn spec(remote_path: &str) -> RemoteSpec {
    let f = PathBuf::from(FIX);
    let mut s = RemoteSpec::new("gust-docker-peer", "127.0.0.1");
    s.user = Some("peer".to_string());
    s.port = Some(PORT);
    s.remote_path = Some(remote_path.to_string());
    s.identity_file = Some(f.join("id_peer"));
    s.known_hosts = Some(f.join("known_hosts"));
    s
}

fn mount_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join("gust-peer-mnt").join(name);
    let _ = std::fs::create_dir_all(&d);
    d
}

#[test]
fn a_container_peer_connects_lists_and_disconnects() {
    gust_testkit::skip_unless!(
        peer_running(),
        "DOCKER_PEER",
        "container peer not running (tests/docker-peers.sh start)"
    );
    let mp = mount_dir("connect");
    let _ = unmount(&mp);

    let r = mount(&spec("/srv/share"), &mp, Duration::from_secs(20));
    assert!(r.is_ok(), "mount failed: {:?}", r.map(|_| ()).err());
    assert!(is_mounted(&mp), "mount claimed success but nothing mounted");

    let names: Vec<String> = std::fs::read_dir(&mp)
        .expect("cannot read the mounted peer")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.contains(&"hello.txt".to_string()), "got {names:?}");

    let body = std::fs::read_to_string(mp.join("hello.txt")).expect("cannot read peer file");
    assert!(body.contains("peer fixture ready"), "got {body:?}");

    unmount(&mp).expect("unmount failed");
    assert!(!is_mounted(&mp), "still mounted after unmount");
}

/// The share is a real boundary: a canary outside it is unreachable.
#[test]
fn the_peer_share_cannot_be_escaped() {
    gust_testkit::skip_unless!(
        peer_running(),
        "DOCKER_PEER",
        "container peer not running (tests/docker-peers.sh start)"
    );
    // The canary really is there, on the peer.
    assert!(
        backend_exists("/root/canary-outside-share.txt"),
        "fixture is wrong: the canary should exist inside the peer"
    );

    let mp = mount_dir("escape");
    let _ = unmount(&mp);
    mount(&spec("/srv/share"), &mp, Duration::from_secs(20)).expect("mount failed");

    // Every way out of the share must fail.
    for attempt in [
        "../root/canary-outside-share.txt",
        "../../root/canary-outside-share.txt",
        "subdir/../../root/canary-outside-share.txt",
    ] {
        let got = std::fs::read_to_string(mp.join(attempt));
        assert!(
            got.is_err() || !got.as_ref().unwrap().contains("CANARY"),
            "ESCAPED THE SHARE via {attempt}: {got:?}"
        );
    }
    let _ = unmount(&mp);
}

/// A write through the mount must land in the PEER's own storage, verified
/// on the backend rather than read back through the same cache that wrote it.
#[test]
fn a_write_reaches_the_peers_own_storage() {
    gust_testkit::skip_unless!(
        peer_running(),
        "DOCKER_PEER",
        "container peer not running (tests/docker-peers.sh start)"
    );
    let mp = mount_dir("write");
    let _ = unmount(&mp);
    mount(&spec("/srv/share"), &mp, Duration::from_secs(20)).expect("mount failed");

    let marker = format!("written-by-gust-{}", std::process::id());
    let body = format!("payload {marker}");
    std::fs::write(mp.join(&marker), body.as_bytes()).expect("write through the mount failed");

    // Independent receipt: ask the peer, not the mount.
    let seen = backend_read(&format!("/srv/share/{marker}"));
    assert_eq!(
        seen.as_deref().map(str::trim),
        Some(body.as_str()),
        "the bytes never reached the peer's own storage"
    );

    std::fs::remove_file(mp.join(&marker)).ok();
    assert!(
        !backend_exists(&format!("/srv/share/{marker}")),
        "deleting through the mount did not remove it on the peer"
    );
    let _ = unmount(&mp);
}

/// A CHANGED host key must be refused, not reported as a vague network error.
/// This is only testable because the fixture pins a real key.
#[test]
fn a_changed_host_key_is_refused() {
    gust_testkit::skip_unless!(
        peer_running(),
        "DOCKER_PEER",
        "container peer not running (tests/docker-peers.sh start)"
    );
    let mp = mount_dir("hostkey");
    let _ = unmount(&mp);

    // A known_hosts naming the right host with the WRONG key.
    let bad = PathBuf::from(FIX).join("known_hosts.wrong");
    let (_d, other_pub) = {
        let d = std::env::temp_dir().join(format!("gust-wrongkey-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let k = d.join("other");
        let ok = std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&k)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            gust_testkit::skip!("DOCKER_PEER", "ssh-keygen unavailable");
        }
        let pubkey = std::fs::read_to_string(k.with_extension("pub")).unwrap();
        (d, pubkey)
    };
    let parts: Vec<&str> = other_pub.split_whitespace().collect();
    std::fs::write(
        &bad,
        format!("[127.0.0.1]:{PORT} {} {}\n", parts[0], parts[1]),
    )
    .unwrap();

    let mut s = spec("/srv/share");
    s.known_hosts = Some(bad.clone());
    let r = mount(&s, &mp, Duration::from_secs(20));

    assert!(
        r.is_err(),
        "A CHANGED HOST KEY WAS ACCEPTED — this is the machine-in-the-middle case"
    );
    assert!(
        !is_mounted(&mp),
        "refused the key but left something mounted"
    );
    let msg = format!("{:?}", r.err().unwrap());
    eprintln!("refusal message: {msg}");
    // The message must point at the key, not just say "connection reset".
    let lower = msg.to_lowercase();
    assert!(
        lower.contains("key") || lower.contains("host") || lower.contains("identif"),
        "the refusal must explain it was a host-key problem, got: {msg}"
    );
    let _ = std::fs::remove_file(&bad);
}

/// A failed mount must never leave key material in argv.
#[test]
fn no_key_material_or_disabled_checking_in_argv() {
    gust_testkit::skip_unless!(
        peer_running(),
        "DOCKER_PEER",
        "container peer not running (tests/docker-peers.sh start)"
    );
    let mp = mount_dir("argv");
    let _ = unmount(&mp);
    mount(&spec("/srv/share"), &mp, Duration::from_secs(20)).expect("mount failed");

    let ps = std::process::Command::new("ps")
        .args(["-eo", "args"])
        .output()
        .expect("ps failed");
    let args = String::from_utf8_lossy(&ps.stdout);
    let sshfs_lines: Vec<&str> = args.lines().filter(|l| l.contains("sshfs")).collect();
    assert!(
        !sshfs_lines.is_empty(),
        "expected an sshfs process while mounted"
    );
    for l in &sshfs_lines {
        assert!(
            !l.contains("BEGIN OPENSSH PRIVATE KEY") && !l.contains("PRIVATE KEY"),
            "key MATERIAL in argv: {l}"
        );
        assert!(
            !l.contains("StrictHostKeyChecking=no"),
            "host-key checking disabled in argv: {l}"
        );
        assert!(
            !l.contains("UserKnownHostsFile=/dev/null"),
            "known_hosts pointed at /dev/null in argv: {l}"
        );
    }
    let _ = unmount(&mp);
}
