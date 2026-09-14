//! The journal must be DURABLE, not just in memory.
//!
//! `jobs::run` carries the comment "Journal BEFORE judging success: a crash
//! must leave a record of what was attempted, not silence" — but the record
//! it updates is an `Arc<Mutex<Job>>` that dies with the process. Nothing is
//! written to disk, `Journal::new` has no caller in the application, and
//! `Journal::unfinished()` (the restart-recovery entry point) is never
//! consulted. So after a crash mid-copy there is no record, and the staging
//! debris the crash left cannot be attributed to any operation.
//!
//! These tests drive the real `jobs::start` and then read the journal file
//! back from disk with a FRESH `Journal` instance, which is what a restarted
//! process does. An in-memory-only implementation fails them.

use gust_ops::jobs;
use gust_ops::journal::{ConflictPolicy, JobKind, JobState, Journal};
use std::fs;
use std::path::PathBuf;

fn fixture(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "gust-journal-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn a_completed_copy_leaves_a_durable_record_a_restart_can_read() {
    let root = fixture("durable");
    let journal_path = root.join("journal.ndjson");
    fs::write(root.join("src.txt"), b"payload").unwrap();
    fs::create_dir_all(root.join("dest")).unwrap();

    let mut h = jobs::start_journalled(
        "durable-1",
        jobs::Request::Copy {
            sources: vec![root.join("src.txt")],
            dest_dir: root.join("dest"),
        },
        ConflictPolicy::Replace,
        Some(Journal::new(&journal_path)),
    );
    let p = h.wait();
    assert_eq!(p.state, JobState::Confirmed, "the copy should succeed");

    // A RESTARTED process sees only what reached the disk.
    assert!(
        journal_path.is_file(),
        "no journal file was written: a crash would leave no record at all"
    );
    let reloaded = Journal::new(&journal_path).load();
    assert!(
        !reloaded.is_empty(),
        "journal file exists but holds no records"
    );
    let mine: Vec<_> = reloaded.iter().filter(|j| j.id == "durable-1").collect();
    assert!(!mine.is_empty(), "this job left no record on disk");
    let last = mine.last().unwrap();
    assert_eq!(last.kind, JobKind::Copy);
    assert_eq!(
        last.state,
        JobState::Confirmed,
        "the terminal state must be the one persisted, not an earlier one"
    );
    assert!(
        last.entries
            .iter()
            .any(|e| e.source == root.join("src.txt")),
        "the record must name what was operated on"
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn a_record_is_on_disk_before_the_operation_is_judged_complete() {
    // The point of journalling first is that a crash DURING the work still
    // leaves evidence. Enough files that the job is still running when the
    // journal is first inspected.
    let root = fixture("before");
    let journal_path = root.join("journal.ndjson");
    let src = root.join("tree");
    fs::create_dir_all(&src).unwrap();
    for i in 0..800 {
        fs::write(src.join(format!("f{i:04}.bin")), vec![b'x'; 2048]).unwrap();
    }
    fs::create_dir_all(root.join("dest")).unwrap();

    let mut h = jobs::start_journalled(
        "before-1",
        jobs::Request::Copy {
            sources: vec![src.clone()],
            dest_dir: root.join("dest"),
        },
        ConflictPolicy::Replace,
        Some(Journal::new(&journal_path)),
    );

    // Poll for a non-terminal record appearing on disk while work continues.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let mut saw_running_on_disk = false;
    while std::time::Instant::now() < deadline {
        if journal_path.is_file() {
            let recs = Journal::new(&journal_path).load();
            if recs
                .iter()
                .any(|j| j.id == "before-1" && !j.state.is_terminal())
            {
                saw_running_on_disk = true;
                break;
            }
        }
        if h.is_finished() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let p = h.wait();
    eprintln!(
        "final state {:?}, saw_running_on_disk={saw_running_on_disk}",
        p.state
    );

    assert!(
        saw_running_on_disk,
        "no in-progress record ever reached the disk: a crash mid-copy would \
         leave staging debris with nothing to attribute it to"
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn an_interrupted_job_is_reported_as_unfinished_after_a_restart() {
    // Simulate what a restart sees: a record written as Running that never
    // reached a terminal state, because the process died.
    let root = fixture("unfinished");
    let journal_path = root.join("journal.ndjson");
    let j = Journal::new(&journal_path);

    let mut job = gust_ops::journal::Job::new("crashed-1", JobKind::Copy);
    job.state = JobState::Running;
    job.entries.push(gust_ops::journal::JournalEntry {
        source: root.join("a.txt"),
        destination: Some(root.join("dest/a.txt")),
        source_identity: None,
        preserved: None,
        done: false,
        error: None,
    });
    j.record(&job).unwrap();

    // A fresh instance, as a restarted process would use.
    let unfinished = Journal::new(&journal_path).unfinished();
    assert_eq!(
        unfinished.len(),
        1,
        "a non-terminal record must be offered for recovery, got {unfinished:?}"
    );
    assert_eq!(unfinished[0].id, "crashed-1");

    // And once it reaches a terminal state, it must no longer be offered.
    let mut done = unfinished[0].clone();
    done.state = JobState::Confirmed;
    Journal::new(&journal_path).record(&done).unwrap();
    assert!(
        Journal::new(&journal_path).unfinished().is_empty(),
        "a completed job must not be offered for recovery forever"
    );
    fs::remove_dir_all(&root).ok();
}

#[test]
fn the_journal_file_is_private() {
    let root = fixture("perms");
    let journal_path = root.join("journal.ndjson");
    let j = Journal::new(&journal_path);
    let mut job = gust_ops::journal::Job::new("p-1", JobKind::Copy);
    job.state = JobState::Confirmed;
    j.record(&job).unwrap();

    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(&journal_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "the journal names the user's paths and must not be readable by others"
    );
    fs::remove_dir_all(&root).ok();
}
