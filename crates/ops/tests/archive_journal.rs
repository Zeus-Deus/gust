use gust_ops::archive::{io::*, jobs::*, ExtractPolicy};
use gust_ops::{JobState, Journal};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "gust-archive-durable-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("source/sub")).unwrap();
        std::fs::create_dir(root.join("destination")).unwrap();
        std::fs::write(root.join("source/sub/file"), b"payload").unwrap();
        Self(root)
    }
    fn archive(&self) -> PathBuf {
        let path = self.0.join("input.tar");
        assert!(Command::new("bsdtar")
            .arg("-cf")
            .arg(&path)
            .arg("-C")
            .arg(self.0.join("source"))
            .arg("sub")
            .status()
            .unwrap()
            .success());
        path
    }
    fn request(&self) -> ArchiveRequest {
        ArchiveRequest::Extract {
            archive: self.archive(),
            destination: self.0.join("destination"),
            policy: ExtractPolicy::default(),
            password: None,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn lifecycle_intents_and_terminal_receipt_are_durable() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let path = f.0.join("state/journal.ndjson");
    let mut handle = start_journalled("durable", f.request(), Journal::new(&path)).unwrap();
    let final_state = handle.wait();
    assert_eq!(
        final_state.phase,
        ArchivePhase::Confirmed,
        "{final_state:?}"
    );
    let text = std::fs::read_to_string(&path).unwrap();
    let records: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records[0]["state"], "Planned");
    let intents: Vec<_> = records
        .iter()
        .enumerate()
        .filter(|(_, r)| r["update"] == "ArchiveIntent")
        .collect();
    let committed: Vec<_> = records
        .iter()
        .enumerate()
        .filter(|(_, r)| r["update"] == "ArchivePublished")
        .collect();
    assert_eq!(intents.len(), 2);
    assert_eq!(committed.len(), 2);
    for (position, record) in committed {
        assert!(intents
            .iter()
            .any(|(before, intent)| *before < position && intent["index"] == record["index"]));
    }
    let loaded = Journal::new(&path).load();
    let record = loaded.last().unwrap();
    assert_eq!(record.state, JobState::Confirmed);
    assert_eq!(record.subtree_identities.len(), 2);
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn failed_journal_admission_has_no_destination_effects() {
    let f = Fixture::new();
    std::fs::write(f.0.join("blocked"), b"new work").unwrap();
    let mut handle = start_journalled(
        "blocked",
        f.request(),
        Journal::new(f.0.join("blocked/journal")),
    )
    .unwrap();
    assert_eq!(handle.wait().phase, ArchivePhase::Failed);
    assert_eq!(
        std::fs::read_dir(f.0.join("destination")).unwrap().count(),
        0
    );
}

#[test]
fn terminal_failure_preserves_a_partial_directory_receipt() {
    let f = Fixture::new();
    std::fs::create_dir(f.0.join("source/a")).unwrap();
    // A directory target blocked by a file fails after the earlier folder was
    // created; all effects must appear in the final failed journal record.
    std::fs::create_dir(f.0.join("source/b-dir")).unwrap();
    let archive = f.0.join("partial-dir.tar");
    assert!(Command::new("bsdtar")
        .arg("-cf")
        .arg(&archive)
        .arg("-C")
        .arg(f.0.join("source"))
        .args(["a", "b-dir"])
        .status()
        .unwrap()
        .success());
    std::fs::write(f.0.join("destination/b-dir"), b"new work").unwrap();
    let path = f.0.join("journal");
    let mut handle = start_journalled(
        "partial",
        ArchiveRequest::Extract {
            archive,
            destination: f.0.join("destination"),
            policy: ExtractPolicy::default(),
            password: None,
        },
        Journal::new(&path),
    )
    .unwrap();
    assert_eq!(handle.wait().phase, ArchivePhase::Failed);
    let loaded = Journal::new(path).load();
    let record = loaded.last().unwrap();
    assert_eq!(record.state, JobState::Failed);
    assert_eq!(record.subtree_identities.len(), 1);
    assert!(record.subtree_identities[0].0.ends_with("a"));
}

#[test]
fn journal_receipts_round_trip_non_utf8_names() {
    use std::os::unix::ffi::OsStringExt;
    let f = Fixture::new();
    let name = std::ffi::OsString::from_vec(b"raw-\xff".to_vec());
    std::fs::write(f.0.join("source/sub").join(&name), b"raw").unwrap();
    let path = f.0.join("journal");
    let mut handle = start_journalled("bytes", f.request(), Journal::new(&path)).unwrap();
    assert_eq!(handle.wait().phase, ArchivePhase::Confirmed);
    let records = Journal::new(path).load();
    assert!(records
        .last()
        .unwrap()
        .subtree_identities
        .iter()
        .any(|(path, _)| path.ends_with(&name)));
}

#[test]
fn encrypted_zip_has_typed_missing_wrong_and_successful_password_results() {
    if !std::path::Path::new("/usr/bin/7z").is_file() {
        gust_testkit::skip!("ARCHIVE", "7z required for synthetic encrypted ZIP fixture");
    }
    let f = Fixture::new();
    let archive = f.0.join("encrypted.zip");
    let mut child = Command::new("/usr/bin/7z")
        .args(["a", "-tzip", "-mem=AES256", "-p", "-bd", "-y"])
        .arg(&archive)
        .arg("--")
        .arg("sub")
        .current_dir(f.0.join("source"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"synthetic-password\nsynthetic-password\n")
        .unwrap();
    assert!(child.wait().unwrap().success());
    let request = |password| ArchiveRequest::Extract {
        archive: archive.clone(),
        destination: f.0.join("destination"),
        policy: ExtractPolicy::default(),
        password,
    };
    let mut no_password = start("missing", request(None)).unwrap();
    assert!(matches!(
        no_password.wait().result.unwrap(),
        Err(ArchiveError::PasswordRequired)
    ));
    let mut wrong = start(
        "wrong",
        request(Some(
            ArchivePassword::new(b"wrong-synthetic".to_vec()).unwrap(),
        )),
    )
    .unwrap();
    assert!(matches!(
        wrong.wait().result.unwrap(),
        Err(ArchiveError::IncorrectPassword)
    ));
    let mut correct = start(
        "correct",
        request(Some(
            ArchivePassword::new(b"synthetic-password".to_vec()).unwrap(),
        )),
    )
    .unwrap();
    assert_eq!(correct.wait().phase, ArchivePhase::Confirmed);
    assert_eq!(
        std::fs::read(f.0.join("destination/sub/file")).unwrap(),
        b"payload"
    );
}

#[test]
fn new_folder_is_one_owned_atomic_publication_with_an_undo_receipt() {
    let f = Fixture::new();
    let destination = f.0.join("new-folder");
    let path = f.0.join("journal");
    let mut handle = start_journalled(
        "new-root",
        ArchiveRequest::ExtractIntoNew {
            archive: f.archive(),
            destination: destination.clone(),
            policy: ExtractPolicy::default(),
            password: None,
        },
        Journal::new(&path),
    )
    .unwrap();
    let state = handle.wait();
    assert_eq!(state.phase, ArchivePhase::Confirmed, "{state:?}");
    let result = state.result.unwrap().unwrap();
    assert_eq!(
        std::fs::read(destination.join("sub/file")).unwrap(),
        b"payload"
    );
    assert_eq!(result.record.entries.len(), 1);
    assert_eq!(
        result.record.entries[0].destination.as_ref(),
        Some(&destination)
    );
    assert_eq!(result.record.subtree_identities.len(), 3);
    let text = std::fs::read_to_string(&path).unwrap();
    let intents = text
        .lines()
        .filter(|line| line.contains("ArchiveIntent"))
        .count();
    assert_eq!(
        intents, 1,
        "only the final root publication is user-visible"
    );
    assert!(gust_ops::undo::apply_undo(&gust_ops::undo::plan_undo(&result.record)).is_complete());
    assert!(!destination.exists());
}

#[test]
fn new_folder_failures_never_reuse_existing_work_or_publish_private_output() {
    let f = Fixture::new();
    let destination = f.0.join("new-folder");
    let mut limited = start_journalled(
        "limited",
        ArchiveRequest::ExtractIntoNew {
            archive: f.archive(),
            destination: destination.clone(),
            policy: ExtractPolicy {
                max_total_bytes: 0,
                ..Default::default()
            },
            password: None,
        },
        Journal::new(f.0.join("journal")),
    )
    .unwrap();
    assert!(matches!(
        limited.wait().result.unwrap(),
        Err(ArchiveError::Limit(_))
    ));
    assert!(!destination.exists());
    assert!(!std::fs::read_dir(&f.0).unwrap().flatten().any(|e| e
        .file_name()
        .to_string_lossy()
        .starts_with(".gust-extract-")));
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(destination.join("new-work"), b"keep").unwrap();
    let mut occupied = start_journalled(
        "occupied",
        ArchiveRequest::ExtractIntoNew {
            archive: f.archive(),
            destination: destination.clone(),
            policy: ExtractPolicy::default(),
            password: None,
        },
        Journal::new(f.0.join("journal")),
    )
    .unwrap();
    assert!(matches!(
        occupied.wait().result.unwrap(),
        Err(ArchiveError::DestinationExists)
    ));
    assert_eq!(
        std::fs::read(destination.join("new-work")).unwrap(),
        b"keep"
    );
    assert_eq!(std::fs::read_dir(destination).unwrap().count(), 1);
}
