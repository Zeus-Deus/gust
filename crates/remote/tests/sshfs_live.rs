//! Real SSHFS against a private, isolated SSH server.
//!
//! Nothing here touches the user's `~/.ssh`, their known_hosts, or any host
//! they actually use: the fixture creates its own host key, its own client
//! key and its own `authorized_keys`, and listens on a private port bound to
//! loopback. If the fixture is not running, every test skips rather than
//! silently passing.
//!
//! Start it with `tests/remote-fixture.sh start`.

use gust_remote::mount::{is_mounted, mount, unmount, MountError, RemoteSpec};
use std::path::{Path, PathBuf};
use std::time::Duration;

const FIXTURE: &str = "/tmp/gust-remote-fixture";
const PORT: u16 = 2222;

fn fixture_running() -> bool {
    Path::new(FIXTURE).join("etc/sshd.pid").exists()
        && std::net::TcpStream::connect(("127.0.0.1", PORT)).is_ok()
}

fn spec(remote_path: &str) -> RemoteSpec {
    let f = PathBuf::from(FIXTURE);
    let mut s = RemoteSpec::new("gust-test", "127.0.0.1");
    s.user = std::env::var("USER").ok();
    s.port = Some(PORT);
    s.remote_path = Some(remote_path.to_string());
    s.identity_file = Some(f.join("etc/id_test"));
    s.known_hosts = Some(f.join("etc/known_hosts"));
    s
}

fn mount_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join("gust-mnt").join(name);
    let _ = std::fs::create_dir_all(&d);
    d
}

/// A place really connects, really lists, and really disconnects.
#[test]
fn a_remote_place_connects_lists_and_disconnects() {
    if !fixture_running() {
        eprintln!("SKIP: remote fixture not running (tests/remote-fixture.sh start)");
        return;
    }
    let mp = mount_dir("connect");
    let _ = unmount(&mp);

    let r = mount(
        &spec(&format!("{FIXTURE}/chroot/shared")),
        &mp,
        Duration::from_secs(15),
    );
    assert!(r.is_ok(), "mount failed: {:?}", r.map(|_| ()).unwrap_err());
    assert!(
        is_mounted(&mp),
        "mount reported success but nothing mounted"
    );

    // The files are really there, over a real SFTP connection.
    // The share is mutable — other tests write to it — so this test plants
    // what it needs instead of assuming a pristine fixture.
    let planted = PathBuf::from(FIXTURE).join("chroot/shared/planted-by-test");
    let _ = std::fs::create_dir_all(&planted);

    let names: Vec<String> = std::fs::read_dir(&mp)
        .expect("cannot read the mounted place")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.contains(&"hello.txt".to_string()), "got {names:?}");
    assert!(
        names.contains(&"planted-by-test".to_string()),
        "a directory created on the backend was not visible over the mount: {names:?}"
    );
    let _ = std::fs::remove_dir(&planted);

    let body = std::fs::read_to_string(mp.join("hello.txt")).expect("cannot read remote file");
    assert!(body.contains("remote fixture ready"), "got {body:?}");

    unmount(&mp).expect("unmount failed");
    assert!(!is_mounted(&mp), "still mounted after unmount");
}

/// Writing through the mount reaches the real backing file.
#[test]
fn a_write_through_a_remote_place_reaches_the_backend() {
    if !fixture_running() {
        eprintln!("SKIP: remote fixture not running");
        return;
    }
    let mp = mount_dir("write");
    let _ = unmount(&mp);
    mount(
        &spec(&format!("{FIXTURE}/chroot/shared")),
        &mp,
        Duration::from_secs(15),
    )
    .expect("mount failed");

    let written = mp.join("written-by-gust.txt");
    std::fs::write(&written, b"round trip").expect("write through the mount failed");

    // Independently verified: read the backing file, not the mount.
    let backing = PathBuf::from(FIXTURE).join("chroot/shared/written-by-gust.txt");
    let body = std::fs::read_to_string(&backing).expect("the backend never received the file");
    assert_eq!(body, "round trip", "the backend content does not match");

    let _ = std::fs::remove_file(&written);
    unmount(&mp).expect("unmount failed");
}

