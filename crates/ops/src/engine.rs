//! Copy / move / delete engine.
//!
//! Every destructive step goes through: stage → verify → commit → (only then)
//! remove source. Special files are classified, never copied blindly.

use crate::anchor::{resolve_parent, DirHandle, FileIdentity};
use crate::journal::{ConflictPolicy, Job, JobKind, JobState, JournalEntry, RecordedIdentity};
use gust_core::lister::CancelToken;
use rustix::fs::{Mode, OFlags};
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileClass {
    Regular,
    Directory,
    Symlink,
    /// FIFO, socket, block/char device — refused by default.
    Special,
}

pub fn classify(st: &rustix::fs::Stat) -> FileClass {
    let ft = st.st_mode & 0o170000;
    match ft {
        0o100000 => FileClass::Regular,
        0o040000 => FileClass::Directory,
        0o120000 => FileClass::Symlink,
        _ => FileClass::Special,
    }
}

#[derive(Debug)]
pub enum OpError {
    Io(io::Error),
    /// Refused on policy grounds, with the reason shown to the user.
    Refused(String),
    Conflict(PathBuf),
    Cancelled,
    /// The record that lets Gust find and clean up after an interrupted
    /// copy could not be saved. Nothing is written without it.
    Recovery(io::Error),
}

