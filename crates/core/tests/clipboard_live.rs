//! Live clipboard tests: does another application actually see what Gust
//! puts on the clipboard, and does Gust see what another one leaves?
//!
//! These need a Wayland session and wl-clipboard. Run inside one:
//!   ./tests/vmsession cargo test -p gust-core --test clipboard_live -- --test-threads=1

use gust_core::clipboard::*;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// These tests REPLACE the real clipboard of whoever runs them, so they only
/// run when asked to: `GUST_LIVE_CLIPBOARD=1` (the QA VM sets it). A normal
/// `cargo test` on a desktop must never eat what somebody just copied.
fn live_clipboard_opted_in() -> bool {
    std::env::var("GUST_LIVE_CLIPBOARD")
        .map(|v| v == "1")
        .unwrap_or(false)
        || gust_testkit::required("CLIPBOARD")
}

macro_rules! need_wayland {
    () => {
        gust_testkit::skip_unless!(
            live_clipboard_opted_in(),
            "CLIPBOARD",
            "writes the real clipboard; set GUST_LIVE_CLIPBOARD=1 to run"
        );
        gust_testkit::skip_unless!(
            std::env::var_os("WAYLAND_DISPLAY").is_some() && system_clipboard_available(),
            "CLIPBOARD",
            "no Wayland session or wl-clipboard"
        );
    };
}

/// Read a type back with wl-paste, the way another application would.
fn paste(ty: &str) -> Option<String> {
    let out = Command::new("wl-paste")
        .arg("--no-newline")
        .arg("--type")
        .arg(ty)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

#[test]
fn wl_clipboard_is_available() {
    gust_testkit::skip_unless!(
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
        "CLIPBOARD",
        "not in a Wayland session"
    );
    assert!(
        system_clipboard_available(),
        "wl-copy is needed for system clipboard interop"
    );
}

/// The whole point: a copy in Gust must be visible to another application.
#[test]
fn a_copy_reaches_the_system_clipboard() {
    need_wayland!();
    let paths = vec![
        PathBuf::from("/tmp/gust-clip-one.txt"),
        PathBuf::from("/tmp/gust-clip-two.txt"),
    ];
    offer(ClipVerb::Copy, &paths).expect("offering should succeed");
    std::thread::sleep(std::time::Duration::from_millis(600));

    let got = paste("x-special/gnome-copied-files")
        .expect("another application should be able to read it");
    assert!(got.starts_with("copy"), "got {got:?}");
    assert!(got.contains("file:///tmp/gust-clip-one.txt"), "got {got:?}");
    assert!(got.contains("file:///tmp/gust-clip-two.txt"), "got {got:?}");
}

/// A cut must survive the trip, or another file manager would copy when the
/// user asked to move.
#[test]
fn a_cut_survives_as_a_cut() {
    need_wayland!();
    offer(ClipVerb::Cut, &[PathBuf::from("/tmp/gust-clip-cut.txt")])
        .expect("offering should succeed");
    std::thread::sleep(std::time::Duration::from_millis(600));

    let got = paste("x-special/gnome-copied-files").expect("readable");
    assert!(
        got.starts_with("cut"),
        "a cut must not arrive as a copy: {got:?}"
    );
}

/// And the other direction: what another file manager leaves, Gust reads.
#[test]
fn gust_reads_what_another_application_left() {
    need_wayland!();
    // Exactly what Nautilus puts on the clipboard for a cut.
    let payload = "cut\nfile:///tmp/gust-from-elsewhere.txt";
    let mut child = Command::new("wl-copy")
        .arg("--type")
        .arg("x-special/gnome-copied-files")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("wl-copy");
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(payload.as_bytes()).unwrap();
    }
    std::thread::sleep(std::time::Duration::from_millis(600));

    let got = read().expect("Gust should read another application's clipboard");
    // Reaped here rather than left running: wl-copy holding an offer is
    // correct in the APP, but a test that leaks one accumulates them across
    // runs, exactly as the QA suite once leaked editors.
    let _ = child.kill();
    let _ = child.wait();

    assert_eq!(got.verb, ClipVerb::Cut);
    assert_eq!(
        got.paths,
        vec![PathBuf::from("/tmp/gust-from-elsewhere.txt")]
    );
}

