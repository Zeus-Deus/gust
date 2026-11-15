//! Keep a stalled source read from holding destination staging and job ownership.
use crate::engine::{OpError, OpResult};
use gust_core::lister::CancelToken;
use rustix::fd::{AsFd, OwnedFd};
use std::io::{self, Read};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

const CHUNK: usize = 128 * 1024;
const LIMIT: usize = 4;
const POLL: Duration = Duration::from_millis(25);

struct Slot(Arc<AtomicUsize>);
impl Slot {
    fn reserve(active: Arc<AtomicUsize>) -> io::Result<Self> {
        let mut n = active.load(Ordering::Acquire);
        loop {
            if n >= LIMIT {
                return Err(io::Error::other(
                    "Source reads are still waiting for a drive. Reconnect it and try again.",
                ));
            }
            match active.compare_exchange_weak(n, n + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => break,
                Err(now) => n = now,
            }
        }
        Ok(Self(active))
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

struct Pending {
    request: SyncSender<Vec<u8>>,
    result: Receiver<(Vec<u8>, io::Result<usize>)>,
    join: std::thread::JoinHandle<()>,
}
impl Pending {
    fn start(source: impl Read + Send + 'static, slot: Slot) -> io::Result<Self> {
        Self::spawn(source, slot, |work| {
            std::thread::Builder::new()
                .name("gust-source-read".into())
                .spawn(work)
        })
    }

    fn spawn(
        source: impl Read + Send + 'static,
        slot: Slot,
        spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> io::Result<std::thread::JoinHandle<()>>,
    ) -> io::Result<Self> {
        let (request, requests) = mpsc::sync_channel::<Vec<u8>>(1);
        let (answers, result) = mpsc::sync_channel(1);
        let join = spawn(Box::new(move || {
            // Drop the source before releasing admission, including on unwind.
            let _slot = slot;
            let mut source = source;
            while let Ok(mut bytes) = requests.recv() {
                let answer = source.read(&mut bytes);
                if answers.send((bytes, answer)).is_err() {
                    break;
                }
            }
        }))?;
        Ok(Self {
            request,
            result,
            join,
        })
    }

    fn finish(self) -> OpResult<()> {
        // No read is outstanding at successful EOF. Retire this reader before
        // admitting the next file, so short copies do not exhaust closing slots.
        drop(self.request);
        drop(self.result);
        self.join
            .join()
            .map_err(|_| io::Error::other("The source reader could not finish."))?;
        Ok(())
    }

    fn read(&self, buffer: &mut Vec<u8>, cancel: &CancelToken) -> OpResult<usize> {
        if cancel.checkpoint() {
            return Err(OpError::Cancelled);
        }
        self.request
            .send(std::mem::take(buffer))
            .map_err(|_| io::Error::other("The source reader stopped before reading."))?;
        loop {
            if cancel.checkpoint() {
                return Err(OpError::Cancelled);
            }
            match self.result.recv_timeout(POLL) {
                Ok((bytes, answer)) => {
                    *buffer = bytes;
                    if cancel.is_cancelled() {
                        return Err(OpError::Cancelled);
                    }
                    return Ok(answer?);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other("The source reader stopped while reading.").into());
                }
            }
        }
    }
}

/// Pins verified local filesystems for one transfer; a held device ID cannot be recycled.
#[derive(Default)]
pub(crate) struct LocalMounts(std::collections::VecDeque<(u64, OwnedFd)>);
impl LocalMounts {
    fn risky(&mut self, source: &OwnedFd, device: u64) -> bool {
        if let Some(at) = self.0.iter().position(|(cached, _)| *cached == device) {
            let held = self.0.remove(at).expect("position came from this cache");
            self.0.push_back(held);
            return false;
        }
        let local = local_filesystem(crate::anchor::fstype_for_fd(source).as_deref());
        if local {
            if let Ok(pin) = rustix::io::fcntl_dupfd_cloexec(source, 0) {
                if self.0.len() == LIMIT {
                    self.0.pop_front();
                }
                self.0.push_back((device, pin));
            }
        }
        !local
    }
}

fn local_filesystem(kind: Option<&str>) -> bool {
    matches!(
        kind,
        Some(
            "ext2"
                | "ext3"
                | "ext4"
                | "btrfs"
                | "xfs"
                | "f2fs"
                | "tmpfs"
                | "ramfs"
                | "vfat"
                | "msdos"
                | "exfat"
                | "ntfs"
                | "ntfs3"
                | "overlay"
                | "squashfs"
                | "iso9660"
                | "udf"
                | "zfs"
        )
    )
}

pub(crate) struct Reader {
    pending: Option<Pending>,
    pub(crate) buffer: Vec<u8>,
}
impl Reader {
    pub(crate) fn new(source: &OwnedFd, device: u64, mounts: &mut LocalMounts) -> io::Result<Self> {
        let risky = mounts.risky(source, device);
        let pending = if risky {
            static ACTIVE: OnceLock<Arc<AtomicUsize>> = OnceLock::new();
            let slot = Slot::reserve(ACTIVE.get_or_init(|| Arc::new(AtomicUsize::new(0))).clone())?;
            let source = std::fs::File::from(rustix::io::fcntl_dupfd_cloexec(source.as_fd(), 0)?);
            Some(Pending::start(source, slot)?)
        } else {
            None
        };
        Ok(Self {
            pending,
            buffer: vec![0; CHUNK],
        })
    }

