//! Live archive tests against a real bsdtar with a genuinely hostile archive.
//!
//! The plan's own qualification (§10) found that the default extractor
//! "created a FIFO and left partial output on failure". This builds an
//! archive containing exactly those hazards and proves Gust refuses them.

use gust_ops::archive::io::*;
use gust_ops::archive::{ArchiveEntryKind, ExtractPolicy};
use std::path::{Path, PathBuf};
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("gust-arch-{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Build an archive containing the hazards the policy exists to refuse.
/// Returns None when the tools needed are unavailable.
fn hostile_archive(dir: &Path) -> Option<PathBuf> {
    let src = dir.join("src");
    std::fs::create_dir_all(&src).ok()?;

    // An ordinary file, which must survive.
    std::fs::write(src.join("good.txt"), "keep me\n").ok()?;
    std::fs::create_dir_all(src.join("sub")).ok()?;
    std::fs::write(src.join("sub/nested.txt"), "also fine\n").ok()?;

    // A FIFO — the exact hazard the plan's fixture produced.
    let fifo = src.join("pipe");
    Command::new("mkfifo").arg(&fifo).output().ok()?;
    if !fifo.exists() {
        return None;
    }

    // A symlink pointing out of the archive.
    std::os::unix::fs::symlink("../../../etc/passwd", src.join("escape")).ok()?;

    // A setuid file.
    let suid = src.join("privileged");
    std::fs::write(&suid, "#!/bin/sh\n").ok()?;
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(&suid).ok()?.permissions();
        p.set_mode(0o4755);
        std::fs::set_permissions(&suid, p).ok()?;
    }

    let archive = dir.join("hostile.tar");
    let out = Command::new("bsdtar")
        .arg("-cf")
        .arg(&archive)
        .arg("-C")
        .arg(&src)
        .arg("--")
        .arg("good.txt")
        .arg("sub")
        .arg("pipe")
        .arg("escape")
        .arg("privileged")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(archive)
}

#[test]
fn bsdtar_is_available() {
    assert!(tool_available(), "these tests need bsdtar");
}

#[test]
fn listing_finds_every_hazard() {
    let d = tmp("list");
    let Some(archive) = hostile_archive(&d) else {
        eprintln!("could not build the hostile archive; skipping");
        return;
    };
    let entries = list(&archive).expect("listing should succeed");

    let by_name = |n: &str| entries.iter().find(|e| e.name == n).cloned();
    assert!(by_name("good.txt").is_some(), "got {entries:#?}");

    let pipe = by_name("pipe").expect("the FIFO must be listed");
    assert_eq!(
        pipe.kind,
        ArchiveEntryKind::Special,
        "a FIFO must be recognised as special, not as a file"
    );

    let link = by_name("escape").expect("the symlink must be listed");
    assert_eq!(link.kind, ArchiveEntryKind::Symlink);
    assert!(link.link_target.as_deref().unwrap_or("").contains(".."));

    let suid = by_name("privileged").expect("the setuid file must be listed");
    assert_ne!(
        suid.mode & 0o4000,
        0,
        "setuid must be visible to the policy"
    );

    std::fs::remove_dir_all(&d).ok();
}

/// Listing must not write anything. Reading what is in an archive is not
/// the same as unpacking it.
#[test]
fn listing_writes_nothing() {
    let d = tmp("readonly");
    let Some(archive) = hostile_archive(&d) else {
        return;
    };
    let dest = d.join("dest");
    std::fs::create_dir_all(&dest).unwrap();

    let before: Vec<_> = std::fs::read_dir(&dest).unwrap().flatten().collect();
    let _ = list(&archive).expect("listing");
    let after: Vec<_> = std::fs::read_dir(&dest).unwrap().flatten().collect();

    assert_eq!(before.len(), after.len(), "listing created something");
    assert!(after.is_empty(), "the destination must still be empty");
    std::fs::remove_dir_all(&d).ok();
}

