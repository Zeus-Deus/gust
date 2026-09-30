//! After a crash, Gust says at start-up what was interrupted — once.
//!
//! The journal records every state of every job durably. These tests pin
//! down what the start-up notice may report: only jobs whose process is
//! really gone, with enough detail to act on, and never twice.

use gust_ops::journal::{Job, JobKind, JobState, Journal, JournalEntry};
use std::path::PathBuf;

fn journal(tag: &str) -> Journal {
    let p = std::env::temp_dir().join(format!("gust-crash-{tag}-{}.ndjson", std::process::id()));
    let _ = std::fs::remove_file(&p);
    Journal::new(p)
}

/// The pid of a process that has certainly exited.
fn dead_pid() -> u32 {
    let mut c = std::process::Command::new("true").spawn().unwrap();
    let pid = c.id();
    c.wait().unwrap();
    pid
}

fn running_copy(id: &str, pid: u32, done_items: usize) -> Job {
    let mut j = Job::new(id, JobKind::Copy);
    j.pid = pid;
    j.state = JobState::Running;
    j.dest_dir = Some(PathBuf::from("/media/usb"));
    j.planned = vec![
        PathBuf::from("/home/u/holiday.mkv"),
        PathBuf::from("/home/u/notes.pdf"),
        PathBuf::from("/home/u/photos"),
    ];
    for p in j.planned.clone().into_iter().take(done_items) {
        j.entries.push(JournalEntry {
            destination: Some(PathBuf::from("/media/usb").join(p.file_name().unwrap())),
            source: p,
            source_identity: None,
            preserved: None,
            done: true,
            error: None,
        });
    }
    j
}

#[test]
fn a_job_whose_process_died_is_reported() {
    let jr = journal("dead");
    jr.record(&running_copy("a", dead_pid(), 1)).unwrap();
    let got = jr.interrupted();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].id, "a");
}

#[test]
fn a_job_still_running_in_a_live_window_is_not_reported() {
    // Another Gust window is copying right now. Calling that "interrupted"
    // would be false, and acting on it would fight a live copy.
    let jr = journal("live");
    jr.record(&running_copy("b", std::process::id(), 1))
        .unwrap();
    assert!(jr.interrupted().is_empty());
}

#[test]
fn a_finished_or_cancelled_job_is_not_reported() {
    let jr = journal("done");
    let pid = dead_pid();
    let mut a = running_copy("a", pid, 3);
    jr.record(&a).unwrap();
    a.state = JobState::Confirmed;
    jr.record(&a).unwrap();
    let mut c = running_copy("c", pid, 0);
    jr.record(&c).unwrap();
    c.state = JobState::Cancelled;
    jr.record(&c).unwrap();
    assert!(jr.interrupted().is_empty());
}

#[test]
fn the_report_says_what_how_far_and_where() {
    let jr = journal("words");
    jr.record(&running_copy("a", dead_pid(), 1)).unwrap();
    let got = jr.interrupted();
    let i = &got[0];
    assert_eq!(i.kind, JobKind::Copy);
    assert_eq!(i.finished, 1);
    assert_eq!(i.total, 3);
    assert_eq!(
        i.dest_dir.as_deref(),
        Some(std::path::Path::new("/media/usb"))
    );
    assert_eq!(
        i.sentence(),
        "Copying holiday.mkv and 2 more to usb was interrupted: 1 of 3 finished."
    );
}

#[test]
fn a_job_that_crashed_before_its_first_item_still_knows_where_it_was_going() {
    let jr = journal("early");
    jr.record(&running_copy("a", dead_pid(), 0)).unwrap();
    let i = &jr.interrupted()[0];
    assert_eq!(i.finished, 0);
    assert_eq!(
        i.dest_dir.as_deref(),
        Some(std::path::Path::new("/media/usb"))
    );
    assert_eq!(
        i.sentence(),
        "Copying holiday.mkv and 2 more to usb was interrupted: nothing had finished."
    );
}

#[test]
fn once_acknowledged_it_is_never_reported_again() {
    let jr = journal("ack");
    jr.record(&running_copy("a", dead_pid(), 1)).unwrap();
    jr.record(&running_copy("z", dead_pid(), 2)).unwrap();
    jr.acknowledge("a").unwrap();
    let left: Vec<String> = jr.interrupted().into_iter().map(|i| i.id).collect();
    assert_eq!(left, vec!["z".to_string()]);
    // And it survives a restart: a fresh Journal over the same file agrees.
    let again = Journal::new(jr.path().to_path_buf());
    assert_eq!(again.interrupted().len(), 1);
}

#[test]
fn a_record_from_an_earlier_boot_is_interrupted_even_if_the_pid_is_reused() {
    // After a reboot the old pid may belong to an unrelated process.
    let jr = journal("boot");
    let mut j = running_copy("a", std::process::id(), 1);
    j.boot = "some-earlier-boot".into();
    jr.record(&j).unwrap();
    assert_eq!(jr.interrupted().len(), 1);
}

#[test]
fn a_job_records_its_plan_the_moment_it_starts() {
    // Written by the real job runner, not by hand: the destination and the
    // planned items must be durable before any byte is copied, or a crash
    // during the first large file leaves a record that cannot say where.
    let root = std::env::temp_dir().join(format!("gust-crash-plan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("dst")).unwrap();
    std::fs::write(root.join("src/a.txt"), b"a").unwrap();
    let jr = Journal::new(root.join("journal.ndjson"));
    let mut h = gust_ops::jobs::start_journalled(
        "plan",
        gust_ops::jobs::Request::Copy {
            sources: vec![root.join("src/a.txt")],
            dest_dir: root.join("dst"),
        },
        gust_ops::journal::ConflictPolicy::Ask,
        Some(Journal::new(root.join("journal.ndjson"))),
    );
    h.wait();
    let first = jr.load().into_iter().next().expect("a record");
    assert_eq!(first.dest_dir.as_deref(), Some(root.join("dst").as_path()));
    assert_eq!(first.planned, vec![root.join("src/a.txt")]);
    assert_eq!(first.pid, std::process::id());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_job_that_stopped_at_a_name_clash_ended_and_is_not_a_crash() {
    // The runner returns after recording Conflict: the worker ended on
    // purpose and told the user at the time. Found live: a stale notice
    // blamed a crash for a paste that had simply stopped at a clash.
    let jr = journal("clash");
    let mut j = running_copy("a", dead_pid(), 0);
    jr.record(&j).unwrap();
    j.state = JobState::Conflict;
    jr.record(&j).unwrap();
    assert!(jr.interrupted().is_empty());
}

#[test]
fn a_record_written_before_owners_were_recorded_is_not_reported() {
    // Journals from older Gust builds have no pid. Whether that process
    // crashed or finished long ago cannot be known, and a notice that may be
    // weeks stale on the first start after an upgrade is noise, not help.
    let jr = journal("legacy");
    let mut j = running_copy("a", 0, 1);
    j.boot = String::new();
    jr.record(&j).unwrap();
    assert!(jr.interrupted().is_empty());
}