    pub(crate) fn read(&mut self, source: &OwnedFd, cancel: &CancelToken) -> OpResult<usize> {
        match &self.pending {
            Some(pending) => pending.read(&mut self.buffer, cancel),
            None => Ok(rustix::io::read(source, &mut self.buffer)?),
        }
    }

    pub(crate) fn finish(&mut self) -> OpResult<()> {
        if let Some(pending) = self.pending.take() {
            pending.finish()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Condvar, Mutex};
    use std::time::Instant;

    #[derive(Default)]
    struct Gate {
        released: Mutex<bool>,
        wake: Condvar,
        entered: AtomicUsize,
        closed: AtomicUsize,
    }
    impl Gate {
        fn release(&self) {
            *self.released.lock().unwrap() = true;
            self.wake.notify_all();
        }
        fn wait_for(&self, value: &AtomicUsize, expected: usize) {
            let until = Instant::now() + Duration::from_secs(3);
            while value.load(Ordering::Acquire) != expected {
                assert!(Instant::now() < until, "worker did not reach {expected}");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
    struct Held(Arc<Gate>);
    impl Read for Held {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            self.0.entered.fetch_add(1, Ordering::Release);
            let mut released = self.0.released.lock().unwrap();
            while !*released {
                released = self.0.wake.wait(released).unwrap();
            }
            bytes[0] = 7;
            Ok(1)
        }
    }
    impl Drop for Held {
        fn drop(&mut self) {
            self.0.closed.fetch_add(1, Ordering::Release);
        }
    }

    #[test]
    fn only_explicit_local_filesystems_may_bypass_read_isolation() {
        for fs in [
            None,
            Some("fuse"),
            Some("fuse.sshfs"),
            Some("fuse.custom"),
            Some("nfs4"),
            Some("cifs"),
            Some("unknown"),
        ] {
            assert!(!local_filesystem(fs), "unsafe local classification: {fs:?}");
        }
        for fs in ["ext4", "btrfs", "tmpfs", "overlay", "exfat"] {
            assert!(local_filesystem(Some(fs)));
        }
    }

    #[test]
    fn local_mount_reuse_holds_its_identity_without_retaining_each_file() {
        use rustix::fd::AsRawFd;
        let path = std::env::temp_dir().join(format!("gust-reader-mount-{}", std::process::id()));
        let file = std::fs::File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        let source: OwnedFd = file.into();
        let mut mounts = LocalMounts::default();
        let device = rustix::fs::fstat(&source).unwrap().st_dev;
        assert!(!mounts.risky(&source, device));
        assert_eq!(mounts.0.len(), 1);
        let held = mounts.0[0].1.as_raw_fd();
        for _ in 0..100 {
            assert!(!mounts.risky(&source, device));
            assert_eq!(mounts.0.len(), 1);
            assert_eq!(mounts.0[0].1.as_raw_fd(), held);
        }
        assert_eq!(
            rustix::fs::fstat(&mounts.0[0].1).unwrap().st_ino,
            rustix::fs::fstat(&source).unwrap().st_ino
        );
        let direct = Reader::new(&source, device, &mut mounts).unwrap();
        assert!(direct.pending.is_none());
    }

    #[test]
    fn cancellation_releases_the_caller_but_keeps_all_four_read_slots_until_close() {
        let active = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Gate::default());
        for expected in 1..=LIMIT {
            let pending =
                Pending::start(Held(gate.clone()), Slot::reserve(active.clone()).unwrap()).unwrap();
            let cancel = CancelToken::new();
            let worker_cancel = cancel.clone();
            let (send, result) = mpsc::channel();
            let worker = std::thread::spawn(move || {
                let result = pending.read(&mut vec![0; CHUNK], &worker_cancel);
                send.send(result).unwrap();
            });
            gate.wait_for(&gate.entered, expected);
            cancel.cancel();
            let outcome = result.recv_timeout(Duration::from_secs(1));
            // Release on a failed cancellation assertion too; never strand test threads.
            if outcome.is_err() {
                gate.release();
            }
            assert!(matches!(outcome.unwrap(), Err(OpError::Cancelled)));
            worker.join().unwrap();
            assert_eq!(active.load(Ordering::Acquire), expected);
        }
        assert!(Slot::reserve(active.clone()).is_err());
        assert_eq!(gate.closed.load(Ordering::Acquire), 0);
        gate.release();
        gate.wait_for(&gate.closed, LIMIT);
        gate.wait_for(&active, 0);
        assert!(Slot::reserve(active).is_ok());
    }

    #[test]
    fn successful_reads_reuse_the_buffer_and_preserve_eof_and_errors() {
        let active = Arc::new(AtomicUsize::new(0));
        let pending = Pending::start(
            io::Cursor::new(vec![1, 2, 3]),
            Slot::reserve(active.clone()).unwrap(),
        )
        .unwrap();
        let mut bytes = vec![0; CHUNK];
        let address = bytes.as_ptr();
        assert_eq!(pending.read(&mut bytes, &CancelToken::new()).unwrap(), 3);
        assert_eq!(&bytes[..3], &[1, 2, 3]);
        assert_eq!(bytes.as_ptr(), address);
        assert_eq!(pending.read(&mut bytes, &CancelToken::new()).unwrap(), 0);
        pending.finish().unwrap();
        assert_eq!(active.load(Ordering::Acquire), 0);
        struct Denied;
        impl Read for Denied {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::ErrorKind::PermissionDenied.into())
            }
        }
        let pending = Pending::start(Denied, Slot::reserve(active).unwrap()).unwrap();
        assert!(
            matches!(pending.read(&mut bytes, &CancelToken::new()), Err(OpError::Io(e)) if e.kind() == io::ErrorKind::PermissionDenied)
        );
    }

