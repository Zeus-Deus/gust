//! Tracked, cancellable archive workers. No frontend or Qt dependencies.
use super::io::{ArchiveError, ArchiveFormat, ArchivePassword, Extracted};
use super::ExtractPolicy;
use crate::journal::{Job, JobKind, JobState, JournalEntry};
use gust_core::lister::CancelToken;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub enum ArchiveRequest {
    Extract {
        archive: PathBuf,
        destination: PathBuf,
        policy: ExtractPolicy,
        password: Option<ArchivePassword>,
    },
    Create {
        directory: PathBuf,
        names: Vec<OsString>,
        destination: PathBuf,
        format: ArchiveFormat,
        password: Option<ArchivePassword>,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchivePhase {
    Planned,
    Reading,
    Extracting,
    Creating,
    Publishing,
    Confirmed,
    Cancelled,
    Failed,
}
impl ArchivePhase {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Confirmed | Self::Cancelled | Self::Failed)
    }
}
#[derive(Debug, Clone)]
pub struct ArchiveResult {
    pub record: Job,
    pub extracted: Option<Extracted>,
}
#[derive(Debug, Clone)]
pub struct ArchiveProgress {
    pub id: String,
    pub phase: ArchivePhase,
    pub entries_seen: usize,
    pub files_done: usize,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
    pub result: Option<Result<ArchiveResult, ArchiveError>>,
}

pub struct ArchiveHandle {
    cancel: CancelToken,
    progress: Arc<Mutex<ArchiveProgress>>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl ArchiveHandle {
    pub fn snapshot(&self) -> ArchiveProgress {
        self.progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
    pub fn is_finished(&self) -> bool {
        self.snapshot().phase.is_terminal()
    }
    /// Worker-only. GUI code polls snapshots and must wait for a terminal phase
    /// before treating cancellation as acknowledged or closing the owner.
    pub fn wait(&mut self) -> ArchiveProgress {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.snapshot()
    }
}
impl Drop for ArchiveHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub fn start(id: impl Into<String>, request: ArchiveRequest) -> std::io::Result<ArchiveHandle> {
    let id = id.into();
    let progress = Arc::new(Mutex::new(ArchiveProgress {
        id: id.clone(),
        phase: ArchivePhase::Planned,
        entries_seen: 0,
        files_done: 0,
        bytes_done: 0,
        bytes_total: None,
        result: None,
    }));
    let cancel = CancelToken::new();
    let shared = progress.clone();
    let token = cancel.clone();
    let worker = std::thread::Builder::new()
        .name("gust-archive".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut report = |phase, entries_seen, bytes_done| {
                    let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                    state.phase = phase;
                    state.entries_seen = entries_seen;
                    state.bytes_done = bytes_done;
                };
                match request {
                    ArchiveRequest::Extract {
                        archive,
                        destination,
                        policy,
                        password,
                    } => {
                        let extracted = super::io::extract_with(
                            &archive,
                            &destination,
                            &policy,
                            password.as_ref(),
                            &token,
                            &mut report,
                        )?;
                        let record = extraction_record(id, &archive, &destination, &extracted);
                        Ok(ArchiveResult {
                            record,
                            extracted: Some(extracted),
                        })
                    }
                    ArchiveRequest::Create { .. } => {
                        Err(ArchiveError::Unsupported("archive creation format".into()))
                    }
                }
            }))
            .unwrap_or_else(|_| {
                Err(ArchiveError::Failed(
                    "Archive worker stopped unexpectedly; check the destination before retrying."
                        .into(),
                ))
            });
            let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
            state.phase = match &result {
                Ok(result) => {
                    state.files_done = result.extracted.as_ref().map_or(1, |e| e.files);
                    ArchivePhase::Confirmed
                }
                Err(ArchiveError::Cancelled) => ArchivePhase::Cancelled,
                Err(ArchiveError::Partial { receipt, cause }) => {
                    state.files_done = receipt.files;
                    if matches!(cause.as_ref(), ArchiveError::Cancelled) {
                        ArchivePhase::Cancelled
                    } else {
                        ArchivePhase::Failed
                    }
                }
                Err(_) => ArchivePhase::Failed,
            };
            state.result = Some(result);
        })?;
    Ok(ArchiveHandle {
        cancel,
        progress,
        worker: Some(worker),
    })
}

fn extraction_record(id: String, archive: &Path, destination: &Path, extracted: &Extracted) -> Job {
    let mut record = Job::new(id, JobKind::Extract);
    record.dest_dir = Some(destination.to_path_buf());
    record.planned = vec![archive.to_path_buf()];
    record.state = JobState::Confirmed;
    record.subtree_identities = extracted.published.clone();
    // Only roots of disjoint created trees become entries; overlapping entries
    // would make an inverse attempt the same subtree twice.
    let created: std::collections::BTreeSet<_> = extracted
        .published
        .iter()
        .map(|(path, _)| path.clone())
        .collect();
    for (path, identity) in &extracted.published {
        if path
            .ancestors()
            .skip(1)
            .any(|ancestor| created.contains(ancestor))
        {
            continue;
        }
        record.entries.push(JournalEntry {
            source: archive.to_path_buf(),
            destination: Some(path.clone()),
            source_identity: Some(*identity),
            preserved: None,
            done: true,
            error: None,
        });
    }
    record
}
