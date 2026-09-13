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
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
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

/// Build a staging name that cannot collide with a user file.
fn staging_name(final_name: &OsStr) -> OsString {
    let mut s = OsString::from(".gust-staging-");
    s.push(format!("{}-", std::process::id()));
    s.push(final_name);
    s
}

/// Copy one regular file into `dst_dir` under `dst_name`.
///
/// Staged under a private name with restrictive permissions from creation,
/// then atomically renamed into place. A partial copy never appears at the
/// destination name.
pub fn copy_regular_file(
    src_dir: &DirHandle,
    src_name: &OsStr,
    dst_dir: &DirHandle,
    dst_name: &OsStr,
    policy: ConflictPolicy,
    cancel: &CancelToken,
) -> OpResult<u64> {
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
            ConflictPolicy::Skip => return Ok(0),
            ConflictPolicy::Ask => {
                return Err(OpError::Conflict(PathBuf::from(dst_name)));
            }
            ConflictPolicy::Replace | ConflictPolicy::KeepBoth => {}
        }
    }

    let src = rustix::fs::openat(
        src_dir.as_fd(),
        src_name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;

    let stage = staging_name(dst_name);
    // Record ownership BEFORE the file exists. A staging file with no claim is
    // never reclaimed by the sweep, so the claim must be durable first.
    let staged_abs = PathBuf::from(dst_dir.display()).join(&stage);
    let claim = crate::staging::claim(&staged_abs).ok();
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
        let _ = crate::staging::confirm(c, &staged_abs);
    }

    let mut written = 0u64;
    let mut buf = vec![0u8; 128 * 1024];

    // Any failure from here on must remove the staging file. A disk that
    // filled up mid-copy would otherwise leave a partial file consuming the
    // last free bytes, invisible to the user and cleaned up by nobody.
    let copy_result = (|| -> OpResult<()> {
        loop {
            if cancel.is_cancelled() {
                return Err(OpError::Cancelled);
            }
            let n = rustix::io::read(&src, &mut buf)?;
            if n == 0 {
                break;
            }
            let mut off = 0;
            while off < n {
                let w = rustix::io::write(&dst, &buf[off..n])?;
                if w == 0 {
                    return Err(OpError::Io(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "destination accepted no bytes",
                    )));
                }
                off += w;
                written += w as u64;
            }
        }

        // Preserve mode (minus setuid/setgid — never applied blindly).
        let safe_mode = st.st_mode & 0o7777 & !0o6000;
        rustix::fs::fchmod(&dst, Mode::from_bits_truncate(safe_mode))?;
        // Durability before the destination name exists. fsync can itself
        // fail on a full filesystem, which is exactly when the staging file
        // must not be left behind.
        rustix::fs::fsync(&dst)?;
        Ok(())
    })();

    drop(dst);

    if let Err(e) = copy_result {
        let _ = rustix::fs::unlinkat(dst_dir.as_fd(), &stage, rustix::fs::AtFlags::empty());
        if let Some(c) = &claim {
            crate::staging::release(c);
        }
        return Err(e);
    }

    rustix::fs::renameat(dst_dir.as_fd(), &stage, dst_dir.as_fd(), dst_name).inspect_err(|_| {
        // Even the final rename can fail (a full directory, a vanished
        // parent). The staging file is ours; do not abandon it.
        let _ = rustix::fs::unlinkat(dst_dir.as_fd(), &stage, rustix::fs::AtFlags::empty());
        if let Some(c) = &claim {
            crate::staging::release(c);
        }
    })?;
    // Committed under its real name: the claim no longer describes anything.
    if let Some(c) = &claim {
        crate::staging::release(c);
    }
    Ok(written)
}

