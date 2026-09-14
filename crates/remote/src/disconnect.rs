//! Whether it is safe to disconnect a cloud mount.
//!
//! Rule 1 of the four non-negotiable remote rules is **cached != uploaded**,
//! and the corollary this module exists for: **unknown != safe**.
//!
//! The disconnect path used to read
//! `if let Ok(p) = vfs_pending(..) { if p.has_pending() { ask } }`, which
//! fails OPEN. When the RC query times out, is refused, or rclone is not
//! installed, upload state is unknown — and the old code silently fell
//! through to unmounting, destroying work that may never have been uploaded.
//! An empty queue is not a receipt, and a query that did not answer is not an
//! empty queue.

use crate::upload::UploadTracker;
use std::path::Path;

/// What the UI must do about a disconnect request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisconnectVerdict {
    /// Nothing outstanding is known and state was readable: proceed.
    Safe,
    /// Answerable by the user: outstanding work is known to exist.
    Ask(String),
    /// Not answerable by a yes/no: state could not be established at all.
    /// Never silently proceed; the user is told what is unknown.
    Refuse(String),
}

impl DisconnectVerdict {
    pub fn may_proceed_without_asking(&self) -> bool {
        *self == DisconnectVerdict::Safe
    }
}

/// Outstanding-upload counts, or the reason they could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingReport {
    Known {
        queued: u64,
        in_progress: u64,
        errored: u64,
    },
    /// The query failed: timeout, refused, rclone missing, bad RC auth.
    Unknown(String),
}

/// Turn a diagnostic reason into something a non-technical person can read.
///
/// The raw string comes from rclone, a socket error or an OS error, and may
/// carry `(os error 2)`. AGENTS.md forbids an errno reaching the user, and a
/// lint enforces it, so the reason is classified rather than interpolated.
/// Anything unrecognised becomes a neutral phrase instead of a confident
/// wrong explanation.
fn plain_reason(why: &str) -> &'static str {
    let w = why.to_lowercase();
    if w.contains("not installed")
        || w.contains("no such file or directory")
        || w.contains("notfound")
    {
        "the helper that reports upload progress is not available"
    } else if w.contains("timed out") || w.contains("timeout") || w.contains("deadline") {
        "it did not answer in time"
    } else if w.contains("refused") || w.contains("connect") {
        "the connection to it was refused"
    } else if w.contains("auth")
        || w.contains("401")
        || w.contains("403")
        || w.contains("forbidden")
    {
        "it refused the request"
    } else {
        "it did not answer"
    }
}