/// Why the recovery record (journal or staging claim) could not be saved,
/// in words. It lives in the user's own state folder, not at the
/// destination, so "the destination is full" would be the wrong advice.
pub fn recovery_reason(e: &io::Error) -> String {
    use io::ErrorKind::*;
    match e.kind() {
        StorageFull => "the disk holding your home folder is full".into(),
        PermissionDenied => "Gust's state folder is not writable".into(),
        ReadOnlyFilesystem => "the disk holding your home folder is read-only".into(),
        _ => match e.raw_os_error() {
            Some(28) => "the disk holding your home folder is full".into(),
            Some(122) => "you are over your disk quota".into(),
            Some(30) => "the disk holding your home folder is read-only".into(),
            _ => plain_io_message(e),
        },
    }
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpError::Recovery(e) => write!(
                f,
                "Gust could not save its recovery record ({}), so it wrote nothing more.",
                recovery_reason(e)
            ),
            OpError::Io(e) => write!(f, "{}", plain_io_message(e)),
            OpError::Refused(r) => write!(f, "refused: {r}"),
            OpError::Conflict(p) => write!(f, "conflict at {}", p.display()),
            OpError::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// Say what went wrong in words, not in errno.
///
/// Gust is built for somebody who does not use a terminal, so
/// "No space left on device (os error 28)" is not an acceptable thing to put
/// in front of them. Every message here says what happened and, where there
/// is one, what to do about it. Anything unrecognised falls back to the OS
/// text rather than inventing a friendlier lie.
/// [`plain_io_message`] for a failure while writing to `dest`. On a network
/// place (sshfs, SMB, rclone...) an I/O error almost always means the
/// connection went, and "the disk reported a read/write error" sends the
/// user looking for a broken disk that does not exist.
pub fn plain_io_message_at(e: &io::Error, dest: &Path) -> String {
    let fstype = gust_core::stall::fstype_for(dest);
    plain_io_message_on(e, dest, fstype.as_deref())
}

/// The decision behind [`plain_io_message_at`], with the filesystem type
/// passed in so it can be tested without a real mount.
pub fn plain_io_message_on(e: &io::Error, dest: &Path, fstype: Option<&str>) -> String {
    // EIO, ENOTCONN, ECONNABORTED, ECONNRESET, ETIMEDOUT.
    let lost = matches!(
        e.raw_os_error(),
        Some(5) | Some(107) | Some(103) | Some(104) | Some(110)
    );
    if lost && fstype.is_some_and(gust_core::stall::can_stall) {
        return format!(
            "The connection to {} was lost. Nothing was replaced; reconnect and copy again.",
            gust_core::stall::place_name_for(dest)
        );
    }
    plain_io_message(e)
}

pub fn plain_io_message(e: &io::Error) -> String {
    use io::ErrorKind::*;
    match e.kind() {
        StorageFull => "The destination is full. Free some space and try again.".into(),
        PermissionDenied => "You do not have permission to write here.".into(),
        NotFound => "That item is no longer there.".into(),
        ReadOnlyFilesystem => "This location is read-only.".into(),
        AlreadyExists => "Something with that name is already there.".into(),
        // ENOTEMPTY and friends have no stable ErrorKind on all targets.
        _ => match e.raw_os_error() {
            Some(28) => "The destination is full. Free some space and try again.".into(),
            Some(39) => "That folder is not empty.".into(),
            Some(36) => "That name is too long for this filesystem.".into(),
            Some(122) => "You are over your disk quota.".into(),
            Some(30) => "This location is read-only.".into(),
            Some(5) => "The disk reported a read/write error.".into(),
            _ => e.to_string(),
        },
    }
}

impl From<io::Error> for OpError {
    fn from(e: io::Error) -> Self {
        OpError::Io(e)
    }
}

impl From<rustix::io::Errno> for OpError {
    fn from(e: rustix::io::Errno) -> Self {
        OpError::Io(e.into())
    }
}

pub type OpResult<T> = Result<T, OpError>;

/// Publication never authorizes replacement. Replace callers must preserve
/// their approved existing object before reaching this boundary; a later
/// occupant is another conflict, for every policy (including Skip).
pub(crate) fn publish_noreplace(
    src_dir: &DirHandle,
    src_name: &OsStr,
    dst_dir: &DirHandle,
    dst_name: &OsStr,
) -> OpResult<()> {
    crate::anchor::rename_noreplace(src_dir, src_name, dst_dir, dst_name).map_err(|e| {
        if e.kind() == io::ErrorKind::AlreadyExists {
            OpError::Conflict(PathBuf::from(dst_name))
        } else {
            OpError::Io(e)
        }
    })
}

/// Give each operation its own stage; O_EXCL still refuses any old occupant.
fn staging_name(final_name: &OsStr) -> OsString {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    loop {
        let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        #[cfg(test)]
        let sequence = publication_tests::NEXT_STAGE_SEQUENCE
            .with(|v| v.take())
            .unwrap_or(sequence);
        let candidate = OsString::from(format!(".gust-staging-{}-{sequence}", std::process::id()));
        if candidate != final_name {
            return candidate;
        }
    }
}

/// Remove a staging file after a failure and drop its claim — but only if
/// the file is really gone. On a remote whose connection just died the
/// unlink fails too; releasing the claim then would leave bytes on the
/// server that no later sweep could prove were ours. Keeping it lets the
/// next visit retain the evidence if remote ownership remains uncertain.
fn discard_staged(dst_dir: &DirHandle, stage: &OsStr, claim: &Option<PathBuf>, written: u64) {
    #[cfg(test)]
    let injected = publication_tests::DISCARD_ERROR.with(|v| v.take());
    #[cfg(not(test))]
    let injected: Option<rustix::io::Errno> = None;
    let result = match injected {
        Some(e) => Err(e),
        None => rustix::fs::unlinkat(dst_dir.as_fd(), stage, rustix::fs::AtFlags::empty()),
    };
    let gone = match result {
        Ok(()) => true,
        Err(rustix::io::Errno::NOENT) => true,
        Err(_) => false,
    };
    if let Some(c) = claim {
        if gone {
            crate::staging::release(c);
        } else {
            crate::staging::mark_unremoved(c, written);
        }
    }
}

/// Copy one regular file into `dst_dir` under `dst_name`.
///
/// Staged under a private name with restrictive permissions from creation,
/// then atomically published. A partial copy never appears at the
/// destination name.
pub fn copy_regular_file(
    src_dir: &DirHandle,
    src_name: &OsStr,
    dst_dir: &DirHandle,
    dst_name: &OsStr,
    policy: ConflictPolicy,
    cancel: &CancelToken,
) -> OpResult<u64> {
    copy_regular_file_with(
        src_dir,
        src_name,
        dst_dir,
        dst_name,
        policy,
        cancel,
        &mut |_| {},
    )
}

/// How often a copy reports bytes from inside one file.
const REPORT_EVERY: u64 = 1024 * 1024;

/// [`copy_regular_file`], reporting the bytes written so far in this file
/// every megabyte — so one big file moves the progress bar instead of
/// sitting at zero until it is done.
pub fn copy_regular_file_with(
    src_dir: &DirHandle,
    src_name: &OsStr,
    dst_dir: &DirHandle,
    dst_name: &OsStr,
    policy: ConflictPolicy,
    cancel: &CancelToken,
    on_bytes: &mut dyn FnMut(u64),
) -> OpResult<u64> {
    match stage_regular_file(
        src_dir, src_name, dst_dir, dst_name, policy, cancel, on_bytes, true,
    )? {
        None => Ok(0),
        Some(staged) => staged.commit(dst_dir, dst_name),
    }
}

/// A copied file still under its private staging name, waiting for
/// [`Staged::commit`]. Until then a crash leaves only claimed staging debris,
/// never a partial file under the real name.
#[derive(Debug)]
pub(crate) struct Staged {
    stage: OsString,
    staged_abs: PathBuf,
    claim: Option<PathBuf>,
    publication: RegularPublication,
    cancel: CancelToken,
    identity: FileIdentity,
    pub(crate) written: u64,
}

#[derive(Debug, Clone, Copy)]
enum RegularPublication {
    Rename,
    Link,
}

/// SSHFS does not implement directory fsync: client success cannot prove
/// durability of the newly linked name before a move deletes its source.
pub(crate) fn require_move_durability(dst_dir: &DirHandle) -> OpResult<()> {
    let fstype = dst_dir.fstype();
    require_move_durability_on(fstype.as_deref())
}

pub(crate) fn require_move_durability_on(fstype: Option<&str>) -> OpResult<()> {
    if matches!(fstype, None | Some("" | "fuse.sshfs")) {
        return Err(OpError::Refused(
            "This place cannot confirm the safety needed to remove the original. Try Copy instead; the original was kept.".into(),
        ));
    }
    Ok(())
}

impl RegularPublication {
    fn for_directory(dir: &DirHandle) -> Self {
        #[cfg(test)]
        if publication_tests::USE_LINK.with(|v| v.get()) {
            return Self::Link;
        }
        match dir.fstype().as_deref() {
            Some("fuse.sshfs") => Self::Link,
            _ => Self::Rename,
        }
    }

    /// Test the real operation on an empty private alias, before payload I/O.
    /// This is per upload, not a capability cache or a probe of the final name.
    fn prepare(self, dir: &DirHandle, stage: &OsStr, final_name: &OsStr) -> OpResult<()> {
        if matches!(self, Self::Rename) {
            return Ok(());
        }
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let alias = loop {
            let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            #[cfg(test)]
            let sequence = publication_tests::NEXT_PROBE_SEQUENCE
                .with(|v| v.take())
                .unwrap_or(sequence);
            let candidate = OsString::from(format!(
                ".gust-staging-{}-probe-{sequence}",
                std::process::id()
            ));
            if candidate != final_name {
                break candidate;
            }
        };
        let path = dir.path().join(&alias);
        let claim = crate::staging::claim(&path).map_err(OpError::Recovery)?;
        // An ambiguous link reply might already have created the alias. Keep
        // its unconfirmed claim; neither a sweep nor this attempt may guess.
        crate::anchor::link_noreplace(dir, stage, &alias).map_err(|_| {
            OpError::Io(io::Error::other(
                "This place could not confirm it can safely add this file before transfer. No file contents were copied. Reconnect or choose a different destination."))
        })?;
        if let Err(e) = crate::staging::confirm_at(&claim, &path, dir, &alias) {
            return Err(OpError::Recovery(e));
        }
        rustix::fs::unlinkat(dir.as_fd(), &alias, rustix::fs::AtFlags::empty())?;
        crate::staging::release(&claim);
        Ok(())
    }
}

impl Staged {
    /// Give the staged file its real name. The caller has made its bytes
    /// durable first (per file, or for a whole batch with one `syncfs`).
    pub(crate) fn commit(self, dst_dir: &DirHandle, dst_name: &OsStr) -> OpResult<u64> {
        #[cfg(test)]
        publication_tests::before_publication();
        let current = dst_dir.stat_child(&self.stage)?;
        if classify(&current) != FileClass::Regular
            || !self.identity.matches(&FileIdentity::from_stat(&current))
        {
            // Do not unlink an object that replaced our stage. Its claim
            // remains as evidence, never authority over this replacement.
            return Err(OpError::Refused(
                "staged file changed before publication; source and recovery claim kept".into(),
            ));
        }
        if self.cancel.checkpoint() {
            self.discard(dst_dir);
            return Err(OpError::Cancelled);
        }
        if matches!(self.publication, RegularPublication::Link) {
            if let Err(e) = crate::anchor::link_noreplace(dst_dir, &self.stage, dst_name) {
                // SFTP v3 may map occupied names to a generic failure. A
                // failed reply never proves that publication did not happen.
                // Retain the stage/claim and source; do not retry the request.
                if let Some(c) = &self.claim {
                    crate::staging::mark_unremoved(c, self.written);
                }
                return Err(if e.kind() == io::ErrorKind::AlreadyExists {
                    OpError::Conflict(PathBuf::from(dst_name))
                } else {
                    OpError::Io(io::Error::other(
                        "The copy was not confirmed. A complete file may already be there. The original and recovery record were kept. Check the destination before trying again."))
                });
            }
            #[cfg(test)]
            publication_tests::after_link();
            // link(2) success is the publication receipt. Alias cleanup is
            // independent: failing it must not erase success or lose a claim.
            let written = self.written;
            self.discard(dst_dir);
            return Ok(written);
        }
        if let Err(e) = publish_noreplace(dst_dir, &self.stage, dst_dir, dst_name) {
            // Even the final rename can fail (a full directory, a vanished
            // parent). The staging file is ours; do not abandon it.
            self.discard(dst_dir);
            return Err(e);
        }
        // Committed under its real name: the claim no longer describes anything.
        if let Some(c) = &self.claim {
            crate::staging::release(c);
        }
        Ok(self.written)
    }

    /// Remove the staged file (a batch that will not be committed).
    pub(crate) fn discard(self, dst_dir: &DirHandle) {
        let current = dst_dir
            .stat_child(&self.stage)
            .map(|s| FileIdentity::from_stat(&s));
        if matches!(current, Ok(current) if self.identity.matches(&current) && self.identity.mode == current.mode)
        {
            discard_staged(dst_dir, &self.stage, &self.claim, self.written);
        } else if let Some(c) = &self.claim {
            // A replaced or unreachable alias is not ours to unlink. Keep
            // its evidence, including after confirmed final publication.
            crate::staging::mark_unremoved(c, self.written);
        }
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        // No longer being written, whatever happened to it.
        crate::staging::end(&self.staged_abs);
    }
}

/// Copy a regular file to a staging name in `dst_dir`. With `durable` the
/// bytes are fsynced before returning; without, the caller must make them
/// durable (`syncfs`) before committing. `None`: skipped by policy.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stage_regular_file(
    src_dir: &DirHandle,
    src_name: &OsStr,
    dst_dir: &DirHandle,
    dst_name: &OsStr,
    policy: ConflictPolicy,
    cancel: &CancelToken,
    on_bytes: &mut dyn FnMut(u64),
    durable: bool,
) -> OpResult<Option<Staged>> {
    stage_regular_file_cached(
        src_dir,
        src_name,
        dst_dir,
        dst_name,
        policy,
        cancel,
        on_bytes,
        durable,
        &mut crate::source_read::LocalMounts::default(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn stage_regular_file_cached(
    src_dir: &DirHandle,
    src_name: &OsStr,
    dst_dir: &DirHandle,
    dst_name: &OsStr,
    policy: ConflictPolicy,
    cancel: &CancelToken,
    on_bytes: &mut dyn FnMut(u64),
    durable: bool,
    sources: &mut crate::source_read::LocalMounts,
) -> OpResult<Option<Staged>> {
    if cancel.checkpoint() {
        return Err(OpError::Cancelled);
    }
    let st = src_dir.stat_child(src_name)?;
    match classify(&st) {
        FileClass::Regular => {}
        FileClass::Special => {
            return Err(OpError::Refused(
                "special file (FIFO/socket/device) is not copied".into(),
            ))
        }
        FileClass::Symlink => return Err(OpError::Refused("symlink needs copy_symlink".into())),
        FileClass::Directory => {
            return Err(OpError::Refused("directory needs recursive copy".into()))
        }
    }

    // Conflict check before doing any work.
    if dst_dir.stat_child(dst_name).is_ok() {
        match policy {
            ConflictPolicy::Skip => return Ok(None),
            // KeepBoth is resolved by the caller choosing a free name; if a
            // name is taken here anyway, it was taken by someone else in the
            // meantime and is theirs.
            ConflictPolicy::Ask | ConflictPolicy::KeepBoth | ConflictPolicy::Merge => {
                return Err(OpError::Conflict(PathBuf::from(dst_name)));
            }
            ConflictPolicy::Replace => {}
        }
    }

    let src = rustix::fs::openat(
        src_dir.as_fd(),
        src_name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let source_identity = FileIdentity::from_stat(&st);
    let st = rustix::fs::fstat(&src)?;
    if classify(&st) != FileClass::Regular
        || !source_identity.matches(&FileIdentity::from_stat(&st))
    {
        return Err(OpError::Refused(
            "source changed before copying; nothing was published".into(),
        ));
    }
    let expected_len = u64::try_from(st.st_size)
        .map_err(|_| OpError::Refused("source reported an invalid file size".into()))?;

    let mut reader = crate::source_read::Reader::new(&src, st.st_dev, sources)?;

    let stage = staging_name(dst_name);
    // Record ownership BEFORE the file exists. A staging file with no claim is
    // never reclaimed by the sweep, so the claim must be durable first.
    let staged_abs = dst_dir.path().join(&stage);
    // Registered as in flight BEFORE the claim exists and for exactly as
    // long as the file is held (until `Staged` is committed or discarded),
    // so no sweep — not even one in this process — can take a copy that is
    // still being written.
    crate::staging::begin(&staged_abs);
    struct Active(Option<PathBuf>);
    impl Drop for Active {
        fn drop(&mut self) {
            if let Some(p) = &self.0 {
                crate::staging::end(p);
            }
        }
    }
    let mut active = Active(Some(staged_abs.clone()));
    // No claim, no copy: a staging file nobody owns can never be cleaned up
    // after a crash, and a full home disk is exactly when that happens.
    let claim = Some(crate::staging::claim(&staged_abs).map_err(OpError::Recovery)?);
    // Create with the source's permission bits masked to owner-only first;
    // widened at the end. Never world-readable in between.
    let dst = match rustix::fs::openat(
        dst_dir.as_fd(),
        &stage,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
        Mode::from_bits_truncate(0o600),
    ) {
        Ok(fd) => fd,
        Err(e) => {
            if let Some(c) = &claim {
                crate::staging::release(c);
            }
            return Err(e.into());
        }
    };
    if let Some(c) = &claim {
        // Bind the claim to the object actually created (device + inode), so
        // reclaiming can prove it is the same file and not a replacement.
        if let Err(e) = crate::staging::confirm_file(c, &staged_abs, &dst) {
            drop(dst);
            discard_staged(dst_dir, &stage, &claim, 0);
            return Err(OpError::Recovery(e));
        }
    }

    let publication = RegularPublication::for_directory(dst_dir);
    if let Err(e) = publication.prepare(dst_dir, &stage, dst_name) {
        drop(dst);
        discard_staged(dst_dir, &stage, &claim, 0);
        return Err(e);
    }

    let mut written = 0u64;
    let mut reported = 0u64;

    // Any failure from here on must remove the staging file. A disk that
    // filled up mid-copy would otherwise leave a partial file consuming the
    // last free bytes, invisible to the user and cleaned up by nobody.
    let copy_result = (|| -> OpResult<()> {
        loop {
            if cancel.checkpoint() {
                return Err(OpError::Cancelled);
            }
            // A disconnected FUSE source can report EOF without changing
            // its stat identity. Exercise that exact case without mutating
            // the source in the regression tests.
            #[cfg(test)]
            let early_eof = tests::EARLY_EOF_AFTER
                .with(|limit| limit.get().is_some_and(|after| written >= after));
            #[cfg(not(test))]
            let early_eof = false;
            let n = if early_eof {
                0
            } else {
                reader.read(&src, cancel)?
            };
            if n == 0 {
                break;
            }
            let mut off = 0;
            while off < n {
                if cancel.checkpoint() {
                    return Err(OpError::Cancelled);
                }
                #[cfg(test)]
                let end =
                    n.min(off.saturating_add(publication_tests::WRITE_LIMIT.with(|v| v.get())));
                #[cfg(not(test))]
                let end = n;
                let w = rustix::io::write(&dst, &reader.buffer[off..end])?;
                if w == 0 {
                    return Err(OpError::Io(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "destination accepted no bytes",
                    )));
                }
                off += w;
                written += w as u64;
                #[cfg(test)]
                publication_tests::after_write();
            }
            if written - reported >= REPORT_EVERY {
                reported = written;
                on_bytes(written);
            }
        }

        if written != expected_len {
            return Err(OpError::Refused(format!(
                "The source changed or stopped responding: expected {expected_len} bytes but received {written}. Nothing was published and the source was not removed. Reconnect if needed and try again."
            )));
        }
        if !source_identity.matches(&FileIdentity::from_stat(&rustix::fs::fstat(&src)?)) {
            return Err(OpError::Refused(
                "source changed during copying; nothing was published and the source was not removed"
                    .into(),
            ));
        }

        // Preserve mode (minus setuid/setgid — never applied blindly).
        if cancel.checkpoint() {
            return Err(OpError::Cancelled);
        }
        let safe_mode = st.st_mode & 0o7777 & !0o6000;
        rustix::fs::fchmod(&dst, Mode::from_bits_truncate(safe_mode))?;
        // Colour tags travel with the copy (DESIGN §6.25), onto the staged
        // file, so they are there the moment it has its name.
        if cancel.checkpoint() {
            return Err(OpError::Cancelled);
        }
        crate::tags::carry(&src, &dst);
        // Durability before the destination name exists. fsync can itself
        // fail on a full filesystem, which is exactly when the staging file
        // must not be left behind. A batch defers this to one syncfs.
        if durable {
            if cancel.checkpoint() {
                return Err(OpError::Cancelled);
            }
            rustix::fs::fsync(&dst)?;
        }
        reader.finish()?;
        Ok(())
    })();

    let identity = rustix::fs::fstat(&dst).map(|s| FileIdentity::from_stat(&s));
    drop(dst);

    if let Err(e) = copy_result {
        discard_staged(dst_dir, &stage, &claim, written);
        return Err(e);
    }
    let identity = identity?;
    // From here `Staged` owns the in-flight mark.
    active.0 = None;
    Ok(Some(Staged {
        stage,
        staged_abs,
        claim,
        publication,
        cancel: cancel.clone(),
        identity,
        written,
    }))
}

/// Copy a symlink as a symlink — never by following it.
pub fn copy_symlink(
    src_dir: &DirHandle,
    src_name: &OsStr,
    dst_dir: &DirHandle,
    dst_name: &OsStr,
) -> OpResult<()> {
    copy_symlink_with_policy(src_dir, src_name, dst_dir, dst_name, ConflictPolicy::Ask)
}

pub(crate) fn copy_symlink_with_policy(
    src_dir: &DirHandle,
    src_name: &OsStr,
    dst_dir: &DirHandle,
    dst_name: &OsStr,
    policy: ConflictPolicy,
) -> OpResult<()> {
    // Copying a link onto itself would replace the original with a copy of
    // itself and hand undo a step that removes the user's only link. The
    // destination must never be the source.
    if src_name == dst_name {
        let same_dir = match (src_dir.stat_self(), dst_dir.stat_self()) {
            (Ok(a), Ok(b)) => a.st_dev == b.st_dev && a.st_ino == b.st_ino,
            _ => false,
        };
        if same_dir {
            return Err(OpError::Refused(
                "a link cannot be copied over itself".into(),
            ));
        }
    }
    if dst_dir.stat_child(dst_name).is_ok() {
        return match policy {
            ConflictPolicy::Skip => Ok(()),
            _ => Err(OpError::Conflict(PathBuf::from(dst_name))),
        };
    }
    let target = rustix::fs::readlinkat(src_dir.as_fd(), src_name, Vec::new())?;
    let stage = staging_name(dst_name);
    let staged_abs = dst_dir.path().join(&stage);
    let claim = Some(crate::staging::claim(&staged_abs).map_err(OpError::Recovery)?);
    if let Err(e) = rustix::fs::symlinkat(target.as_c_str(), dst_dir.as_fd(), &stage) {
        if let Some(c) = &claim {
            crate::staging::release(c);
        }
        return Err(e.into());
    }
    if let Some(c) = &claim {
        if let Err(e) = crate::staging::confirm_at(c, &staged_abs, dst_dir, &stage) {
            discard_staged(dst_dir, &stage, &claim, 0);
            return Err(OpError::Recovery(e));
        }
    }
    #[cfg(test)]
    publication_tests::before_publication();
    let r = publish_noreplace(dst_dir, &stage, dst_dir, dst_name);
    if r.is_err() {
        discard_staged(dst_dir, &stage, &claim, 0);
    } else if let Some(c) = &claim {
        crate::staging::release(c);
    }
    r?;
    Ok(())
}

/// Same-filesystem move: a single atomic rename, nothing to clean up.
pub fn move_same_fs(
    src_dir: &DirHandle,
    src_name: &OsStr,
    dst_dir: &DirHandle,
    dst_name: &OsStr,
) -> OpResult<()> {
    require_move_durability(dst_dir)?;
    publish_noreplace(src_dir, src_name, dst_dir, dst_name)
}

/// Cross-filesystem move: copy, verify, commit, then remove ONLY the verified
/// unchanged source (DESIGN.md §6).
///
/// If the source changed while copying, the copy stays and the source is kept.
/// Losing data is never the fallback.
pub fn move_cross_fs(
    src_dir: &DirHandle,
    src_name: &OsStr,
    dst_dir: &DirHandle,
    dst_name: &OsStr,
    policy: ConflictPolicy,
    cancel: &CancelToken,
) -> OpResult<()> {
    require_move_durability(dst_dir)?;
    let before = FileIdentity::from_stat(&src_dir.stat_child(src_name)?);
    let Some(staged) = stage_regular_file(
        src_dir,
        src_name,
        dst_dir,
        dst_name,
        policy,
        cancel,
        &mut |_| {},
        true,
    )?
    else {
        // Skip has no publication receipt, including for an empty source.
        return Ok(());
    };
    staged.commit(dst_dir, dst_name)?;

    // Re-verify the source is byte-identical to what we copied.
    let after = FileIdentity::from_stat(&src_dir.stat_child(src_name)?);
    if !before.matches(&after) {
        return Err(OpError::Refused(
            "source changed during move; destination written, source kept".into(),
        ));
    }
    // The copy receipt remains valid if cancellation arrives after commit;
    // retain the source instead of admitting another destructive step.
    let cancelled_after_copy =
        || OpError::Refused("destination was published; move cancelled and source kept".into());
    if cancel.checkpoint() {
        return Err(cancelled_after_copy());
    }
    // Durability of the destination directory entry before unlinking.
    rustix::fs::fsync(dst_dir.as_fd())?;
    if cancel.checkpoint() {
        return Err(cancelled_after_copy());
    }
    rustix::fs::unlinkat(src_dir.as_fd(), src_name, rustix::fs::AtFlags::empty())?;
    Ok(())
}

/// Plan a copy/move without performing it. Captures source identities so the
/// journal can later verify nothing was swapped.
pub fn plan_job(
    id: &str,
    kind: JobKind,
    sources: &[PathBuf],
    dest_dir: &Path,
    policy: ConflictPolicy,
) -> Job {
    let mut job = Job::new(id, kind);
    job.conflict_policy = policy;
    for s in sources {
        let identity = FileIdentity::of_path(s).ok().map(RecordedIdentity::from);
        let name = s.file_name().unwrap_or(OsStr::new("unnamed"));
        job.entries.push(JournalEntry {
            source: s.clone(),
            destination: Some(dest_dir.join(name)),
            source_identity: identity,
            preserved: None,
            done: false,
            error: None,
        });
    }
    job.state = JobState::Planned;
    job
}

/// Execute a planned copy job against real paths.
pub fn run_copy_job(job: &mut Job, cancel: &CancelToken) -> OpResult<()> {
    job.state = JobState::Running;
    for idx in 0..job.entries.len() {
        if cancel.checkpoint() {
            job.state = JobState::Cancelled;
            return Err(OpError::Cancelled);
        }
        let (src, dst) = {
            let e = &job.entries[idx];
            let Some(d) = e.destination.clone() else {
                continue;
            };
            (e.source.clone(), d)
        };
        let res = (|| -> OpResult<()> {
            let (sd, sn) = resolve_parent(&src)?;
            let (dd, dn) = resolve_parent(&dst)?;
            let st = sd.stat_child(&sn)?;
            match classify(&st) {
                FileClass::Regular => {
                    copy_regular_file(&sd, &sn, &dd, &dn, job.conflict_policy, cancel)?;
                }
                FileClass::Symlink => {
                    copy_symlink_with_policy(&sd, &sn, &dd, &dn, job.conflict_policy)?
                }
                FileClass::Directory => {
                    return Err(OpError::Refused(
                        "recursive directory copy not in this path".into(),
                    ))
                }
                FileClass::Special => return Err(OpError::Refused("special file refused".into())),
            }
            Ok(())
        })();
        match res {
            Ok(()) => job.entries[idx].done = true,
            Err(e) => {
                job.entries[idx].error = Some(e.to_string());
                job.state = JobState::Failed;
                return Err(e);
            }
        }
    }
    job.state = JobState::Confirmed;
    Ok(())
}

#[cfg(test)]
#[path = "publication_tests.rs"]
pub(crate) mod publication_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    thread_local! {
        pub(super) static EARLY_EOF_AFTER: std::cell::Cell<Option<u64>> = const {
            std::cell::Cell::new(None)
        };
    }

    struct EarlyEof;
    impl EarlyEof {
        fn after(bytes: u64) -> Self {
            EARLY_EOF_AFTER.with(|limit| limit.set(Some(bytes)));
            Self
        }
    }
    impl Drop for EarlyEof {
        fn drop(&mut self) {
            EARLY_EOF_AFTER.with(|limit| limit.set(None));
        }
    }

    #[test]
    fn early_eof_copy_never_publishes_or_confirms() {
        let d = tmp("early-eof-copy");
        let source = d.join("source.bin");
        let dest = d.join("dest");
        fs::create_dir(&dest).unwrap();
        let original = vec![73; 4 * 1024 * 1024];
        fs::write(&source, &original).unwrap();
        let before = FileIdentity::of_path(&source).unwrap();
        let mut job = plan_job(
            "early-eof",
            JobKind::Copy,
            std::slice::from_ref(&source),
            &dest,
            ConflictPolicy::Ask,
        );
        let _fault = EarlyEof::after(256 * 1024);
        let result = run_copy_job(&mut job, &CancelToken::new());
        assert!(matches!(result, Err(OpError::Refused(_))), "{result:?}");
        assert_eq!(job.state, JobState::Failed);
        assert!(job.entries.iter().all(|entry| !entry.done));
        assert_eq!(fs::read(&source).unwrap(), original);
        assert!(before.matches(&FileIdentity::of_path(&source).unwrap()));
        assert_eq!(fs::read_dir(&dest).unwrap().count(), 0);
        assert!(!crate::staging::load_all()
            .iter()
            .any(|claim| claim.path.parent() == Some(dest.as_path())));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn early_eof_cross_fs_move_keeps_the_unchanged_source() {
        let d = tmp("early-eof-move");
        let source = d.join("source.bin");
        let dest = d.join("dest");
        fs::create_dir(&dest).unwrap();
        let original = vec![91; 4 * 1024 * 1024];
        fs::write(&source, &original).unwrap();
        let before = FileIdentity::of_path(&source).unwrap();
        let (sd, sn) = handles(&d, "source.bin");
        let (dd, dn) = handles(&dest, "source.bin");
        let _fault = EarlyEof::after(256 * 1024);
        let result = move_cross_fs(&sd, &sn, &dd, &dn, ConflictPolicy::Ask, &CancelToken::new());
        assert!(matches!(result, Err(OpError::Refused(_))), "{result:?}");
        assert_eq!(fs::read(&source).unwrap(), original);
        assert!(before.matches(&FileIdentity::of_path(&source).unwrap()));
        assert_eq!(fs::read_dir(&dest).unwrap().count(), 0);
        assert!(!crate::staging::load_all()
            .iter()
            .any(|claim| claim.path.parent() == Some(dest.as_path())));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn source_changed_during_copy_is_not_published() {
        use std::io::Write;
        let d = tmp("changed-copy");
        let source = d.join("source.bin");
        let dest = d.join("dest");
        fs::create_dir(&dest).unwrap();
        let mut original = vec![61; 4 * 1024 * 1024];
        fs::write(&source, &original).unwrap();
        let (sd, sn) = handles(&d, "source.bin");
        let (dd, dn) = handles(&dest, "source.bin");
        let mut changed = false;
        let result = copy_regular_file_with(
            &sd,
            &sn,
            &dd,
            &dn,
            ConflictPolicy::Ask,
            &CancelToken::new(),
            &mut |_| {
                if !changed {
                    changed = true;
                    let mut file = fs::OpenOptions::new().write(true).open(&source).unwrap();
                    file.write_all(&[17]).unwrap();
                    file.set_modified(std::time::SystemTime::UNIX_EPOCH)
                        .unwrap();
                }
            },
        );
        assert!(changed);
        assert!(matches!(result, Err(OpError::Refused(_))), "{result:?}");
        original[0] = 17;
        assert_eq!(fs::read(&source).unwrap(), original);
        assert_eq!(fs::read_dir(&dest).unwrap().count(), 0);
        fs::remove_dir_all(&d).unwrap();
    }

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("gust-ops-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn handles(dir: &Path, name: &str) -> (DirHandle, OsString) {
        resolve_parent(&dir.join(name)).unwrap()
    }

    #[test]
    fn a_lost_connection_is_not_called_a_disk_error() {
        // Found live: killing the SFTP server mid-copy showed "The disk
        // reported a read/write error." There was no disk problem.
        let dest = Path::new("/run/user/1000/gust/places/studio/holiday.mkv");
        let eio = io::Error::from_raw_os_error(5);
        let m = plain_io_message_on(&eio, dest, Some("fuse.sshfs"));
        assert!(m.contains("connection to studio was lost"), "{m}");
        assert!(!m.contains("disk"), "{m}");
        let m = plain_io_message_on(&io::Error::from_raw_os_error(107), dest, Some("cifs"));
        assert!(m.contains("was lost"), "{m}");
        // A local disk still reports a disk error, honestly.
        let m = plain_io_message_on(&eio, Path::new("/home/u/x"), Some("ext4"));
        assert_eq!(m, "The disk reported a read/write error.");
        // A full remote is still "full", not "connection lost".
        let m = plain_io_message_on(&io::Error::from_raw_os_error(28), dest, Some("fuse.sshfs"));
        assert!(m.contains("full"), "{m}");
    }

    #[test]
    fn a_staging_file_that_cannot_be_removed_keeps_its_claim() {
        // Found live: the SFTP server died mid-copy, the unlink of the
        // staging file failed too (the connection was gone), and the claim
        // was released anyway — leaving 231 MB on the server that no later
        // sweep could ever prove was Gust's. A failed removal must keep
        // the claim so the next visit, with the server back, cleans it up.
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("unremovable");
        crate::staging::fault::state_home(&d.join("state"));
        fs::create_dir_all(d.join("src")).unwrap();
        fs::create_dir_all(d.join("dst")).unwrap();
        fs::write(d.join("src/big.bin"), vec![7u8; 3 * 1024 * 1024]).unwrap();
        let (sd, sn) = handles(&d.join("src"), "big.bin");
        let (dd, dn) = handles(&d.join("dst"), "big.bin");
        let cancel = CancelToken::new();
        let dst = d.join("dst");
        let mut once = false;
        let r = copy_regular_file_with(
            &sd,
            &sn,
            &dd,
            &dn,
            ConflictPolicy::Ask,
            &cancel,
            &mut |_| {
                if !once {
                    once = true;
                    // The destination stops accepting removals, then the copy fails.
                    fs::set_permissions(&dst, fs::Permissions::from_mode(0o555)).unwrap();
                    cancel.cancel();
                }
            },
        );
        assert!(r.is_err());
        let left: Vec<_> = fs::read_dir(&dst)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        fs::set_permissions(&dst, fs::Permissions::from_mode(0o755)).unwrap();
        if left.is_empty() {
            // Running as root: the removal succeeded, nothing to prove.
            fs::remove_dir_all(&d).ok();
            return;
        }
        assert!(
            crate::staging::load_all()
                .iter()
                .any(|c| left.contains(&c.path)),
            "the staging file that could not be removed lost its ownership record: {left:?}"
        );
        // With the destination reachable again, the next visit reclaims it
        // even though its owner — this process — is still running.
        assert_eq!(
            sweep_stale_staging(&dst),
            1,
            "the failed copy's debris is reclaimed"
        );
        assert!(
            fs::read_dir(&dst).unwrap().next().is_none(),
            "nothing is left behind"
        );
        assert!(
            !crate::staging::load_all()
                .iter()
                .any(|c| left.contains(&c.path)),
            "the claim went with the file"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn copies_a_regular_file_with_contents_and_mode() {
        let d = tmp("copy");
        fs::write(d.join("src.txt"), b"payload").unwrap();
        fs::set_permissions(d.join("src.txt"), std::fs::Permissions::from_mode(0o640)).unwrap();
        use std::os::unix::fs::PermissionsExt;

        let (sd, sn) = handles(&d, "src.txt");
        let (dd, dn) = handles(&d, "dst.txt");
        let n = copy_regular_file(
            &sd,
            &sn,
            &dd,
            &dn,
            ConflictPolicy::Replace,
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(n, 7);
        assert_eq!(fs::read(d.join("dst.txt")).unwrap(), b"payload");
        let mode = fs::metadata(d.join("dst.txt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o640, "permissions preserved");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn refuses_to_copy_a_fifo() {
        // Special files are refused, not silently turned into empty files.
        let d = tmp("fifo");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            d.join("pipe"),
            rustix::fs::FileType::Fifo,
            Mode::from_bits_truncate(0o600),
            0,
        )
        .unwrap();
        let (sd, sn) = handles(&d, "pipe");
        let (dd, dn) = handles(&d, "copy");
        let err = copy_regular_file(
            &sd,
            &sn,
            &dd,
            &dn,
            ConflictPolicy::Replace,
            &CancelToken::new(),
        )
        .unwrap_err();
        assert!(matches!(err, OpError::Refused(_)), "got {err:?}");
        assert!(!d.join("copy").exists(), "nothing created for refused file");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn ask_policy_reports_conflict_without_touching_destination() {
        let d = tmp("conflict");
        fs::write(d.join("a"), b"new").unwrap();
        fs::write(d.join("b"), b"ORIGINAL").unwrap();
        let (sd, sn) = handles(&d, "a");
        let (dd, dn) = handles(&d, "b");
        let err = copy_regular_file(&sd, &sn, &dd, &dn, ConflictPolicy::Ask, &CancelToken::new())
            .unwrap_err();
        assert!(matches!(err, OpError::Conflict(_)));
        assert_eq!(fs::read(d.join("b")).unwrap(), b"ORIGINAL", "untouched");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn skip_policy_leaves_destination_alone() {
        let d = tmp("skip");
        fs::write(d.join("a"), b"new").unwrap();
        fs::write(d.join("b"), b"ORIGINAL").unwrap();
        let (sd, sn) = handles(&d, "a");
        let (dd, dn) = handles(&d, "b");
        let n = copy_regular_file(
            &sd,
            &sn,
            &dd,
            &dn,
            ConflictPolicy::Skip,
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(n, 0);
        assert_eq!(fs::read(d.join("b")).unwrap(), b"ORIGINAL");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn cancelled_copy_leaves_no_partial_destination() {
        let d = tmp("cancelcopy");
        fs::write(d.join("big"), vec![7u8; 512 * 1024]).unwrap();
        let (sd, sn) = handles(&d, "big");
        let (dd, dn) = handles(&d, "out");
        let tok = CancelToken::new();
        tok.cancel();
        let err = copy_regular_file(&sd, &sn, &dd, &dn, ConflictPolicy::Replace, &tok).unwrap_err();
        assert!(matches!(err, OpError::Cancelled));
        assert!(!d.join("out").exists(), "no partial file left behind");
        // And no staging litter.
        let leftovers: Vec<_> = fs::read_dir(&d)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".gust-staging"))
            .collect();
        assert!(leftovers.is_empty(), "staging files cleaned up");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn copies_symlink_as_symlink_not_as_target() {
        let d = tmp("symcopy");
        fs::write(d.join("target.txt"), b"real content").unwrap();
        std::os::unix::fs::symlink("target.txt", d.join("link")).unwrap();
        let (sd, sn) = handles(&d, "link");
        let (dd, dn) = handles(&d, "link-copy");
        copy_symlink(&sd, &sn, &dd, &dn).unwrap();
        let md = fs::symlink_metadata(d.join("link-copy")).unwrap();
        assert!(md.file_type().is_symlink(), "copied as a symlink");
        assert_eq!(
            fs::read_link(d.join("link-copy")).unwrap(),
            Path::new("target.txt")
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn same_fs_move_is_atomic_rename() {
        let d = tmp("mv");
        fs::write(d.join("a"), b"data").unwrap();
        let (sd, sn) = handles(&d, "a");
        let (dd, dn) = handles(&d, "b");
        move_same_fs(&sd, &sn, &dd, &dn).unwrap();
        assert!(!d.join("a").exists());
        assert_eq!(fs::read(d.join("b")).unwrap(), b"data");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn setuid_bit_is_never_propagated() {
        let d = tmp("setuid");
        fs::write(d.join("s"), b"x").unwrap();
        fs::set_permissions(d.join("s"), std::fs::Permissions::from_mode(0o4755)).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let (sd, sn) = handles(&d, "s");
        let (dd, dn) = handles(&d, "s-copy");
        copy_regular_file(
            &sd,
            &sn,
            &dd,
            &dn,
            ConflictPolicy::Replace,
            &CancelToken::new(),
        )
        .unwrap();
        let mode = fs::metadata(d.join("s-copy")).unwrap().permissions().mode();
        assert_eq!(mode & 0o6000, 0, "setuid/setgid must not be copied");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn plan_records_source_identity() {
        let d = tmp("plan");
        fs::write(d.join("f"), b"x").unwrap();
        let job = plan_job(
            "j",
            JobKind::Copy,
            &[d.join("f")],
            &d.join("sub"),
            ConflictPolicy::Ask,
        );
        assert_eq!(job.entries.len(), 1);
        assert!(job.entries[0].source_identity.is_some());
        assert_eq!(job.state, JobState::Planned);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn copy_job_runs_and_confirms() {
        let d = tmp("job");
        fs::create_dir(d.join("out")).unwrap();
        fs::write(d.join("one.txt"), b"1").unwrap();
        fs::write(d.join("two.txt"), b"22").unwrap();
        let mut job = plan_job(
            "j2",
            JobKind::Copy,
            &[d.join("one.txt"), d.join("two.txt")],
            &d.join("out"),
            ConflictPolicy::Replace,
        );
        run_copy_job(&mut job, &CancelToken::new()).unwrap();
        assert_eq!(job.state, JobState::Confirmed);
        assert_eq!(fs::read(d.join("out/one.txt")).unwrap(), b"1");
        assert_eq!(fs::read(d.join("out/two.txt")).unwrap(), b"22");
        assert!(job.entries.iter().all(|e| e.done));
        fs::remove_dir_all(&d).ok();
    }
}

/// Remove staging files left behind by a process that died mid-copy.
///
/// Stages have unique per-operation names. Only abandoned, validated local
/// claims with exact object identity authorize cleanup. Network, unconfirmed,
/// legacy and uncertain records/files remain for inspection; no sweep touches
/// the final publication name or an active operation's stage.
pub fn sweep_stale_staging(dir: &Path) -> usize {
    let mut removed = 0usize;

    // Ownership comes from the claim store, never from the filename. A file
    // that merely LOOKS like debris was not created by us and is left alone:
    // PIDs are recycled, remote PIDs are meaningless locally, and a user is
    // entitled to name a file whatever they like.
    for c in crate::staging::load_all() {
        // Only this directory's debris is this call's business.
        if c.path.parent() != Some(dir) {
            continue;
        }
        // A copy in flight right now — in another process or in this one —
        // is never touched. A claim this process holds for a copy that has
        // already failed is abandoned, but remote ownership still cannot
        // be inferred from the name, size, time or synthetic inode.
        if !crate::staging::is_abandoned(&c) {
            continue;
        }
        // The claim must describe the object that is actually there.
        let Ok(md) = c.path.symlink_metadata() else {
            // The file is already gone; the claim is the only leftover.
            continue;
        };
        if !md.is_file() {
            // Never a directory, never a symlink, never followed.
            continue;
        }
        if c.dev == 0 || c.ino == 0 {
            // An unconfirmed claim cannot prove which object it owns.
            continue;
        }
        if !crate::staging::describes(&c, &md) {
            // Something else occupies that path now. Not ours to remove.
            continue;
        }
        if std::fs::remove_file(&c.path).is_ok() {
            removed += 1;
            // The record described that file and nothing else.
            crate::staging::release(&c.claim_file);
        }
    }
    removed
}

/// Remove claim files whose staging file no longer exists and whose owner is
/// gone, so the claim store cannot grow without bound.
pub fn prune_dead_claims() -> usize {
    let mut pruned = 0usize;
    for c in crate::staging::load_all() {
        if !crate::staging::is_abandoned(&c) || c.net || c.dev == 0 || c.ino == 0 {
            continue;
        }
        // An inaccessible mount, an unmounted remote, or an unconfirmed
        // request is not evidence that its staging object does not exist.
        if !matches!(c.path.symlink_metadata(), Err(e) if e.kind() == io::ErrorKind::NotFound) {
            continue;
        }
        if std::fs::remove_file(&c.claim_file).is_ok() {
            pruned += 1;
        }
    }
    pruned
}

// `pid_is_alive` was removed with the filename-shape sweep: a live PID never
// established ownership of a staging file (PIDs are recycled, and a remote
// filesystem's PIDs are meaningless locally). Ownership now comes from a
// recorded claim bound to dev+inode+boot_id; see crate::staging.

#[cfg(test)]
mod sweep_tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gust-sweep-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn debris_from_a_dead_process_is_removed() {
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let d = tmp("dead");
        // Isolate the claim store so this test owns it.
        crate::staging::fault::state_home(&d.join("state"));

        // Real debris: a staging file WITH an ownership claim, whose owning
        // process is gone. A filename alone is deliberately not enough any
        // more, so the claim is what makes this reclaimable.
        let debris = d.join(".gust-staging-999999-report.pdf");
        fs::write(&debris, b"partial").unwrap();
        let claim = crate::staging::claim(&debris).unwrap();
        crate::staging::confirm(&claim, &debris).unwrap();
        // Retain a complete current-format claim, but make its owner absent.
        crate::staging::fault::rewrite_claim(&claim, |c| c.pid = 999999);
        fs::write(d.join("real.txt"), b"keep me").unwrap();

        assert_eq!(sweep_stale_staging(&d), 1);
        assert!(!debris.exists());
        assert_eq!(
            fs::read(d.join("real.txt")).unwrap(),
            b"keep me",
            "ordinary files are never touched"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn uncertain_network_debris_is_retained_after_a_reconnect() {
        // Found live: after the SFTP server came back, sshfs numbered the
        // same file inode 2 instead of 5, so a dev+inode check could never
        // match and 232 MB stayed on the server for good.
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        use std::os::unix::fs::MetadataExt;
        let d = tmp("netdebris");
        crate::staging::fault::state_home(&d.join("state"));
        let debris = d.join(".gust-staging-999999-holiday.mkv");
        fs::write(&debris, vec![1u8; 4096]).unwrap();
        let claim = crate::staging::claim(&debris).unwrap();
        let md = debris.symlink_metadata().unwrap();
        let write_claim = |net: u8, written: u64, ino: u64| {
            crate::staging::fault::rewrite_claim(&claim, |c| {
                c.pid = 999999;
                c.net = net == 1;
                c.written = written;
                c.ino = ino;
            });
        };
        // The inode changed, on a LOCAL disk: never removed on weaker proof.
        write_claim(0, 4096, md.ino() + 1000);
        assert_eq!(
            sweep_stale_staging(&d),
            0,
            "a local file needs its exact identity"
        );
        // A network place, but the file holds MORE than Gust ever wrote:
        // somebody else wrote to it. Not ours.
        write_claim(1, 1024, md.ino() + 1000);
        assert_eq!(sweep_stale_staging(&d), 0, "larger than what Gust wrote");
        // A replacement can satisfy the old size/time heuristic. Keep it
        // and the claim: retained debris is not proven ownership cleanup.
        write_claim(1, 8192, md.ino() + 1000);
        assert_eq!(
            sweep_stale_staging(&d),
            0,
            "size/time are not ownership proof"
        );
        assert!(debris.exists());
        // Even a reused synthetic inode cannot prove this is our object.
        write_claim(1, 8192, md.ino());
        assert_eq!(sweep_stale_staging(&d), 0);
        assert!(crate::staging::load_all().iter().any(|c| c.path == debris));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_staging_shaped_file_with_no_claim_is_never_removed() {
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let d = tmp("unclaimed");
        crate::staging::fault::state_home(&d.join("state"));

        // Exactly the old heuristic's target: the right name shape and a PID
        // that cannot be alive. Without an ownership record it is somebody
        // else's file and must survive.
        let impostor = d.join(".gust-staging-999999-my-thesis.txt");
        fs::write(&impostor, b"the only copy the user has").unwrap();

        assert_eq!(sweep_stale_staging(&d), 0);
        assert_eq!(
            fs::read(&impostor).unwrap(),
            b"the only copy the user has",
            "a filename is not an ownership record"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_live_processes_staging_file_is_left_alone() {
        let d = tmp("live");
        // Our own PID is definitely alive: this stands in for another copy
        // running right now, whose staging file must not be stolen.
        let mine = format!(".gust-staging-{}-inflight.bin", std::process::id());
        fs::write(d.join(&mine), b"in progress").unwrap();

        assert_eq!(sweep_stale_staging(&d), 0);
        assert!(
            d.join(&mine).exists(),
            "taking a live copy's staging file would corrupt it"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn unrelated_dotfiles_are_never_swept() {
        let d = tmp("dotfiles");
        for n in [
            ".bashrc",
            ".hidden",
            ".gust-something-else",
            "gust-staging-1-x",
        ] {
            fs::write(d.join(n), b"x").unwrap();
        }
        assert_eq!(sweep_stale_staging(&d), 0);
        for n in [
            ".bashrc",
            ".hidden",
            ".gust-something-else",
            "gust-staging-1-x",
        ] {
            assert!(d.join(n).exists(), "{n} must survive");
        }
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_directory_named_like_debris_is_not_removed() {
        let d = tmp("dirdebris");
        fs::create_dir(d.join(".gust-staging-999999-adirectory")).unwrap();
        assert_eq!(sweep_stale_staging(&d), 0, "only regular files are swept");
        assert!(d.join(".gust-staging-999999-adirectory").exists());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_symlink_named_like_debris_is_not_followed() {
        let d = tmp("symdebris");
        fs::write(d.join("precious.txt"), b"do not delete").unwrap();
        std::os::unix::fs::symlink(d.join("precious.txt"), d.join(".gust-staging-999999-link"))
            .unwrap();

        sweep_stale_staging(&d);
        assert_eq!(
            fs::read(d.join("precious.txt")).unwrap(),
            b"do not delete",
            "a sweep must never follow a link to its target"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_malformed_staging_name_is_ignored() {
        let d = tmp("malformed");
        for n in [
            ".gust-staging-",
            ".gust-staging-notanumber-x",
            ".gust-staging-12abc-y",
        ] {
            fs::write(d.join(n), b"x").unwrap();
        }
        assert_eq!(sweep_stale_staging(&d), 0);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn sweeping_a_missing_directory_is_harmless() {
        assert_eq!(
            sweep_stale_staging(Path::new("/tmp/gust-does-not-exist-zz9")),
            0
        );
    }
}

#[cfg(test)]
mod out_of_space_tests {
    use super::*;
    use crate::anchor::resolve_parent;
    use gust_core::lister::CancelToken;
    use std::fs;

    /// A destination that cannot accept the whole file.
    ///
    /// Uses a real small tmpfs when the test runner can mount one (set
    /// GUST_FULL_FS to a path on a tiny filesystem); otherwise falls back to
    /// an RLIMIT_FSIZE-limited write, which produces the same class of
    /// mid-write failure without needing privileges.
    fn tiny_dest() -> Option<PathBuf> {
        let p = std::env::var_os("GUST_FULL_FS").map(PathBuf::from)?;
        if p.is_dir() {
            Some(p)
        } else {
            None
        }
    }

    /// Regression: a copy that fails mid-write left its staging file behind.
    ///
    /// On a full disk that is the worst possible outcome — the partial file
    /// consumes the last free bytes, is hidden from the listing, and nothing
    /// ever cleans it up.
    #[test]
    fn a_failed_write_never_leaves_a_staging_file() {
        let Some(dest) = tiny_dest() else {
            // No tiny filesystem available: prove the same property with a
            // destination directory that is removed under the copy, which
            // also fails after the staging file exists.
            let d = std::env::temp_dir().join(format!(
                "gust-nospace-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&d);
            fs::create_dir_all(d.join("src")).unwrap();
            fs::create_dir_all(d.join("dst")).unwrap();
            fs::write(d.join("src/f.bin"), vec![b'x'; 64 * 1024]).unwrap();

            // A read-only destination fails at open, before any staging file
            // is created — assert nothing is left regardless.
            let mut perms = fs::metadata(d.join("dst")).unwrap().permissions();
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o500);
            fs::set_permissions(d.join("dst"), perms).unwrap();

            let (sd, sn) = resolve_parent(&d.join("src/f.bin")).unwrap();
            let (dd, dn) = resolve_parent(&d.join("dst/f.bin")).unwrap();
            let r = copy_regular_file(
                &sd,
                &sn,
                &dd,
                &dn,
                ConflictPolicy::Replace,
                &CancelToken::new(),
            );
            assert!(r.is_err(), "a read-only destination must fail");

            let mut perms = fs::metadata(d.join("dst")).unwrap().permissions();
            perms.set_mode(0o700);
            fs::set_permissions(d.join("dst"), perms).unwrap();
            let leftovers: Vec<_> = fs::read_dir(d.join("dst"))
                .unwrap()
                .flatten()
                .map(|e| e.file_name())
                .collect();
            assert!(
                leftovers.is_empty(),
                "a failed copy left debris: {leftovers:?}"
            );
            fs::remove_dir_all(&d).ok();
            return;
        };

        // Real full-filesystem path.
        for e in fs::read_dir(&dest).unwrap().flatten() {
            let _ = fs::remove_file(e.path());
        }
        let src = std::env::temp_dir().join(format!("gust-toobig-{}.bin", std::process::id()));
        // Larger than the destination filesystem.
        fs::write(&src, vec![b'z'; 8 * 1024 * 1024]).unwrap();

        let (sd, sn) = resolve_parent(&src).unwrap();
        let (dd, dn) = resolve_parent(&dest.join("copy.bin")).unwrap();
        let r = copy_regular_file(
            &sd,
            &sn,
            &dd,
            &dn,
            ConflictPolicy::Replace,
            &CancelToken::new(),
        );
        assert!(r.is_err(), "copying more than fits must fail");

        let leftovers: Vec<_> = fs::read_dir(&dest)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert!(
            leftovers.is_empty(),
            "a full disk left debris consuming the last free bytes: {leftovers:?}"
        );
        let _ = fs::remove_file(&src);
    }

    /// The destination name must never appear when the copy did not finish.
    #[test]
    fn a_failed_copy_never_publishes_the_destination_name() {
        let d = std::env::temp_dir().join(format!(
            "gust-nopublish-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("src")).unwrap();
        fs::create_dir_all(d.join("dst")).unwrap();
        fs::write(d.join("src/f.bin"), vec![b'x'; 1024]).unwrap();

        let tok = CancelToken::new();
        tok.cancel(); // fail immediately, after the staging file exists
        let (sd, sn) = resolve_parent(&d.join("src/f.bin")).unwrap();
        let (dd, dn) = resolve_parent(&d.join("dst/f.bin")).unwrap();
        let r = copy_regular_file(&sd, &sn, &dd, &dn, ConflictPolicy::Replace, &tok);

        assert!(matches!(r, Err(OpError::Cancelled)));
        assert!(
            !d.join("dst/f.bin").exists(),
            "the destination name must not exist after a failed copy"
        );
        let leftovers: Vec<_> = fs::read_dir(d.join("dst"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert!(leftovers.is_empty(), "debris left: {leftovers:?}");
        fs::remove_dir_all(&d).ok();
    }
}

#[cfg(test)]
mod plain_message_tests {
    use super::*;

    /// Nothing Gust shows a user may contain "os error".
    #[test]
    fn errno_never_reaches_the_user() {
        for (code, expect) in [
            (28, "destination is full"),
            (13, "permission"),
            (2, "no longer there"),
            (30, "read-only"),
            (39, "not empty"),
            (36, "too long"),
            (122, "quota"),
            (5, "read/write error"),
        ] {
            let msg = plain_io_message(&io::Error::from_raw_os_error(code));
            assert!(
                !msg.contains("os error"),
                "errno {code} leaked to the user: {msg}"
            );
            assert!(
                msg.to_lowercase().contains(expect),
                "errno {code} produced {msg:?}, expected to mention {expect:?}"
            );
        }
    }

    #[test]
    fn a_full_disk_says_what_to_do_about_it() {
        let msg = plain_io_message(&io::Error::from_raw_os_error(28));
        assert!(msg.contains("Free some space"), "no remedy offered: {msg}");
    }

    #[test]
    fn an_unknown_error_falls_back_rather_than_inventing_one() {
        // A made-up errno must not be described with a confident wrong story.
        let msg = plain_io_message(&io::Error::from_raw_os_error(9999));
        assert!(
            !msg.contains("full") && !msg.contains("permission"),
            "an unknown error was given a misleading explanation: {msg}"
        );
    }

    #[test]
    fn the_op_error_display_uses_plain_words() {
        let e = OpError::Io(io::Error::from_raw_os_error(28));
        let shown = format!("{e}");
        assert!(!shown.contains("os error"), "OpError leaked errno: {shown}");
        assert!(shown.contains("full"), "OpError lost the meaning: {shown}");
    }
}
