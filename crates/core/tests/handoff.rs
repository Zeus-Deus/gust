//! Detaching a tab into its own window: what travels with it, and the
//! "the new window is really up" handshake.

use gust_core::handoff::{Handoff, ReadyWait};
use std::path::PathBuf;
use std::time::Duration;

fn sample() -> Handoff {
    Handoff {
        location: PathBuf::from("/home/u/Projects"),
        view: "grid".into(),
        cursor_name: Some("notes.md".into()),
        selected: vec!["a.txt".into(), "b c.pdf".into()],
        back: vec![PathBuf::from("/home/u"), PathBuf::from("/home/u/Documents")],
        show_hidden: true,
    }
}

#[test]
fn everything_the_tab_had_travels() {
    let h = sample();
    assert_eq!(Handoff::decode(&h.encode()), Some(h));
}

#[test]
fn names_with_any_bytes_travel_intact() {
    use std::os::unix::ffi::OsStrExt;
    let mut h = sample();
    let odd = std::ffi::OsStr::from_bytes(b"caf\xe9\nline").to_os_string();
    h.selected = vec![odd.clone()];
    h.cursor_name = Some(odd);
    assert_eq!(Handoff::decode(&h.encode()), Some(h));
}

#[test]
fn a_damaged_handoff_is_refused_not_half_applied() {
    let bytes = sample().encode();
    assert_eq!(Handoff::decode(&bytes[..bytes.len() / 2]), None);
    assert_eq!(Handoff::decode(b"garbage"), None);
}

#[test]
fn the_parent_waits_for_ready_and_knows_when_the_child_died() {
    let d = std::env::temp_dir().join(format!("gust-handoff-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let ready = d.join("ready");

    // A child that exits without ever becoming ready.
    let mut dead = std::process::Command::new("true").spawn().unwrap();
    let w = ReadyWait::new(ready.clone(), Duration::from_secs(5));
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        w.poll(&mut dead).is_err(),
        "a dead child must not count as ready"
    );

    // A child that signals ready.
    let mut live = std::process::Command::new("sleep")
        .arg("5")
        .spawn()
        .unwrap();
    let w = ReadyWait::new(ready.clone(), Duration::from_secs(5));
    assert_eq!(w.poll(&mut live), Ok(false), "not ready yet");
    std::fs::write(&ready, b"ok").unwrap();
    assert_eq!(w.poll(&mut live), Ok(true));
    let _ = live.kill();
    let _ = live.wait();

    // A child that never answers within the deadline.
    let _ = std::fs::remove_file(&ready);
    let mut slow = std::process::Command::new("sleep")
        .arg("5")
        .spawn()
        .unwrap();
    let w = ReadyWait::new(ready, Duration::from_millis(50));
    std::thread::sleep(Duration::from_millis(80));
    assert!(
        w.poll(&mut slow).is_err(),
        "a window that never appears times out"
    );
    let _ = slow.kill();
    let _ = slow.wait();
}
