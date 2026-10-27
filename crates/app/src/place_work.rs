//! Background work that still holds a place open.
//!
//! A transfer, an archive being written or read, or a stop that is still
//! waiting for a remote call to return all keep the files they touch in use.
//! Closing the window or tab, or disconnecting the place underneath, must
//! wait for that work rather than treat "stop requested" as "stopped".
//!
//! Everything here is in memory. The GUI thread asks these questions on every
//! close or disconnect, so none of them may touch the filesystem.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Does any of `paths` lie at or below `mount`?
///
/// Compared component-wise, so `/run/places/nas` never matches
/// `/run/places/nas-backup`.
pub fn touches(paths: &[PathBuf], mount: &Path) -> bool {
    paths.iter().any(|p| p.starts_with(mount))
}

/// Every path a transfer can touch: its sources and its destination folder.
///
/// Both are written into the job's record when it is admitted, before the
/// worker starts planning, so a transfer still walking a remote source is
/// already covered. Clones the record; meant for a close or disconnect
/// decision, not a per-frame poll.
pub fn job_paths(job: &gust_ops::jobs::JobHandle) -> Vec<PathBuf> {
    let rec = job.record();
    let mut paths = rec.planned;
    paths.extend(rec.dest_dir);
    paths
}

/// Each running piece of archive work: its claim id and the paths it uses.
type Claims = Arc<Mutex<Vec<(u64, Vec<PathBuf>)>>>;

/// Archive work this model started that has not finished yet.
///
/// Compress and extract run on their own threads with no job handle, so
/// without this the window and its tabs forgot about them the moment they
/// started. A permissions change has the same shape and is held the same way.
#[derive(Clone, Default)]
pub struct ArchiveWork {
    running: Claims,
}

/// One piece of archive work. Dropping it, including by a panic on the
/// worker, marks the work finished.
pub struct ArchiveClaim {
    id: u64,
    running: Claims,
}

impl ArchiveWork {
    /// Record work over `paths` until the returned claim is dropped.
    pub fn claim(&self, paths: Vec<PathBuf>) -> ArchiveClaim {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        lock(&self.running).push((id, paths));
        ArchiveClaim {
            id,
            running: Arc::clone(&self.running),
        }
    }

    pub fn is_running(&self) -> bool {
        !lock(&self.running).is_empty()
    }

    pub fn touches(&self, mount: &Path) -> bool {
        lock(&self.running)
            .iter()
            .any(|(_, paths)| touches(paths, mount))
    }
}

impl Drop for ArchiveClaim {
    fn drop(&mut self) {
        lock(&self.running).retain(|(id, _)| *id != self.id);
    }
}

/// A poisoned lock still holds a usable list: losing track of running work
/// would be worse than reading it after another thread panicked.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sibling_with_a_longer_name_is_not_inside_the_place() {
        let mount = Path::new("/runtime/gust/places/nas");
        assert!(touches(&[mount.join("photos/a.jpg")], mount));
        assert!(touches(&[mount.to_path_buf()], mount));
        assert!(!touches(
            &[PathBuf::from("/runtime/gust/places/nas-backup/a")],
            mount
        ));
        assert!(!touches(&[], mount));
    }

    #[test]
    fn a_transfer_names_its_source_and_destination_from_admission() {
        // Nothing here exists, so the worker only fails to plan; the paths
        // must already be known before it gets that far.
        let mount = Path::new("/nonexistent-gust-place/nas");
        let up = gust_ops::jobs::start(
            "place-work-up",
            gust_ops::jobs::Request::Copy {
                sources: vec![PathBuf::from("/nonexistent-gust-local/a.bin")],
                dest_dir: mount.join("in"),
            },
            gust_ops::journal::ConflictPolicy::Ask,
        );
        assert!(touches(&job_paths(&up), mount), "destination on the place");
        let mut down = gust_ops::jobs::start(
            "place-work-down",
            gust_ops::jobs::Request::Move {
                sources: vec![mount.join("b.bin")],
                dest_dir: PathBuf::from("/nonexistent-gust-local"),
            },
            gust_ops::journal::ConflictPolicy::Ask,
        );
        assert!(touches(&job_paths(&down), mount), "source on the place");
        down.wait();
        assert!(
            touches(&job_paths(&down), mount),
            "still named once it ended"
        );
        assert!(!touches(
            &job_paths(&down),
            Path::new("/nonexistent-gust-place/other")
        ));
    }

    #[test]
    fn archive_work_is_held_until_its_claim_is_dropped() {
        let work = ArchiveWork::default();
        let mount = Path::new("/places/nas");
        assert!(!work.is_running());

        let first = work.claim(vec![mount.join("in.tar")]);
        let second = work.claim(vec![PathBuf::from("/local/out")]);
        assert!(work.is_running());
        assert!(work.touches(mount));

        drop(first);
        assert!(work.is_running(), "the other claim is still running");
        assert!(!work.touches(mount));

        drop(second);
        assert!(!work.is_running());
    }

    #[test]
    fn a_panicking_worker_still_releases_its_claim() {
        let work = ArchiveWork::default();
        let claim = work.claim(vec![PathBuf::from("/places/nas/a")]);
        let joined = std::thread::spawn(move || {
            let _held = claim;
            panic!("worker failed");
        })
        .join();
        assert!(joined.is_err());
        assert!(!work.is_running());
    }
}
