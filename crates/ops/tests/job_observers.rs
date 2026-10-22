use gust_core::lister::CancelToken;
use gust_ops::{
    archive::{
        io::ArchiveFormat,
        jobs::{self as archive_jobs, ArchivePhase, ArchiveRequest},
    },
    jobs::{self, Request},
    ConflictPolicy, JobState,
};
use std::path::PathBuf;
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "gust-observers-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(p.join("source")).unwrap();
        std::fs::create_dir(p.join("dest")).unwrap();
        std::fs::write(p.join("source/file"), b"retained").unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn copy_observers_survive_worker_handoff_without_owning_its_join() {
    let f = Fixture::new();
    let mut handle = jobs::start(
        "copy",
        Request::Copy {
            sources: vec![f.0.join("source/file")],
            dest_dir: f.0.join("dest"),
        },
        ConflictPolicy::Ask,
    );
    let observer = handle.observer();
    let clone = observer.clone();
    drop(clone);
    let finished = std::thread::spawn(move || handle.wait()).join().unwrap();
    assert_eq!(finished.state, JobState::Confirmed);
    assert!(observer.is_finished());
    assert_eq!(observer.record().state, JobState::Confirmed);
    assert_eq!(observer.snapshot().entries_done, finished.entries_done);
    assert!(!observer.cancel_requested());
    observer.pause();
    assert!(!observer.is_paused());
    observer.resume();
    assert_eq!(std::fs::read(f.0.join("dest/file")).unwrap(), b"retained");
}
#[test]
fn archive_observers_do_not_cancel_on_drop_and_preserve_terminal_receipts() {
    let f = Fixture::new();
    let mut handle = archive_jobs::start(
        "archive",
        ArchiveRequest::Create {
            directory: f.0.join("source"),
            names: vec!["file".into()],
            destination: f.0.join("created.zip"),
            format: ArchiveFormat::Zip,
            password: None,
        },
    )
    .unwrap();
    let observer = handle.observer();
    drop(observer.clone());
    let finished = std::thread::spawn(move || handle.wait()).join().unwrap();
    assert_eq!(finished.phase, ArchivePhase::Confirmed, "{finished:?}");
    assert!(observer.is_finished());
    assert_eq!(
        observer.snapshot().result.unwrap().unwrap().record.state,
        JobState::Confirmed
    );
    let cancel = CancelToken::new();
    let out = f.0.join("out");
    std::fs::create_dir(&out).unwrap();
    gust_ops::archive::io::extract_with(
        &f.0.join("created.zip"),
        &out,
        &Default::default(),
        None,
        &cancel,
        &mut |_, _, _| {},
    )
    .unwrap();
    assert_eq!(std::fs::read(out.join("file")).unwrap(), b"retained");
}
