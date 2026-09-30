//! Live folder updates.
//!
//! One inotify watch on the folder on screen, on its own thread. It reports
//! that the folder's CONTENTS changed (something appeared, disappeared, was
//! renamed, or finished being written) or that the folder itself is GONE
//! (deleted, moved away, unmounted). It never reports which file: the
//! listing is re-read anyway, and a per-file diff would be a second source
//! of truth that could disagree with the first.
//!
//! Two rules keep it cheap:
//! - Changes are coalesced. Unpacking an archive into the folder is one or
//!   two refreshes, not one per file: the report waits for a short quiet
//!   gap, but never longer than a ceiling, so a folder that changes
//!   continuously still refreshes a few times a second rather than never.
//! - Writes are not changes. A file being downloaded or copied in chunks
//!   produces no reports until it is closed (`IN_CLOSE_WRITE`), so a big
//!   transfer does not relist the folder hundreds of times.
//!
//! Only for local folders. Network and FUSE mounts can stall and inotify
//! cannot see changes made on the other end anyway; those keep Ctrl+R.

use rustix::event::{eventfd, poll, EventfdFlags, PollFd, PollFlags, Timespec};
use rustix::fs::inotify;
use rustix::io::Errno;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::path::Path;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// What happened to the watched folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Something inside it changed: list it again.
    Contents,
    /// The folder itself was deleted, moved away or unmounted. The watch
    /// has ended; nothing further will be reported.
    Gone,
}

/// Quiet gap that ends a burst of changes.
const SETTLE: Duration = Duration::from_millis(120);
/// A burst that never goes quiet still reports this often.
const CEILING: Duration = Duration::from_millis(600);

/// A running watch. Dropping it stops the thread and waits for it.
pub struct DirWatch {
    stop: OwnedFd,
    thread: Option<JoinHandle<()>>,
}