/// Decide whether a cloud mount may be disconnected.
///
/// `tracker` is consulted as well as the VFS queue: the queue describes
/// rclone's own work, while the tracker records per-object state that Gust
/// promised the user. Either one holding dirty work blocks a silent
/// disconnect.
pub fn verdict_for(
    place: &str,
    report: &PendingReport,
    tracker: Option<&UploadTracker>,
    mount_point: Option<&Path>,
    force: bool,
) -> DisconnectVerdict {
    if force {
        // The user has already been told and chose to proceed.
        return DisconnectVerdict::Safe;
    }

    // Files Gust itself still considers dirty, regardless of what the queue
    // says. A tracked file whose state is unknown counts as dirty.
    let dirty_tracked: Vec<String> = tracker
        .map(|t| {
            t.dirty()
                .into_iter()
                .filter(|f| mount_point.map(|mp| f.path.starts_with(mp)).unwrap_or(true))
                .map(|f| {
                    f.path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| f.path.to_string_lossy().into_owned())
                })
                .collect()
        })
        .unwrap_or_default();

    match report {
        PendingReport::Unknown(why) => DisconnectVerdict::Refuse(format!(
            "Gust cannot tell whether everything in {place} has finished \
             uploading, because {}. Disconnecting now could lose work that \
             is only on this computer. Try again in a moment, or reconnect \
             the place and wait for uploads to finish.",
            plain_reason(why)
        )),
        PendingReport::Known {
            queued,
            in_progress,
            errored,
        } => {
            if *errored > 0 {
                let s = if *errored == 1 { "" } else { "s" };
                let verb = if *errored == 1 { "is" } else { "are" };
                return DisconnectVerdict::Refuse(format!(
                    "{errored} file{s} in {place} failed to upload and {verb} \
                     still only on this computer. Disconnecting now would \
                     lose {}. Fix the problem or copy {} somewhere else first.",
                    if *errored == 1 { "it" } else { "them" },
                    if *errored == 1 { "it" } else { "them" }
                ));
            }
            let n = queued.saturating_add(*in_progress);
            if n > 0 {
                let subject = if n == 1 {
                    "1 file is".to_string()
                } else {
                    format!("{n} files are")
                };
                return DisconnectVerdict::Ask(format!(
                    "{subject} still waiting to upload. Disconnecting now \
                     would lose that work."
                ));
            }
            if !dirty_tracked.is_empty() {
                let n = dirty_tracked.len();
                let subject = if n == 1 {
                    format!("{} is", dirty_tracked[0])
                } else {
                    format!("{n} files are")
                };
                return DisconnectVerdict::Ask(format!(
                    "{subject} not confirmed uploaded yet. Disconnecting now \
                     would lose that work."
                ));
            }
            DisconnectVerdict::Safe
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upload::{UploadState, UploadTracker};
    use std::path::PathBuf;

    #[test]
    fn an_unreadable_queue_is_refused_never_treated_as_empty() {
        for why in [
            "connection refused",
            "timed out",
            "rclone is not installed",
            "authentication failed",
        ] {
            let v = verdict_for(
                "Dropbox",
                &PendingReport::Unknown(why.to_string()),
                None,
                None,
                false,
            );
            match &v {
                DisconnectVerdict::Refuse(msg) => {
                    assert!(
                        msg.contains("cannot tell"),
                        "the refusal must say the state is unknown: {msg}"
                    );
                    assert!(!msg.contains("os error"), "errno leaked: {msg}");
                }
                other => panic!("UNKNOWN UPLOAD STATE WAS TREATED AS SAFE ({why}): {other:?}"),
            }
            assert!(!v.may_proceed_without_asking());
        }
    }

    #[test]
    fn a_known_empty_queue_with_nothing_tracked_is_safe() {
        let v = verdict_for(
            "Drive",
            &PendingReport::Known {
                queued: 0,
                in_progress: 0,
                errored: 0,
            },
            None,
            None,
            false,
        );
        assert_eq!(v, DisconnectVerdict::Safe);
    }

    #[test]
    fn pending_uploads_are_answerable_and_count_correctly() {
        let v = verdict_for(
            "Drive",
            &PendingReport::Known {
                queued: 2,
                in_progress: 1,
                errored: 0,
            },
            None,
            None,
            false,
        );
        match v {
            DisconnectVerdict::Ask(m) => {
                assert!(m.contains("3 files are"), "got {m}");
            }
            other => panic!("expected Ask, got {other:?}"),
        }
    }

    #[test]
    fn one_pending_upload_uses_a_singular_verb() {
        let v = verdict_for(
            "Drive",
            &PendingReport::Known {
                queued: 1,
                in_progress: 0,
                errored: 0,
            },
            None,
            None,
            false,
        );
        match v {
            DisconnectVerdict::Ask(m) => assert!(m.contains("1 file is"), "got {m}"),
            other => panic!("expected Ask, got {other:?}"),
        }
    }

    /// A failed upload is NOT a yes/no question: saying yes loses the file.
    #[test]
    fn failed_uploads_are_refused_not_merely_asked() {
        let v = verdict_for(
            "Proton",
            &PendingReport::Known {
                queued: 0,
                in_progress: 0,
                errored: 2,
            },
            None,
            None,
            false,
        );
        match v {
            DisconnectVerdict::Refuse(m) => {
                assert!(m.contains("failed to upload"), "got {m}");
                assert!(m.contains("are"), "plural agreement: {m}");
            }
            other => panic!("a failed upload must be refused, got {other:?}"),
        }
    }

    /// The VFS queue can be empty while Gust still has unconfirmed files.
    /// An empty queue is not a receipt.
    #[test]
    fn an_empty_queue_does_not_override_gusts_own_dirty_state() {
        let mut t = UploadTracker::new();
        t.set(
            &PathBuf::from("/mnt/cloud/report.odt"),
            UploadState::Cached,
            None,
        );
        let v = verdict_for(
            "Drive",
            &PendingReport::Known {
                queued: 0,
                in_progress: 0,
                errored: 0,
            },
            Some(&t),
            Some(Path::new("/mnt/cloud")),
            false,
        );
        match v {
            DisconnectVerdict::Ask(m) => {
                assert!(m.contains("report.odt"), "must name the file: {m}");
                assert!(m.contains("not confirmed uploaded"), "got {m}");
            }
            other => panic!("an unconfirmed file must block a silent disconnect, got {other:?}"),
        }
    }

    #[test]
    fn a_confirmed_file_does_not_block_disconnect() {
        let mut t = UploadTracker::new();
        t.set(
            &PathBuf::from("/mnt/cloud/done.txt"),
            UploadState::Uploading,
            Some("rev-1".to_string()),
        );
        t.confirm(&PathBuf::from("/mnt/cloud/done.txt"), "rev-1")
            .expect("matching revision must confirm");
        let v = verdict_for(
            "Drive",
            &PendingReport::Known {
                queued: 0,
                in_progress: 0,
                errored: 0,
            },
            Some(&t),
            Some(Path::new("/mnt/cloud")),
            false,
        );
        assert_eq!(v, DisconnectVerdict::Safe, "confirmed work must not nag");
    }

    /// Dirty files under a DIFFERENT mount must not block this one.
    #[test]
    fn dirty_files_elsewhere_do_not_block_this_place() {
        let mut t = UploadTracker::new();
        t.set(
            &PathBuf::from("/mnt/other/elsewhere.txt"),
            UploadState::Cached,
            None,
        );
        let v = verdict_for(
            "Drive",
            &PendingReport::Known {
                queued: 0,
                in_progress: 0,
                errored: 0,
            },
            Some(&t),
            Some(Path::new("/mnt/cloud")),
            false,
        );
        assert_eq!(v, DisconnectVerdict::Safe);
    }

    #[test]
    fn force_proceeds_because_the_user_was_already_told() {
        let v = verdict_for(
            "Drive",
            &PendingReport::Unknown("timed out".into()),
            None,
            None,
            true,
        );
        assert_eq!(v, DisconnectVerdict::Safe);
    }

    #[test]
    fn no_user_facing_message_contains_an_errno() {
        let msgs = [
            verdict_for(
                "P",
                &PendingReport::Unknown("No such file or directory (os error 2)".into()),
                None,
                None,
                false,
            ),
            verdict_for(
                "P",
                &PendingReport::Known {
                    queued: 0,
                    in_progress: 0,
                    errored: 1,
                },
                None,
                None,
                false,
            ),
        ];
        for m in msgs {
            let text = match m {
                DisconnectVerdict::Refuse(s) | DisconnectVerdict::Ask(s) => s,
                DisconnectVerdict::Safe => String::new(),
            };
            assert!(
                !text.contains("os error"),
                "an errno reached the user: {text}"
            );
        }
    }
}
