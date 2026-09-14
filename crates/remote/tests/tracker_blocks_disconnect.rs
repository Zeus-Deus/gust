//! A file written into a cloud mount blocks a disconnect until the backend
//! confirms it.
//!
//! This is remote rule 1 end-to-end at the policy layer: "cached != uploaded".
//! The app records every cloud destination a finished job wrote as `Cached`,
//! and only a confirmed revision clears it. rclone's VFS queue can be empty
//! the whole time — an empty queue is not a receipt.

use gust_remote::disconnect::{verdict_for, DisconnectVerdict, PendingReport};
use gust_remote::upload::{UploadState, UploadTracker};
use std::path::{Path, PathBuf};

/// The queue rclone reports when it has nothing of its own to do.
fn empty_queue() -> PendingReport {
    PendingReport::Known {
        queued: 0,
        in_progress: 0,
        errored: 0,
    }
}

#[test]
fn a_cached_write_blocks_disconnect_even_when_the_queue_is_empty() {
    let mp = Path::new("/mnt/gust-cloud-test");
    let written = mp.join("quarterly.odt");

    let mut t = UploadTracker::new();
    // Exactly what the app does when a job finishes inside a cloud mount.
    t.set(&written, UploadState::Cached, None);

    match verdict_for("Drive", &empty_queue(), Some(&t), Some(mp), false) {
        DisconnectVerdict::Ask(msg) => {
            assert!(msg.contains("quarterly.odt"), "must name the file: {msg}");
            assert!(msg.contains("not confirmed uploaded"), "got {msg}");
        }
        other => panic!("AN EMPTY QUEUE WAS TREATED AS A RECEIPT for a cached write: {other:?}"),
    }
}

#[test]
fn confirming_the_sent_revision_releases_the_disconnect() {
    let mp = Path::new("/mnt/gust-cloud-test");
    let written = mp.join("quarterly.odt");

    let mut t = UploadTracker::new();
    t.set(&written, UploadState::Uploading, Some("rev-7".to_string()));
    t.confirm(&written, "rev-7")
        .expect("the revision that was sent must confirm");

    assert_eq!(
        verdict_for("Drive", &empty_queue(), Some(&t), Some(mp), false),
        DisconnectVerdict::Safe,
        "a confirmed file must not keep blocking a disconnect"
    );
}

/// Someone else wrote in the meantime: that is a conflict, not a success,
/// and it must still block.
#[test]
fn a_different_backend_revision_is_a_conflict_and_still_blocks() {
    let mp = Path::new("/mnt/gust-cloud-test");
    let written = mp.join("shared-notes.md");

    let mut t = UploadTracker::new();
    t.set(&written, UploadState::Uploading, Some("mine".to_string()));
    let r = t.confirm(&written, "somebody-elses");
    assert!(r.is_err(), "a different revision must not confirm");

    match verdict_for("Drive", &empty_queue(), Some(&t), Some(mp), false) {
        DisconnectVerdict::Ask(msg) => {
            assert!(msg.contains("shared-notes.md"), "got {msg}");
        }
        other => panic!("a conflicted file must still block: {other:?}"),
    }
}

/// Several cached files report a count, not a wall of names.
#[test]
fn several_cached_files_are_summarised() {
    let mp = Path::new("/mnt/gust-cloud-test");
    let mut t = UploadTracker::new();
    for n in ["a.txt", "b.txt", "c.txt"] {
        t.set(&mp.join(n), UploadState::Cached, None);
    }
    match verdict_for("Drive", &empty_queue(), Some(&t), Some(mp), false) {
        DisconnectVerdict::Ask(msg) => {
            assert!(msg.contains("3 files are"), "got {msg}");
        }
        other => panic!("expected Ask, got {other:?}"),
    }
}

/// A write to a LOCAL folder must never be mistaken for cloud work.
#[test]
fn a_local_write_does_not_block_a_cloud_disconnect() {
    let mp = Path::new("/mnt/gust-cloud-test");
    let mut t = UploadTracker::new();
    t.set(
        &PathBuf::from("/home/someone/Documents/local.txt"),
        UploadState::Cached,
        None,
    );
    assert_eq!(
        verdict_for("Drive", &empty_queue(), Some(&t), Some(mp), false),
        DisconnectVerdict::Safe
    );
}
