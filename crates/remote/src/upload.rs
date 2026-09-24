//! Upload state tracking.
//!
//! Rule 1 of the four non-negotiable remote rules: **cached ≠ uploaded**.
//! An empty upload queue is not a receipt for a particular file, so state is
//! tracked per object and only a backend confirmation counts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadState {
    /// Written to the local VFS cache only.
    Cached,
    Queued,
    Uploading,
    /// Backend confirmed this specific object/revision.
    Confirmed,
    Retrying,
    Failed,
    Conflict,
}

impl UploadState {
    /// May this be shown to the user as "uploaded"/"saved to the cloud"?
    pub fn is_uploaded(self) -> bool {
        self == UploadState::Confirmed
    }

    /// Does this file still hold unsaved work that must survive a restart?
    pub fn is_dirty(self) -> bool {
        matches!(
            self,
            UploadState::Cached
                | UploadState::Queued
                | UploadState::Uploading
                | UploadState::Retrying
                | UploadState::Conflict
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            UploadState::Cached => "Saved on this computer",
            UploadState::Queued => "Queued to upload",
            UploadState::Uploading => "Uploading…",
            UploadState::Confirmed => "Uploaded",
            UploadState::Retrying => "Retrying…",
            UploadState::Failed => "Upload failed",
            UploadState::Conflict => "Conflict — not uploaded",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TrackedFile {
    pub path: PathBuf,
    pub state: UploadState,
    /// Content revision this state refers to. Confirmation is bound to a
    /// revision, never just a path (plan §9 external-save conflict contract).
    pub revision: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub struct UploadTracker {
    files: BTreeMap<PathBuf, TrackedFile>,
}

impl UploadTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, path: &Path, state: UploadState, revision: Option<String>) {
        let e = self.files.entry(path.to_path_buf()).or_insert(TrackedFile {
            path: path.to_path_buf(),
            state,
            revision: revision.clone(),
            error: None,
        });
        e.state = state;
        if revision.is_some() {
            e.revision = revision;
        }
    }

    pub fn fail(&mut self, path: &Path, error: impl Into<String>) {
        if let Some(e) = self.files.get_mut(path) {
            e.state = UploadState::Failed;
            e.error = Some(error.into());
        }
    }

    pub fn state_of(&self, path: &Path) -> Option<UploadState> {
        self.files.get(path).map(|f| f.state)
    }

    /// Files with unsaved work. These must block a "safe to close" claim.
    pub fn dirty(&self) -> Vec<&TrackedFile> {
        self.files.values().filter(|f| f.state.is_dirty()).collect()
    }

    /// Confirm an upload — but ONLY for the revision that was sent.
    ///
    /// If the backend reports a different revision, someone else wrote in the
    /// meantime: that is a conflict, not a success.
    pub fn confirm(&mut self, path: &Path, backend_revision: &str) -> Result<(), UploadState> {
        let Some(e) = self.files.get_mut(path) else {
            return Err(UploadState::Failed);
        };
        match &e.revision {
            Some(sent) if sent == backend_revision => {
                e.state = UploadState::Confirmed;
                Ok(())
            }
            Some(_) => {
                e.state = UploadState::Conflict;
                Err(UploadState::Conflict)
            }
            None => {
                // No revision to bind to — cannot claim confirmation.
                e.state = UploadState::Conflict;
                Err(UploadState::Conflict)
            }
        }
    }

    /// Is it safe to close the window without losing work?
    pub fn safe_to_close(&self) -> bool {
        self.dirty().is_empty()
    }

    /// An empty queue proves nothing on its own (plan §3 rule 1).
    pub fn queue_empty_means_uploaded(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_is_never_reported_as_uploaded() {
        assert!(!UploadState::Cached.is_uploaded());
        assert!(!UploadState::Queued.is_uploaded());
        assert!(!UploadState::Uploading.is_uploaded());
        assert!(UploadState::Confirmed.is_uploaded());
        // And the label must not say "uploaded" either.
        assert!(!UploadState::Cached
            .label()
            .to_lowercase()
            .contains("uploaded"));
    }

    #[test]
    fn empty_queue_is_not_a_receipt() {
        let t = UploadTracker::new();
        assert!(!t.queue_empty_means_uploaded());
    }

    #[test]
    fn confirmation_is_bound_to_the_sent_revision() {
        let mut t = UploadTracker::new();
        let p = Path::new("/mnt/remote/doc.txt");
        t.set(p, UploadState::Uploading, Some("rev-1".into()));
        // Backend reports a DIFFERENT revision: someone else wrote.
        let r = t.confirm(p, "rev-2");
        assert_eq!(r, Err(UploadState::Conflict));
        assert_eq!(t.state_of(p), Some(UploadState::Conflict));
        assert!(!t.safe_to_close(), "conflict still holds unsaved work");
    }

    #[test]
    fn matching_revision_confirms() {
        let mut t = UploadTracker::new();
        let p = Path::new("/mnt/remote/doc.txt");
        t.set(p, UploadState::Uploading, Some("rev-9".into()));
        assert!(t.confirm(p, "rev-9").is_ok());
        assert_eq!(t.state_of(p), Some(UploadState::Confirmed));
        assert!(t.safe_to_close());
    }

    #[test]
    fn confirmation_without_a_revision_is_refused() {
        let mut t = UploadTracker::new();
        let p = Path::new("/mnt/remote/x");
        t.set(p, UploadState::Uploading, None);
        assert!(t.confirm(p, "whatever").is_err());
    }

    #[test]
    fn dirty_files_block_closing() {
        let mut t = UploadTracker::new();
        t.set(Path::new("/a"), UploadState::Queued, Some("r".into()));
        assert!(!t.safe_to_close());
        assert_eq!(t.dirty().len(), 1);
        t.confirm(Path::new("/a"), "r").unwrap();
        assert!(t.safe_to_close());
    }

    #[test]
    fn failure_records_the_reason() {
        let mut t = UploadTracker::new();
        let p = Path::new("/b");
        t.set(p, UploadState::Uploading, Some("r".into()));
        t.fail(p, "network unreachable");
        assert_eq!(t.state_of(p), Some(UploadState::Failed));
        assert_eq!(t.dirty().len(), 0, "failed is terminal, not pending");
    }
}