/// The whole point: extraction refuses the hazards and keeps the rest.
#[test]
fn extraction_refuses_the_hazards_and_keeps_the_rest() {
    let d = tmp("extract");
    let Some(archive) = hostile_archive(&d) else {
        return;
    };
    let dest = d.join("dest");
    std::fs::create_dir_all(&dest).unwrap();

    let done =
        extract(&archive, &dest, &ExtractPolicy::default()).expect("extraction should complete");

    // The good file arrived.
    assert!(
        dest.join("good.txt").is_file(),
        "an ordinary file must be extracted; got {done:?}"
    );

    // None of the hazards did.
    assert!(!dest.join("pipe").exists(), "a FIFO was created");
    assert!(
        !dest.join("escape").exists(),
        "an escaping symlink was created"
    );
    assert!(
        !dest.join("privileged").exists(),
        "a setuid file was extracted"
    );

    // And the user is told, rather than left to discover it.
    assert!(
        !done.refused.is_empty(),
        "refusals must be reported, not silently dropped"
    );
    let said = done.refused.join(" | ");
    assert!(said.contains("pipe"), "the FIFO must be named: {said}");
    assert!(
        said.contains("privileged"),
        "the setuid file must be named: {said}"
    );

    std::fs::remove_dir_all(&d).ok();
}

/// Nothing may be left behind in the destination after an extraction,
/// successful or not: a staging directory that survives looks like a folder
/// the user did not create.
#[test]
fn no_staging_directory_survives() {
    let d = tmp("staging");
    let Some(archive) = hostile_archive(&d) else {
        return;
    };
    let dest = d.join("dest");
    std::fs::create_dir_all(&dest).unwrap();

    let _ = extract(&archive, &dest, &ExtractPolicy::default());

    let leftovers: Vec<String> = std::fs::read_dir(&dest)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with(".gust-extract"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "staging directories survived: {leftovers:?}"
    );
    std::fs::remove_dir_all(&d).ok();
}

/// A round trip: compress a selection, then read it back.
#[test]
fn compressing_then_listing_finds_what_went_in() {
    let d = tmp("roundtrip");
    let src = d.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("one.txt"), "1\n").unwrap();
    std::fs::write(src.join("two.txt"), "2\n").unwrap();

    let archive = d.join("out.tar.zst");
    let out = Command::new("bsdtar")
        .args(compress_args(
            &src,
            &archive,
            &["one.txt".into(), "two.txt".into()],
        ))
        .output()
        .expect("bsdtar");
    assert!(
        out.status.success(),
        "compress failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(archive.is_file(), "the archive was not written");

    let entries = list(&archive).expect("listing the new archive");
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"one.txt"), "got {names:?}");
    assert!(names.contains(&"two.txt"), "got {names:?}");

    std::fs::remove_dir_all(&d).ok();
}

/// A file that is not an archive must be refused with a reason, not a guess.
#[test]
fn a_non_archive_is_refused_clearly() {
    let d = tmp("notarchive");
    let f = d.join("notes.txt");
    std::fs::write(&f, "just some text, definitely not an archive\n").unwrap();

    match list(&f) {
        Err(e) => {
            let m = e.message();
            assert!(!m.is_empty());
            assert!(
                m.to_lowercase().contains("archive") || m.contains("notes.txt"),
                "the message should name the problem: {m}"
            );
        }
        Ok(entries) => assert!(
            entries.is_empty(),
            "a text file must not appear to contain {} entries",
            entries.len()
        ),
    }
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn an_absolute_path_member_is_refused() {
    // bsdtar stores absolute paths with the leading slash stripped by
    // default, so build the hazard directly through the policy instead.
    use gust_ops::archive::{vet_entries, ArchiveEntry};
    let entries = vec![ArchiveEntry {
        name: "/etc/passwd".into(),
        kind: ArchiveEntryKind::Regular,
        link_target: None,
        size: 10,
        mode: 0o644,
    }];
    let (ok, bad) = vet_entries(&entries, &ExtractPolicy::default());
    assert!(ok.is_empty(), "an absolute path must never be allowed");
    assert!(!bad.is_empty());
}