impl DirWatch {
    /// Watch `dir` and call `on_change` (on the watch thread) for every
    /// coalesced change. Fails if the folder cannot be watched — it does
    /// not exist, is not a folder, or the inotify limits are exhausted — in
    /// which case the caller simply has no live updates.
    pub fn start(
        dir: &Path,
        on_change: impl FnMut(Change) + Send + 'static,
    ) -> io::Result<DirWatch> {
        let ino = inotify::init(inotify::CreateFlags::CLOEXEC | inotify::CreateFlags::NONBLOCK)?;
        use inotify::WatchFlags as W;
        // The watch is added HERE, not on the thread, so a change made the
        // moment `start` returns is never missed and errors reach the caller.
        inotify::add_watch(
            &ino,
            dir,
            W::CREATE
                | W::DELETE
                | W::MOVED_FROM
                | W::MOVED_TO
                | W::CLOSE_WRITE
                | W::ATTRIB
                | W::DELETE_SELF
                | W::MOVE_SELF
                | W::ONLYDIR
                // An unlinked file still open elsewhere must not keep
                // reporting into a folder it is no longer part of.
                | W::EXCL_UNLINK,
        )?;
        let stop = eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)?;
        let stop_thread = stop.try_clone()?;
        let thread = std::thread::Builder::new()
            .name("gust-watch".into())
            .spawn(move || run(ino, stop_thread, on_change))?;
        Ok(DirWatch {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for DirWatch {
    fn drop(&mut self) {
        let _ = rustix::io::write(&self.stop, &1u64.to_ne_bytes());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// What one batch of raw events amounts to.
#[derive(Default)]
struct Batch {
    contents: bool,
    gone: bool,
}

/// Drain every queued event without blocking.
fn read_events(ino: &OwnedFd, buf: &mut [MaybeUninit<u8>]) -> Batch {
    use inotify::ReadFlags as R;
    let mut batch = Batch::default();
    let mut reader = inotify::Reader::new(ino, buf);
    loop {
        match reader.next() {
            Ok(ev) => {
                let f = ev.events();
                if f.intersects(R::DELETE_SELF | R::MOVE_SELF | R::UNMOUNT | R::IGNORED) {
                    batch.gone = true;
                } else {
                    // Includes QUEUE_OVERFLOW: events were lost, so relist.
                    batch.contents = true;
                }
            }
            Err(Errno::AGAIN) | Err(Errno::INTR) => break,
            Err(_) => {
                // An unreadable inotify fd means the watch is broken: say
                // Contents once so the view is at least current, then stop.
                batch.contents = true;
                batch.gone = true;
                break;
            }
        }
    }
    batch
}

fn run(ino: OwnedFd, stop: OwnedFd, mut on_change: impl FnMut(Change)) {
    let mut buf = [MaybeUninit::<u8>::uninit(); 16 * 1024];
    // When the current burst started, if one is pending.
    let mut pending_since: Option<Instant> = None;
    let mut last_event = Instant::now();

    loop {
        // Block until something happens, or until a pending burst is due.
        let timeout = pending_since.map(|since| {
            let quiet_due = last_event + SETTLE;
            let ceiling_due = since + CEILING;
            quiet_due
                .min(ceiling_due)
                .saturating_duration_since(Instant::now())
        });
        let ts = timeout.map(|d| Timespec {
            tv_sec: d.as_secs() as _,
            tv_nsec: d.subsec_nanos() as _,
        });
        let mut fds = [
            PollFd::new(&ino, PollFlags::IN),
            PollFd::new(&stop, PollFlags::IN),
        ];
        match poll(&mut fds, ts.as_ref()) {
            Ok(_) | Err(Errno::INTR) => {}
            Err(_) => return,
        }
        if !fds[1].revents().is_empty() {
            return;
        }
        if !fds[0].revents().is_empty() {
            let batch = read_events(&ino, &mut buf);
            if batch.gone {
                on_change(Change::Gone);
                return;
            }
            if batch.contents {
                last_event = Instant::now();
                pending_since.get_or_insert(last_event);
            }
        }
        if let Some(since) = pending_since {
            let now = Instant::now();
            if now >= last_event + SETTLE || now >= since + CEILING {
                pending_since = None;
                on_change(Change::Contents);
            }
        }
    }
}

/// One step of turning the rows on screen into a relisted folder.
///
/// Applied IN ORDER: every `Remove` comes first, highest row first, so each
/// index is still valid when it is applied; then every `Insert`, lowest row
/// first, where `at` is also the entry's row in the new listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowEdit {
    Remove(usize),
    Insert { at: usize, from_new: usize },
}

/// The fewest removals and insertions that turn `old` into `new`, both
/// lists of unique names (a folder cannot hold a name twice).
///
/// Rows that are in both lists and already in the right relative order stay
/// where they are — found as the longest increasing run of their new
/// positions, O(n log n). A live refresh uses this instead of a model reset
/// so the list keeps its scroll position, cursor and hover while another
/// program changes the folder.
pub fn row_edits(old: &[std::ffi::OsString], new: &[std::ffi::OsString]) -> Vec<RowEdit> {
    use std::collections::HashMap;
    let pos_new: HashMap<&std::ffi::OsStr, usize> = new
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_os_str(), i))
        .collect();

    // For each old row that survives, its position in `new`.
    let common: Vec<(usize, usize)> = old
        .iter()
        .enumerate()
        .filter_map(|(i, n)| pos_new.get(n.as_os_str()).map(|&j| (i, j)))
        .collect();

    // Longest strictly increasing subsequence of the new positions.
    let mut tails: Vec<usize> = Vec::new(); // index into `common`
    let mut prev: Vec<Option<usize>> = vec![None; common.len()];
    for (k, &(_, j)) in common.iter().enumerate() {
        let at = tails.partition_point(|&t| common[t].1 < j);
        if at > 0 {
            prev[k] = Some(tails[at - 1]);
        }
        if at == tails.len() {
            tails.push(k);
        } else {
            tails[at] = k;
        }
    }
    let mut stay_old = vec![false; old.len()];
    let mut stay_new = vec![false; new.len()];
    let mut cur = tails.last().copied();
    while let Some(k) = cur {
        stay_old[common[k].0] = true;
        stay_new[common[k].1] = true;
        cur = prev[k];
    }

    let mut edits: Vec<RowEdit> = (0..old.len())
        .rev()
        .filter(|&i| !stay_old[i])
        .map(RowEdit::Remove)
        .collect();
    edits.extend(
        (0..new.len())
            .filter(|&j| !stay_new[j])
            .map(|j| RowEdit::Insert { at: j, from_new: j }),
    );
    edits
}