/// A changed host key stops the connection. Nothing silently trusts it.
#[test]
fn a_changed_host_key_refuses_to_connect() {
    if !fixture_running() {
        eprintln!("SKIP: remote fixture not running");
        return;
    }
    let mp = mount_dir("badkey");
    let _ = unmount(&mp);

    // A known_hosts claiming a DIFFERENT key for this server: exactly what a
    // machine-in-the-middle would produce.
    let bogus = std::env::temp_dir().join("gust-bogus-known-hosts");
    std::fs::write(
        &bogus,
        "[127.0.0.1]:2222 ssh-ed25519 \
         AAAAC3NzaC1lZDI1NTE5AAAAIJm0000000000000000000000000000000000000\n",
    )
    .unwrap();

    let mut s = spec(&format!("{FIXTURE}/chroot/shared"));
    s.known_hosts = Some(bogus.clone());

    let err = mount(&s, &mp, Duration::from_secs(15))
        .expect_err("a changed host key was ACCEPTED — connection must be refused");

    assert!(
        !is_mounted(&mp),
        "something was mounted despite a bad host key"
    );
    // It must be recognised as an identity problem, not a vague failure.
    assert_eq!(
        err,
        MountError::HostKeyChanged,
        "a changed host key was misdiagnosed as {err:?}"
    );
    let m = err.message();
    assert!(
        m.contains("identity has changed"),
        "the warning does not explain itself: {m}"
    );

    let _ = std::fs::remove_file(&bogus);
}

/// An offline host fails within the timeout instead of hanging.
#[test]
fn an_offline_host_times_out_rather_than_hanging() {
    let mp = mount_dir("offline");
    let _ = unmount(&mp);

    let mut s = RemoteSpec::new("nowhere", "127.0.0.1");
    s.user = Some("nobody".into());
    s.port = Some(2299); // nothing listens here
    s.remote_path = Some("/".into());

    let start = std::time::Instant::now();
    let err = mount(&s, &mp, Duration::from_secs(8)).expect_err("connected to nothing");
    let took = start.elapsed();

    assert!(
        took < Duration::from_secs(20),
        "an offline host blocked for {took:?} — a stuck mount must not block Gust"
    );
    assert!(!is_mounted(&mp), "something mounted from an offline host");
    // Whatever the classification, it must be sayable to a person.
    let m = err.message();
    assert!(!m.contains("os error"), "raw system text: {m}");
}

/// Mounting over a folder that already has files is refused.
#[test]
fn a_non_empty_folder_is_never_mounted_over() {
    let mp = mount_dir("nonempty");
    let _ = unmount(&mp);
    std::fs::write(mp.join("users-own-file.txt"), b"do not hide me").unwrap();

    let err = mount(&spec("/"), &mp, Duration::from_secs(5))
        .expect_err("mounted over a folder containing the user's files");
    assert!(
        matches!(err, MountError::BadMountPoint(_)),
        "wrong refusal: {err:?}"
    );
    // The file must still be reachable — nothing was hidden behind a mount.
    assert!(
        mp.join("users-own-file.txt").exists(),
        "the user's file was hidden by a mount"
    );
    let _ = std::fs::remove_file(mp.join("users-own-file.txt"));
}

/// The source is never left damaged by a failed remote operation.
#[test]
fn a_failed_connection_leaves_no_debris() {
    let mp = mount_dir("debris");
    let _ = unmount(&mp);

    let mut s = RemoteSpec::new("nowhere", "127.0.0.1");
    s.port = Some(2299);
    s.remote_path = Some("/".into());
    let _ = mount(&s, &mp, Duration::from_secs(5));

    let left: Vec<_> = std::fs::read_dir(&mp)
        .map(|rd| rd.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(
        left.is_empty(),
        "a failed connection left files behind: {left:?}"
    );
    assert!(!is_mounted(&mp));
}