    #[test]
    fn consecutive_successful_files_release_admission_before_the_next_file() {
        let active = Arc::new(AtomicUsize::new(0));
        for _ in 0..100 {
            let pending = Pending::start(
                io::Cursor::new(vec![9]),
                Slot::reserve(active.clone()).unwrap(),
            )
            .unwrap();
            let mut bytes = vec![0; CHUNK];
            assert_eq!(pending.read(&mut bytes, &CancelToken::new()).unwrap(), 1);
            assert_eq!(pending.read(&mut bytes, &CancelToken::new()).unwrap(), 0);
            pending.finish().unwrap();
            assert_eq!(active.load(Ordering::Acquire), 0);
        }
    }

    #[test]
    fn spawn_failure_drops_the_source_and_slot_without_a_fallback_read() {
        let active = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Gate::default());
        let result = Pending::spawn(
            Held(gate.clone()),
            Slot::reserve(active.clone()).unwrap(),
            |_work| Err(io::Error::other("injected spawn failure")),
        );
        assert!(result.is_err());
        assert_eq!(active.load(Ordering::Acquire), 0);
        assert_eq!(gate.closed.load(Ordering::Acquire), 1);
        assert_eq!(gate.entered.load(Ordering::Acquire), 0);
    }

    #[test]
    fn cancellation_before_dispatch_never_reads_and_a_pause_is_cancellable() {
        let active = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Gate::default());
        let pending =
            Pending::start(Held(gate.clone()), Slot::reserve(active.clone()).unwrap()).unwrap();
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(matches!(
            pending.read(&mut vec![0; CHUNK], &cancel),
            Err(OpError::Cancelled)
        ));
        assert_eq!(gate.entered.load(Ordering::Acquire), 0);
        let cancel = CancelToken::new();
        cancel.pause();
        let worker_cancel = cancel.clone();
        let worker = std::thread::spawn(move || pending.read(&mut vec![0; CHUNK], &worker_cancel));
        cancel.cancel();
        assert!(matches!(worker.join().unwrap(), Err(OpError::Cancelled)));
        Gate::default().wait_for(&active, 0);
        assert_eq!(gate.entered.load(Ordering::Acquire), 0);
    }
}
