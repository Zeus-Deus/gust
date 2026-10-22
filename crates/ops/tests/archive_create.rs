use gust_core::lister::CancelToken;
use gust_ops::{
    archive::{
        io::{self, ArchiveError, ArchiveFormat, ArchivePassword},
        jobs::{start_journalled, ArchivePhase, ArchiveRequest},
        ExtractPolicy,
    },
    journal::{JobKind, JobState, Journal},
};
use std::{ffi::OsString, path::PathBuf};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "gust-archive-create-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(path.join("source/folder")).unwrap();
        std::fs::write(path.join("source/folder/file"), b"contents\n").unwrap();
        std::fs::write(path.join("source/-dash"), b"flag safe").unwrap();
        std::fs::write(path.join("source/line\nbreak"), b"byte safe").unwrap();
        Self(path)
    }
    fn names(&self) -> Vec<OsString> {
        vec!["folder".into(), "-dash".into(), "line\nbreak".into()]
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn all_formats_round_trip_complete_byte_names_and_journal_undo() {
    let f = Fixture::new();
    for (format, extension) in [
        (ArchiveFormat::TarZstd, "tar.zst"),
        (ArchiveFormat::Zip, "zip"),
        (ArchiveFormat::SevenZip, "7z"),
    ] {
        let destination = f.0.join(format!("made.{extension}"));
        let journal = Journal::new(f.0.join(format!("journal-{extension}")));
        let mut handle = start_journalled(
            extension,
            ArchiveRequest::Create {
                directory: f.0.join("source"),
                names: f.names(),
                destination: destination.clone(),
                format,
                password: None,
            },
            Journal::new(journal.path()),
        )
        .unwrap();
        let progress = handle.wait();
        assert_eq!(progress.phase, ArchivePhase::Confirmed, "{progress:?}");
        let result = progress.result.unwrap().unwrap();
        assert!(result.extracted.is_none());
        assert_eq!(result.record.kind, JobKind::Compress);
        assert_eq!(result.record.state, JobState::Confirmed);
        assert_eq!(result.record.entries.len(), 1);
        assert_eq!(journal.load().last().unwrap().kind, JobKind::Compress);
        let out = f.0.join(format!("out-{extension}"));
        std::fs::create_dir(&out).unwrap();
        io::extract(&destination, &out, &ExtractPolicy::default()).unwrap();
        assert_eq!(
            std::fs::read(out.join("folder/file")).unwrap(),
            b"contents\n"
        );
        assert_eq!(std::fs::read(out.join("-dash")).unwrap(), b"flag safe");
        assert_eq!(
            std::fs::read(out.join("line\nbreak")).unwrap(),
            b"byte safe"
        );
        assert!(
            gust_ops::undo::apply_undo(&gust_ops::undo::plan_undo(&result.record)).is_complete()
        );
        assert!(!destination.exists());
        assert!(f.0.join("source/folder/file").exists());
    }
}
#[test]
fn encrypted_zip_password_is_typed_and_debug_redacted() {
    let f = Fixture::new();
    let destination = f.0.join("locked.zip");
    let password = ArchivePassword::new(b"synthetic test secret".to_vec()).unwrap();
    assert!(!format!("{password:?}").contains("synthetic"));
    io::create_with(
        &f.0.join("source"),
        &f.names(),
        &destination,
        ArchiveFormat::Zip,
        Some(&password),
        &CancelToken::new(),
        &mut |_, _, _| {},
    )
    .unwrap();
    let out = f.0.join("out");
    std::fs::create_dir(&out).unwrap();
    assert_eq!(
        io::extract(&destination, &out, &ExtractPolicy::default()),
        Err(ArchiveError::PasswordRequired)
    );
    assert_eq!(
        io::extract_with(
            &destination,
            &out,
            &ExtractPolicy::default(),
            Some(&ArchivePassword::new(b"wrong".to_vec()).unwrap()),
            &CancelToken::new(),
            &mut |_, _, _| {}
        ),
        Err(ArchiveError::IncorrectPassword)
    );
    io::extract_with(
        &destination,
        &out,
        &ExtractPolicy::default(),
        Some(&password),
        &CancelToken::new(),
        &mut |_, _, _| {},
    )
    .unwrap();
    assert_eq!(
        std::fs::read(out.join("folder/file")).unwrap(),
        b"contents\n"
    );
}
#[test]
fn occupied_canceled_changed_and_recursive_outputs_never_publish() {
    let f = Fixture::new();
    let source = f.0.join("source");
    let destination = f.0.join("archive.zip");
    std::os::unix::fs::symlink("absent", &destination).unwrap();
    assert_eq!(
        io::create_with(
            &source,
            &f.names(),
            &destination,
            ArchiveFormat::Zip,
            None,
            &CancelToken::new(),
            &mut |_, _, _| {}
        ),
        Err(ArchiveError::DestinationExists)
    );
    assert_eq!(
        std::fs::read_link(&destination).unwrap(),
        PathBuf::from("absent")
    );
    std::fs::remove_file(&destination).unwrap();
    let cancel = CancelToken::new();
    cancel.cancel();
    assert_eq!(
        io::create_with(
            &source,
            &f.names(),
            &destination,
            ArchiveFormat::Zip,
            None,
            &cancel,
            &mut |_, _, _| {}
        ),
        Err(ArchiveError::Cancelled)
    );
    assert!(!destination.exists());
    assert!(io::create_with(
        &source,
        &f.names(),
        &source.join("folder/recursive.zip"),
        ArchiveFormat::Zip,
        None,
        &CancelToken::new(),
        &mut |_, _, _| {}
    )
    .is_err());
    let mut changed = false;
    assert!(io::create_with(
        &source,
        &f.names(),
        &destination,
        ArchiveFormat::Zip,
        None,
        &CancelToken::new(),
        &mut |phase, _, _| {
            if phase == ArchivePhase::Creating && !changed {
                changed = true;
                std::fs::write(source.join("folder/new-work"), b"preserve").unwrap();
            }
        }
    )
    .is_err());
    assert!(!destination.exists());
    assert_eq!(
        std::fs::read(source.join("folder/new-work")).unwrap(),
        b"preserve"
    );
    assert!(!std::fs::read_dir(&f.0).unwrap().flatten().any(|e| e
        .file_name()
        .to_string_lossy()
        .starts_with(".gust-extract-")));
}
#[test]
fn links_are_stored_without_reading_targets_and_unsupported_encryption_has_no_effects() {
    let f = Fixture::new();
    let source = f.0.join("source");
    std::os::unix::fs::symlink("folder/file", source.join("link")).unwrap();
    let names = vec![OsString::from("folder"), OsString::from("link")];
    let destination = f.0.join("link.tar.zst");
    io::create_with(
        &source,
        &names,
        &destination,
        ArchiveFormat::TarZstd,
        None,
        &CancelToken::new(),
        &mut |_, _, _| {},
    )
    .unwrap();
    let out = f.0.join("out");
    std::fs::create_dir(&out).unwrap();
    io::extract(&destination, &out, &ExtractPolicy::default()).unwrap();
    assert_eq!(
        std::fs::read_link(out.join("link")).unwrap(),
        PathBuf::from("folder/file")
    );
    let password = ArchivePassword::new(b"synthetic".to_vec()).unwrap();
    assert!(matches!(
        io::create_with(
            &source,
            &names,
            &f.0.join("unsupported.7z"),
            ArchiveFormat::SevenZip,
            Some(&password),
            &CancelToken::new(),
            &mut |_, _, _| {}
        ),
        Err(ArchiveError::Unsupported(_))
    ));
    assert!(!f.0.join("unsupported.7z").exists());
}
