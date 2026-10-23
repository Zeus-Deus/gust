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
    ExtractIntoNew {
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
/// A cloneable in-memory view with no join or Drop cancellation.
#[derive(Clone)]
pub struct ArchiveObserver {
    cancel: CancelToken,
    progress: Arc<Mutex<ArchiveProgress>>,
}
impl ArchiveObserver {
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
}
impl ArchiveHandle {
    pub fn observer(&self) -> ArchiveObserver {
        ArchiveObserver {
            cancel: self.cancel.clone(),
            progress: self.progress.clone(),
        }
    }

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
    start_inner(id.into(), request, None)
}

/// Admission is asynchronous: the worker durably records Planned before any
/// staging/output effect. A journal failure becomes a terminal failed snapshot.
pub fn start_journalled(
    id: impl Into<String>,
    request: ArchiveRequest,
    journal: crate::journal::Journal,
) -> std::io::Result<ArchiveHandle> {
    start_inner(id.into(), request, Some(journal))
}

fn start_inner(
    id: String,
    request: ArchiveRequest,
    journal: Option<crate::journal::Journal>,
) -> std::io::Result<ArchiveHandle> {
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
    let worker = std::thread::Builder::new().name("gust-archive".into()).spawn(move || {
        let mut planned = Job::new(id.clone(), if matches!(&request, ArchiveRequest::Create { .. }) { JobKind::Compress } else { JobKind::Extract });
        match &request {
            ArchiveRequest::Extract { archive, destination, .. } | ArchiveRequest::ExtractIntoNew { archive, destination, .. } => { planned.planned = vec![archive.clone()]; planned.dest_dir = Some(destination.clone()); }
            ArchiveRequest::Create { directory, names, destination, .. } => { planned.planned = names.iter().map(|n| directory.join(n)).collect(); planned.dest_dir = destination.parent().map(Path::to_path_buf); }
        }
        let mut observer = DurablePublication { journal, id: id.clone(), tickets: Vec::new(), shared: shared.clone() };
        let execution = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if let Some(journal) = &observer.journal { journal.record(&planned).map_err(journal_error)?; }
            let mut report = |phase, entries_seen, bytes_done| {
                let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                state.phase = phase; state.entries_seen = entries_seen; state.bytes_done = bytes_done;
            };
            match request {
                ArchiveRequest::Extract { archive, destination, policy, password } => {
                    let extracted = super::io::extract_recorded(&archive, &destination, &policy, password.as_ref(), &token, &mut report, &mut observer)?;
                    let record = extraction_record(id.clone(), &archive, &destination, &extracted);
                    Ok(ArchiveResult { record, extracted: Some(extracted) })
                }
                ArchiveRequest::ExtractIntoNew { archive, destination, policy, password } => {
                    let extracted = super::io::extract_new_recorded(&archive, &destination, &policy, password.as_ref(), &token, &mut report, &mut observer)?;
                    let record = extraction_record(id.clone(), &archive, &destination, &extracted);
                    Ok(ArchiveResult { record, extracted: Some(extracted) })
                }
                ArchiveRequest::Create { directory, names, destination, format, password } => {
                    let created = super::create::create_recorded(super::create::CreateInput { directory: &directory, names: &names, destination: &destination, format, password: password.as_ref() }, &token, &mut report, &mut observer)?;
                    let mut record = extraction_record(id.clone(), &directory, &destination, &created);
                    record.kind = JobKind::Compress;
                    record.dest_dir = destination.parent().map(Path::to_path_buf);
                    record.planned = names.iter().map(|n| directory.join(n)).collect();
                    Ok(ArchiveResult { record, extracted: None })
                },
            }
        }));
        let panicked = execution.is_err();
        let mut result = execution.unwrap_or_else(|_| Err(ArchiveError::Failed("Archive worker stopped unexpectedly; inspect its durable intents and destination before retrying.".into())));
        let terminal = match &result {
            Ok(result) => result.record.clone(),
            Err(error) => {
                let mut record = if let ArchiveError::Partial { receipt, .. } = error {
                    extraction_record(id, planned.planned.first().map(PathBuf::as_path).unwrap_or(Path::new("/")),
                        planned.dest_dir.as_deref().unwrap_or(Path::new("/")), receipt)
                } else {
                    observer.journal.as_ref().and_then(|journal| journal.load().into_iter().rfind(|job| job.id == planned.id)).unwrap_or_else(|| planned.clone())
                };
                record.kind = planned.kind;
                record.state = if panicked { JobState::Committing } else if cancelled(error) { JobState::Cancelled } else { JobState::Failed };
                record
            }
        };
        if let Some(journal) = &observer.journal {
            if let Err(error) = journal.record(&terminal) {
                let cause = journal_error(error);
                result = match result {
                    Ok(ArchiveResult { extracted: Some(receipt), .. }) if !receipt.published.is_empty() => Err(ArchiveError::Partial { receipt, cause: Box::new(cause) }),
                    Ok(ArchiveResult { record, extracted: None }) if !record.subtree_identities.is_empty() => Err(ArchiveError::Partial { receipt: Extracted { files: record.entries.len(), published: record.subtree_identities, ..Default::default() }, cause: Box::new(cause) }),
                    Err(ArchiveError::Partial { receipt, .. }) => Err(ArchiveError::Partial { receipt, cause: Box::new(cause) }),
                    _ => Err(cause),
                };
            }
        }
        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
        state.phase = match &result {
            Ok(result) => { state.files_done = result.extracted.as_ref().map_or(1, |e| e.files); ArchivePhase::Confirmed }
            Err(error) if cancelled(error) => ArchivePhase::Cancelled,
            Err(ArchiveError::Partial { receipt, .. }) => { state.files_done = receipt.files; ArchivePhase::Failed }
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

fn cancelled(error: &ArchiveError) -> bool {
    match error {
        ArchiveError::Cancelled => true,
        ArchiveError::Partial { cause, .. } => cancelled(cause),
        _ => false,
    }
}
fn journal_error(error: std::io::Error) -> ArchiveError {
    ArchiveError::Failed(format!("Archive journal could not be updated: {error}"))
}
struct DurablePublication {
    journal: Option<crate::journal::Journal>,
    id: String,
    tickets: Vec<bool>,
    shared: Arc<Mutex<ArchiveProgress>>,
}
impl super::io::PublicationObserver for DurablePublication {
    fn intent(
        &mut self,
        source: &Path,
        destination: &Path,
        expected: Option<crate::anchor::FileIdentity>,
    ) -> Result<usize, ArchiveError> {
        use std::os::unix::ffi::OsStrExt;
        let index = self.tickets.len();
        if let Some(journal) = &self.journal {
            journal
                .record_update(&crate::journal::JournalUpdate::ArchiveIntent {
                    id: self.id.clone(),
                    index,
                    source: source.as_os_str().as_bytes().to_vec(),
                    destination: destination.as_os_str().as_bytes().to_vec(),
                    expected: expected.map(Into::into),
                })
                .map_err(journal_error)?;
        }
        self.tickets.push(expected.is_some());
        Ok(index)
    }
    fn committed(
        &mut self,
        index: usize,
        identity: crate::anchor::FileIdentity,
    ) -> Result<(), ArchiveError> {
        if let Some(journal) = &self.journal {
            journal
                .record_update(&crate::journal::JournalUpdate::ArchivePublished {
                    id: self.id.clone(),
                    index,
                    identity: identity.into(),
                })
                .map_err(journal_error)?;
        }
        if self.tickets.get(index) == Some(&true) {
            self.shared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .files_done += 1;
        }
        Ok(())
    }
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