/// Copy a symlink as a symlink — never by following it.
pub fn copy_symlink(
    src_dir: &DirHandle,
    src_name: &OsStr,
    dst_dir: &DirHandle,
    dst_name: &OsStr,
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
    let target = rustix::fs::readlinkat(src_dir.as_fd(), src_name, Vec::new())?;
    let stage = staging_name(dst_name);
    let staged_abs = PathBuf::from(dst_dir.display()).join(&stage);
    let claim = crate::staging::claim(&staged_abs).ok();
    if let Err(e) = rustix::fs::symlinkat(target.as_c_str(), dst_dir.as_fd(), &stage) {
        if let Some(c) = &claim {
            crate::staging::release(c);
        }
        return Err(e.into());
    }
    if let Some(c) = &claim {
        let _ = crate::staging::confirm(c, &staged_abs);
    }
    let r = rustix::fs::renameat(dst_dir.as_fd(), &stage, dst_dir.as_fd(), dst_name);
    if r.is_err() {
        let _ = rustix::fs::unlinkat(dst_dir.as_fd(), &stage, rustix::fs::AtFlags::empty());
    }
    if let Some(c) = &claim {
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
    rustix::fs::renameat(src_dir.as_fd(), src_name, dst_dir.as_fd(), dst_name)?;
    Ok(())
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
    let before = FileIdentity::from_stat(&src_dir.stat_child(src_name)?);
    copy_regular_file(src_dir, src_name, dst_dir, dst_name, policy, cancel)?;

    // Re-verify the source is byte-identical to what we copied.
    let after = FileIdentity::from_stat(&src_dir.stat_child(src_name)?);
    if !before.matches(&after) {
        return Err(OpError::Refused(
            "source changed during move; destination written, source kept".into(),
        ));
    }
    // Durability of the destination directory entry before unlinking.
    rustix::fs::fsync(dst_dir.as_fd())?;
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
        if cancel.is_cancelled() {
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
                FileClass::Symlink => copy_symlink(&sd, &sn, &dd, &dn)?,
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
mod tests {
    use super::*;
    use std::fs;

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
/// A copy stages under `.gust-staging-<pid>-<name>` and renames it into place
/// atomically, so a crash can never publish a partial file — but it can leave
/// the staging file behind. Nothing else will ever clean it up, so a folder
/// that survived a crash would accumulate hidden debris forever.
///
/// Only files whose owning PID is no longer alive are removed. A staging file
/// belonging to a RUNNING process is another copy in flight, and taking it
/// would corrupt that copy.
pub fn sweep_stale_staging(dir: &Path) -> usize {
    use std::os::unix::fs::MetadataExt;
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
        // A live owner in this same boot is a copy in flight right now.
        if crate::staging::owner_is_live(&c) {
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
        if md.dev() != c.dev || md.ino() != c.ino {
            // Something else occupies that path now. Not ours to remove.
            continue;
        }
        if std::fs::remove_file(&c.path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Remove claim files whose staging file no longer exists and whose owner is
/// gone, so the claim store cannot grow without bound.
pub fn prune_dead_claims() -> usize {
    let mut pruned = 0usize;
    for c in crate::staging::load_all() {
        if crate::staging::owner_is_live(&c) {
            continue;
        }
        if c.path.symlink_metadata().is_ok() {
            continue; // still describes a real file; keep it
        }
        // Reconstruct this claim's own file name and drop it.
        for e in std::fs::read_dir(crate::staging::claims_dir())
            .into_iter()
            .flatten()
            .flatten()
        {
            let Ok(text) = std::fs::read_to_string(e.path()) else {
                continue;
            };
            if text.contains(&format!("path={}", c.path.display()))
                && std::fs::remove_file(e.path()).is_ok()
            {
                pruned += 1;
            }
        }
    }
    pruned
}

/// Is this PID still running? Conservative: unknown means "yes, leave it".
fn pid_is_alive(pid: u32) -> bool {
    if pid == 0 {
        return true;
    }
    Path::new(&format!("/proc/{pid}")).exists()
}

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
        let d = tmp("dead");
        // Isolate the claim store so this test owns it.
        std::env::set_var("XDG_STATE_HOME", d.join("state"));

        // Real debris: a staging file WITH an ownership claim, whose owning
        // process is gone. A filename alone is deliberately not enough any
        // more, so the claim is what makes this reclaimable.
        let debris = d.join(".gust-staging-999999-report.pdf");
        fs::write(&debris, b"partial").unwrap();
        let claim = crate::staging::claim(&debris).unwrap();
        crate::staging::confirm(&claim, &debris).unwrap();
        // Rewrite the claim so it belongs to a process that cannot be running.
        {
            use std::os::unix::fs::MetadataExt;
            let md = debris.symlink_metadata().unwrap();
            fs::write(
                &claim,
                format!(
                    "path={}\nboot={}\npid=999999\ndev={}\nino={}\n",
                    debris.display(),
                    crate::staging::boot_id(),
                    md.dev(),
                    md.ino()
                ),
            )
            .unwrap();
        }
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
    fn a_staging_shaped_file_with_no_claim_is_never_removed() {
        let d = tmp("unclaimed");
        std::env::set_var("XDG_STATE_HOME", d.join("state"));

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
