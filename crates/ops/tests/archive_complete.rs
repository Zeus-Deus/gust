//! Synthetic end-to-end archive execution, separate from display preview.
use gust_core::lister::CancelToken;
use gust_ops::archive::{io::*, jobs::*, ExtractPolicy};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "gust-archive-regression-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(path.join("source")).unwrap();
        std::fs::create_dir(path.join("destination")).unwrap();
        Self(path)
    }
    fn archive(&self) -> PathBuf {
        let path = self.0.join("input.tar");
        assert!(Command::new("bsdtar")
            .arg("-cf")
            .arg(&path)
            .arg("-C")
            .arg(self.0.join("source"))
            .arg(".")
            .status()
            .unwrap()
            .success());
        path
    }
    fn destination(&self) -> PathBuf {
        self.0.join("destination")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn put(path: impl AsRef<Path>, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn all_2005_files_are_extracted_even_when_preview_is_truncated() {
    let f = Fixture::new();
    for n in 0..2005 {
        put(f.0.join(format!("source/file-{n:04}")), b"payload");
    }
    let archive = f.archive();
    let preview = list_preview(&archive, &CancelToken::new()).unwrap();
    assert_eq!(preview.entries.len(), MAX_LISTED);
    assert!(preview.truncated);
    assert_eq!(
        enumerate(&archive, &ExtractPolicy::default(), &CancelToken::new())
            .unwrap()
            .len(),
        2006
    );
    let extracted = extract(&archive, &f.destination(), &ExtractPolicy::default()).unwrap();
    assert_eq!(extracted.files, 2005, "{extracted:?}");
    assert_eq!(std::fs::read_dir(f.destination()).unwrap().count(), 2005);
    for n in 0..2005 {
        assert_eq!(
            std::fs::read(f.destination().join(format!("file-{n:04}"))).unwrap(),
            b"payload"
        );
    }
}

#[test]
fn entry_and_actual_byte_limits_publish_nothing() {
    let f = Fixture::new();
    put(f.0.join("source/first"), &[42; 4096]);
    put(f.0.join("source/second"), b"payload");
    let archive = f.archive();
    for policy in [
        ExtractPolicy {
            max_entries: 2,
            ..Default::default()
        },
        ExtractPolicy {
            max_total_bytes: 4096,
            ..Default::default()
        },
    ] {
        assert!(matches!(
            extract(&archive, &f.destination(), &policy),
            Err(ArchiveError::Limit(_))
        ));
        assert_eq!(std::fs::read_dir(f.destination()).unwrap().count(), 0);
    }
}

#[test]
fn cancellation_during_data_stream_removes_only_owned_staging() {
    let f = Fixture::new();
    put(f.0.join("source/large"), &[1; 65536]);
    let archive = f.archive();
    let foreign = f
        .destination()
        .join(format!(".gust-extract-{}", std::process::id()));
    std::fs::create_dir(&foreign).unwrap();
    put(foreign.join("keep"), b"new work");
    let token = CancelToken::new();
    let result = extract_with(
        &archive,
        &f.destination(),
        &ExtractPolicy::default(),
        None,
        &token,
        &mut |_, _, bytes| {
            if bytes >= 4096 {
                token.cancel();
            }
        },
    );
    assert!(matches!(result, Err(ArchiveError::Cancelled)), "{result:?}");
    assert_eq!(std::fs::read_dir(f.destination()).unwrap().count(), 1);
    assert_eq!(std::fs::read(foreign.join("keep")).unwrap(), b"new work");
}

#[test]
fn symlinks_and_hardlinks_stay_inside_and_keep_identity() {
    use std::os::unix::fs::MetadataExt;
    let f = Fixture::new();
    put(f.0.join("source/original"), b"payload");
    std::fs::hard_link(f.0.join("source/original"), f.0.join("source/hard")).unwrap();
    std::os::unix::fs::symlink("original", f.0.join("source/link")).unwrap();
    std::os::unix::fs::symlink("../outside", f.0.join("source/escape")).unwrap();
    let done = extract(&f.archive(), &f.destination(), &ExtractPolicy::default()).unwrap();
    assert_eq!(done.files, 3);
    assert!(done.refused.iter().any(|s| s.contains("escape")));
    assert_eq!(
        std::fs::read_link(f.destination().join("link")).unwrap(),
        PathBuf::from("original")
    );
    assert_eq!(
        std::fs::metadata(f.destination().join("original"))
            .unwrap()
            .ino(),
        std::fs::metadata(f.destination().join("hard"))
            .unwrap()
            .ino()
    );
}

#[test]
fn destination_symlink_cannot_redirect_extraction() {
    let f = Fixture::new();
    std::fs::create_dir(f.0.join("source/nested")).unwrap();
    put(f.0.join("source/nested/file"), b"payload");
    std::fs::create_dir(f.0.join("outside")).unwrap();
    std::os::unix::fs::symlink(f.0.join("outside"), f.destination().join("nested")).unwrap();
    assert!(extract(&f.archive(), &f.destination(), &ExtractPolicy::default()).is_err());
    assert_eq!(std::fs::read_dir(f.0.join("outside")).unwrap().count(), 0);
}

#[test]
fn dangling_destination_link_is_preserved_by_atomic_keep_both() {
    let f = Fixture::new();
    put(f.0.join("source/file"), b"payload");
    std::os::unix::fs::symlink("missing", f.destination().join("file")).unwrap();
    let done = extract(&f.archive(), &f.destination(), &ExtractPolicy::default()).unwrap();
    assert_eq!(done.files, 1);
    assert_eq!(done.renamed.len(), 1);
    assert_eq!(
        std::fs::read_link(f.destination().join("file")).unwrap(),
        PathBuf::from("missing")
    );
    assert_eq!(
        std::fs::read(f.destination().join("file (2)")).unwrap(),
        b"payload"
    );
}

#[test]
fn structured_names_preserve_newlines_arrows_and_os_bytes() {
    use std::os::unix::ffi::OsStringExt;
    let f = Fixture::new();
    let names = [
        std::ffi::OsString::from("line\nbreak -> literal"),
        std::ffi::OsString::from_vec(b"raw-\xff".to_vec()),
    ];
    for name in &names {
        put(f.0.join("source").join(name), b"payload");
    }
    let done = extract(&f.archive(), &f.destination(), &ExtractPolicy::default()).unwrap();
    assert_eq!(done.files, 2);
    for name in names {
        assert_eq!(
            std::fs::read(f.destination().join(name)).unwrap(),
            b"payload"
        );
    }
}

#[test]
fn truncated_input_never_returns_success() {
    let f = Fixture::new();
    put(f.0.join("source/large"), &[7; 65536]);
    let archive = f.archive();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&archive)
        .unwrap()
        .set_len(1024)
        .unwrap();
    assert!(extract(&archive, &f.destination(), &ExtractPolicy::default()).is_err());
    assert_eq!(std::fs::read_dir(f.destination()).unwrap().count(), 0);
}

#[test]
fn jobs_return_a_terminal_receipt_and_undo_record() {
    let f = Fixture::new();
    put(f.0.join("source/file"), b"payload");
    let mut handle = start(
        "synthetic",
        ArchiveRequest::Extract {
            archive: f.archive(),
            destination: f.destination(),
            policy: ExtractPolicy::default(),
            password: None,
        },
    )
    .unwrap();
    let final_state = handle.wait();
    assert_eq!(final_state.phase, ArchivePhase::Confirmed);
    let result = final_state.result.unwrap().unwrap();
    assert_eq!(result.record.entries.len(), 1);
    assert!(!result.record.subtree_identities.is_empty());
    let plan = gust_ops::undo::plan_undo(&result.record);
    assert!(gust_ops::undo::apply_undo(&plan).is_complete());
    assert!(!f.destination().join("file").exists());
}

#[test]
fn sandbox_fallback_is_refused_and_password_debug_is_redacted() {
    assert_eq!(
        list_command(Path::new("/unused"), false)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::Unsupported
    );
    let password = ArchivePassword::new(b"synthetic-secret".to_vec()).unwrap();
    assert!(!format!("{password:?}").contains("synthetic-secret"));
}

#[test]
fn a_symlink_follows_the_extracted_keep_both_target() {
    let f = Fixture::new();
    put(f.0.join("source/original"), b"archive bytes");
    std::os::unix::fs::symlink("original", f.0.join("source/link")).unwrap();
    put(f.destination().join("original"), b"new user work");
    let result = extract(&f.archive(), &f.destination(), &ExtractPolicy::default()).unwrap();
    assert_eq!(result.files, 2);
    assert_eq!(
        std::fs::read(f.destination().join("original")).unwrap(),
        b"new user work"
    );
    assert_eq!(
        std::fs::read(f.destination().join("link")).unwrap(),
        b"archive bytes"
    );
}

#[test]
fn concurrent_extractions_have_distinct_owned_staging_and_keep_both() {
    let f = Fixture::new();
    put(f.0.join("source/file"), b"payload");
    let archive = f.archive();
    let destination = f.destination();
    std::thread::scope(|scope| {
        let a = scope.spawn(|| extract(&archive, &destination, &ExtractPolicy::default()));
        let b = scope.spawn(|| extract(&archive, &destination, &ExtractPolicy::default()));
        assert_eq!(a.join().unwrap().unwrap().files, 1);
        assert_eq!(b.join().unwrap().unwrap().files, 1);
    });
    assert_eq!(std::fs::read_dir(f.destination()).unwrap().count(), 2);
}

#[test]
fn a_publication_failure_reports_the_partial_receipt() {
    let f = Fixture::new();
    std::fs::create_dir(f.0.join("source/a")).unwrap();
    std::fs::create_dir(f.0.join("source/b")).unwrap();
    put(f.0.join("source/b/file"), b"payload");
    put(f.destination().join("b"), b"new work");
    match extract(&f.archive(), &f.destination(), &ExtractPolicy::default()) {
        Err(ArchiveError::Partial { receipt, .. }) => {
            assert_eq!(receipt.dirs, 1);
            assert_eq!(receipt.published.len(), 1);
            assert_eq!(receipt.published[0].0, f.destination().join("a"));
        }
        result => panic!("must report the created directory as partial, got {result:?}"),
    }
    assert_eq!(
        std::fs::read(f.destination().join("b")).unwrap(),
        b"new work"
    );
}

#[test]
fn publication_receipts_do_not_adopt_a_later_edit() {
    let f = Fixture::new();
    put(f.0.join("source/file"), b"payload");
    let mut changed = false;
    let done = extract_with(
        &f.archive(),
        &f.destination(),
        &ExtractPolicy::default(),
        None,
        &CancelToken::new(),
        &mut |phase, _, _| {
            if phase == ArchivePhase::Publishing
                && !changed
                && f.destination().join("file").exists()
            {
                put(f.destination().join("file"), b"later user work");
                changed = true;
            }
        },
    )
    .unwrap();
    assert!(changed);
    let expected = &done
        .published
        .iter()
        .find(|(p, _)| p.ends_with("file"))
        .unwrap()
        .1;
    assert!(
        !expected.matches(&gust_ops::FileIdentity::of_path(&f.destination().join("file")).unwrap())
    );
}