/// A name full of the characters that break naive URI handling.
#[test]
fn a_difficult_name_survives_the_round_trip() {
    need_wayland!();
    let nasty = PathBuf::from("/tmp/my holiday #1 & more.txt");
    offer(ClipVerb::Copy, std::slice::from_ref(&nasty)).expect("offering");
    std::thread::sleep(std::time::Duration::from_millis(600));

    let got = read().expect("readable");
    assert_eq!(
        got.paths,
        vec![nasty],
        "a space, a hash and an ampersand must all survive"
    );
}

/// An http URI on the clipboard is not something to paste into a folder.
#[test]
fn a_web_uri_is_not_read_as_a_file() {
    need_wayland!();
    let mut child = Command::new("wl-copy")
        .arg("--type")
        .arg("text/uri-list")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("wl-copy");
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(b"http://example.com/thing.zip\r\n")
            .unwrap();
    }
    std::thread::sleep(std::time::Duration::from_millis(600));

    let got = read();
    let _ = child.kill();
    let _ = child.wait();
    match got {
        None => {}
        Some(c) => assert!(
            c.paths.is_empty(),
            "a web URI must not become a path: {:?}",
            c.paths
        ),
    }
}

/// Ordinary copied text must not look like a file list.
#[test]
fn plain_prose_is_not_a_file_list() {
    need_wayland!();
    let mut child = Command::new("wl-copy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("wl-copy");
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(b"just some words I copied").unwrap();
    }
    std::thread::sleep(std::time::Duration::from_millis(600));

    let got = read();
    let _ = child.kill();
    let _ = child.wait();
    match got {
        None => {}
        Some(c) => assert!(
            c.paths.is_empty(),
            "prose became a file list: {:?}",
            c.paths
        ),
    }
}

/// No file name may reach a command line, where `ps` would show it.
#[test]
fn no_path_reaches_a_command_line() {
    need_wayland!();
    let marker = "/tmp/gust-clip-secret-marker-9f3a.txt";
    offer(ClipVerb::Copy, &[PathBuf::from(marker)]).expect("offering");
    std::thread::sleep(std::time::Duration::from_millis(400));

    let ps = Command::new("ps")
        .args(["-eo", "args"])
        .output()
        .expect("ps");
    let text = String::from_utf8_lossy(&ps.stdout);
    assert!(
        !text.contains(marker),
        "the path appeared in a command line; it belongs on stdin"
    );
}

/// One copy must serve every kind of application at once: a file manager
/// reads the GNOME type, a browser upload box or chat app reads
/// text/uri-list, and a terminal or editor reads plain text. Offering only
/// the GNOME type meant pasting into anything but a file manager gave
/// nothing at all.
#[test]
fn one_copy_is_readable_as_files_uris_and_plain_text() {
    need_wayland!();
    let paths = vec![
        PathBuf::from("/tmp/gust clip/one.txt"),
        PathBuf::from("/tmp/gust-clip-two.txt"),
    ];
    offer(ClipVerb::Copy, &paths).expect("offering should succeed");
    std::thread::sleep(std::time::Duration::from_millis(600));

    let types = Command::new("wl-paste")
        .arg("--list-types")
        .output()
        .unwrap();
    let types = String::from_utf8_lossy(&types.stdout).to_string();
    for want in [
        "x-special/gnome-copied-files",
        "text/uri-list",
        "text/plain",
    ] {
        assert!(
            types.lines().any(|t| t == want),
            "{want} missing from {types:?}"
        );
    }
    let gnome = paste("x-special/gnome-copied-files").unwrap();
    assert!(gnome.starts_with("copy\n"), "{gnome:?}");
    let uris = paste("text/uri-list").unwrap();
    assert_eq!(
        uris,
        "file:///tmp/gust%20clip/one.txt\r\nfile:///tmp/gust-clip-two.txt\r\n"
    );
    let text = paste("text/plain").unwrap();
    assert_eq!(text, "/tmp/gust clip/one.txt\n/tmp/gust-clip-two.txt");
    // And Gust still reads its own offer back as a file list.
    let back = read().expect("Gust reads it back");
    assert_eq!(back.paths, paths);
}
