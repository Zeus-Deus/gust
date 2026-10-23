//! Descriptor-anchored path resolution.
//!
//! The counterexample this exists for (plan §10): an attacker replaces an
//! *intermediate* directory mid-operation. Opening the final component with
//! `O_NOFOLLOW` does not help — the redirect already happened earlier in the
//! path. So Gust walks the path one component at a time, holding a descriptor
//! for each, and every step refuses to traverse a symlink.

use rustix::fd::{AsFd, OwnedFd};
use rustix::fs::{Mode, OFlags};
use std::ffi::OsStr;
use std::io;
use std::path::{Component, Path};

/// A directory descriptor that is guaranteed to be the directory we walked to,
/// not whatever currently lives at that path string.
#[derive(Debug)]
pub struct DirHandle {
    fd: OwnedFd,
    path: std::path::PathBuf,
    /// Kept for diagnostics only — never re-resolved for I/O.
    display: String,
}

impl DirHandle {
    pub fn as_fd(&self) -> rustix::fd::BorrowedFd<'_> {
        self.fd.as_fd()
    }

    pub fn display(&self) -> &str {
        &self.display
    }

    /// Lossless lexical path for mount evidence and recovery records only.
    /// Filesystem mutations continue to use the held descriptor.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn fstype(&self) -> Option<String> {
        fstype_for_fd(&self.fd)
    }

    /// Open the root `/` as the walk anchor.
    pub fn open_root() -> io::Result<Self> {
        let fd = rustix::fs::open(
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        Ok(DirHandle {
            fd,
            path: "/".into(),
            display: "/".to_string(),
        })
    }

    /// Open a subdirectory *relative to this handle*, refusing symlinks.
    pub fn open_child_dir(&self, name: &OsStr) -> io::Result<Self> {
        let fd = rustix::fs::openat(
            self.fd.as_fd(),
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        Ok(DirHandle {
            fd,
            path: self.path.join(name),
            display: format!(
                "{}/{}",
                self.display.trim_end_matches('/'),
                name.to_string_lossy()
            ),
        })
    }

    /// `fstatat` a child without following symlinks.
    pub fn stat_child(&self, name: &OsStr) -> io::Result<rustix::fs::Stat> {
        Ok(rustix::fs::statat(
            self.fd.as_fd(),
            name,
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        )?)
    }

    /// Identity of this directory itself.
    pub fn stat_self(&self) -> io::Result<rustix::fs::Stat> {
        Ok(rustix::fs::fstat(self.fd.as_fd())?)
    }
}

/// Match the held mount, including after an ancestor rename. Proc reads are
/// local and bounded; detached, missing or ambiguous mount evidence is Unknown.
pub(crate) fn fstype_for_fd(fd: impl AsFd) -> Option<String> {
    use rustix::fd::AsRawFd;
    use std::io::Read;
    fn bounded(path: impl AsRef<Path>, limit: usize) -> Option<Vec<u8>> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        (bytes.len() <= limit).then_some(bytes)
    }
    let fdinfo = bounded(
        format!("/proc/self/fdinfo/{}", fd.as_fd().as_raw_fd()),
        64 * 1024,
    )?;
    let mounts = bounded(
        "/proc/self/mountinfo",
        gust_core::mount_table::MAX_MOUNTINFO_BYTES,
    )?;
    fstype_from_fdinfo(&fdinfo, &mounts)
}

fn fstype_from_fdinfo(fdinfo: &[u8], mounts: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(fdinfo).ok()?;
    let mut ids = text.lines().filter_map(|l| l.strip_prefix("mnt_id:"));
    let id: u64 = ids.next()?.trim().parse().ok()?;
    if id == 0 || ids.next().is_some() {
        return None;
    }
    let table = gust_core::mount_table::MountInfo::parse(mounts).ok()?;
    table
        .visible_mounts()
        .ok()?
        .into_iter()
        .find(|m| m.id == id)
        .map(|m| m.fs_type.clone())
}

#[cfg(test)]
thread_local! {
    /// One-shot kernel-support injection; filesystem effects stay real.
    pub(crate) static UNSUPPORTED_ERRNO: std::cell::Cell<Option<rustix::io::Errno>> =
        const { std::cell::Cell::new(None) };
}

/// Rename that can never replace what is already at the destination.
///
/// `renameat2(RENAME_NOREPLACE)` makes "is the name free?" and "take it" one
/// atomic step, so a file that appears between a check and the rename is
/// refused with `AlreadyExists` instead of silently destroyed. A filesystem
/// or kernel without the primitive is refused, for every source type. Neither
/// check-then-rename nor link-then-unlink is a safe atomic-move substitute.
/// This guards destination occupancy, not concurrent source identity changes.
pub fn rename_noreplace(
    from_dir: &DirHandle,
    from: &OsStr,
    to_dir: &DirHandle,
    to: &OsStr,
) -> io::Result<()> {
    use rustix::fs::RenameFlags;
    use rustix::io::Errno;
    #[cfg(test)]
    let unsupported = UNSUPPORTED_ERRNO.with(|e| e.take());
    #[cfg(not(test))]
    let unsupported: Option<Errno> = None;
    let result = match unsupported {
        Some(e) => Err(e),
        None => rustix::fs::renameat_with(
            from_dir.as_fd(),
            from,
            to_dir.as_fd(),
            to,
            RenameFlags::NOREPLACE,
        ),
    };
    match result {
        Ok(()) => Ok(()),
        Err(Errno::INVAL) | Err(Errno::NOSYS) | Err(Errno::OPNOTSUPP) => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "This place cannot safely add this item without risking another item. The original was kept. Choose a different destination.",
        )),
        Err(e) => Err(e.into()),
    }
}

/// [`rename_noreplace`] for two absolute paths, each resolved without
/// following a symlink at any level.
pub fn rename_noreplace_paths(from: &Path, to: &Path) -> io::Result<()> {
    let (fd, fname) = resolve_parent(from)?;
    let (td, tname) = resolve_parent(to)?;
    rename_noreplace(&fd, &fname, &td, &tname)
}

/// Publish a completed regular-file stage by adding a name atomically.
/// Unlike a move, success leaves the staging alias for separate cleanup.
/// Empty flags deliberately do not follow a substituted source symlink.
pub(crate) fn link_noreplace(
    dir: &DirHandle,
    stage: &OsStr,
    destination: &OsStr,
) -> io::Result<()> {
    #[cfg(test)]
    if let Some(e) = LINK_ERROR.with(|v| v.take()) {
        return Err(e.into());
    }
    rustix::fs::linkat(
        dir.as_fd(),
        stage,
        dir.as_fd(),
        destination,
        rustix::fs::AtFlags::empty(),
    )?;
    #[cfg(test)]
    if let Some(e) = LINK_REPLY_ERROR.with(|v| v.take()) {
        return Err(e.into());
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    pub(crate) static LINK_ERROR: std::cell::Cell<Option<rustix::io::Errno>> = const { std::cell::Cell::new(None) };
    pub(crate) static LINK_REPLY_ERROR: std::cell::Cell<Option<rustix::io::Errno>> = const { std::cell::Cell::new(None) };
}

/// Walk an absolute path component-by-component, holding a descriptor at each
/// step and refusing to cross any symlink.
///
/// Returns the descriptor of the *parent* directory plus the final component
/// name, which is what every safe mutation (`openat`, `unlinkat`, `renameat`)
/// actually needs.
pub fn resolve_parent(path: &Path) -> io::Result<(DirHandle, std::ffi::OsString)> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path must be absolute",
        ));
    }
    let mut comps: Vec<&OsStr> = Vec::new();
    for c in path.components() {
        match c {
            Component::RootDir => {}
            Component::Normal(n) => comps.push(n),
            Component::CurDir => {}
            // `..` is refused: it would let a traversal escape the anchor.
            Component::ParentDir => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "'..' is not allowed in an anchored path",
                ))
            }
            Component::Prefix(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "unexpected path prefix",
                ))
            }
        }
    }
    let Some((last, parents)) = comps.split_last() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path has no final component",
        ));
    };

    let mut dir = DirHandle::open_root()?;
    for c in parents {
        // NOFOLLOW at EVERY level — this is the whole point.
        dir = dir.open_child_dir(c)?;
    }
    Ok((dir, last.to_os_string()))
}

/// Identity used to prove a file has not been swapped between two points in
/// time (DESIGN.md §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIdentity {
    pub dev: u64,
    pub ino: u64,
    pub mtime: i64,
    pub mtime_nsec: i64,
    pub size: u64,
    pub mode: u32,
}

impl FileIdentity {
    pub fn from_stat(st: &rustix::fs::Stat) -> Self {
        FileIdentity {
            dev: st.st_dev,
            ino: st.st_ino,
            mtime: st.st_mtime,
            mtime_nsec: st.st_mtime_nsec as i64,
            size: st.st_size as u64,
            mode: st.st_mode,
        }
    }

    pub fn of_path(path: &Path) -> io::Result<Self> {
        let (dir, name) = resolve_parent(path)?;
        Ok(Self::from_stat(&dir.stat_child(&name)?))
    }

    /// Same file, unchanged — required before any destructive undo step.
    pub fn matches(&self, other: &FileIdentity) -> bool {
        self.dev == other.dev
            && self.ino == other.ino
            && self.mtime == other.mtime
            && self.mtime_nsec == other.mtime_nsec
            && self.size == other.size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("gust-anchor-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn resolves_a_normal_path() {
        let d = tmp("normal");
        fs::create_dir(d.join("sub")).unwrap();
        fs::write(d.join("sub/f.txt"), b"hello").unwrap();
        let (dir, name) = resolve_parent(&d.join("sub/f.txt")).unwrap();
        assert_eq!(name, OsStr::new("f.txt"));
        let st = dir.stat_child(&name).unwrap();
        assert_eq!(st.st_size as u64, 5);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn refuses_symlinked_intermediate_directory() {
        // THE counterexample from plan §10. Final-component O_NOFOLLOW alone
        // would happily write through `link/` into the attacker's target.
        let d = tmp("swap");
        fs::create_dir(d.join("real")).unwrap();
        fs::create_dir(d.join("attacker")).unwrap();
        fs::write(d.join("attacker/f.txt"), b"attacker owns this").unwrap();
        // "link" is a symlink standing in as an intermediate directory.
        std::os::unix::fs::symlink(d.join("attacker"), d.join("link")).unwrap();

        let err = resolve_parent(&d.join("link/f.txt")).unwrap_err();
        // The kernel refuses O_DIRECTORY|O_NOFOLLOW on a symlink with ENOTDIR
        // (the symlink is not a directory); a NOFOLLOW-only open reports
        // ELOOP. Either way the traversal is REFUSED — that is the property
        // under test, so accept both rather than pinning one errno.
        let e = err.raw_os_error();
        assert!(
            e == Some(rustix::io::Errno::NOTDIR.raw_os_error())
                || e == Some(rustix::io::Errno::LOOP.raw_os_error()),
            "traversing a symlinked intermediate dir must be refused, got {err:?}"
        );
        // And the attacker's file was never reached.
        assert_eq!(
            fs::read(d.join("attacker/f.txt")).unwrap(),
            b"attacker owns this"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn refuses_dotdot() {
        let e = resolve_parent(Path::new("/tmp/../etc/passwd")).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn refuses_relative_path() {
        assert!(resolve_parent(Path::new("relative/path")).is_err());
    }

    #[test]
    fn identity_detects_replacement() {
        let d = tmp("identity");
        let f = d.join("f.txt");
        fs::write(&f, b"one").unwrap();
        let before = FileIdentity::of_path(&f).unwrap();

        // Replace the file with a different one at the same path.
        fs::remove_file(&f).unwrap();
        fs::write(&f, b"two").unwrap();
        let after = FileIdentity::of_path(&f).unwrap();

        assert!(!before.matches(&after), "swapped file must not match");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn identity_matches_untouched_file() {
        let d = tmp("same");
        let f = d.join("f.txt");
        fs::write(&f, b"stable").unwrap();
        let a = FileIdentity::of_path(&f).unwrap();
        let b = FileIdentity::of_path(&f).unwrap();
        assert!(a.matches(&b));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn identity_detects_fifo_replacing_regular_file() {
        // Filename-to-FIFO replacement (DESIGN.md §6).
        let d = tmp("fifo");
        let f = d.join("f");
        fs::write(&f, b"data").unwrap();
        let before = FileIdentity::of_path(&f).unwrap();
        fs::remove_file(&f).unwrap();
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &f,
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_bits_truncate(0o600),
            0,
        )
        .unwrap();
        let after = FileIdentity::of_path(&f).unwrap();
        assert!(!before.matches(&after));
        assert_ne!(before.mode & 0o170000, after.mode & 0o170000);
        fs::remove_dir_all(&d).ok();
    }
}

#[cfg(test)]
mod byte_path_tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn mount_evidence_path_preserves_bytes_hidden_by_diagnostic_labels() {
        let root = std::env::temp_dir().join(format!("gust-anchor-bytes-{}", std::process::id()));
        let parent = root.join(std::ffi::OsString::from_vec(b"place-\xff".to_vec()));
        std::fs::create_dir_all(&parent).unwrap();
        let (dir, _) = resolve_parent(&parent.join("final")).unwrap();
        assert_eq!(dir.path(), parent);
        assert_ne!(dir.path().as_os_str(), OsStr::new(dir.display()));
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod held_mount_tests {
    use super::*;

    #[test]
    fn renamed_mount_path_cannot_reclassify_a_held_remote_descriptor_as_local() {
        let mounts =
            b"10 1 0:1 / / rw - ext4 root rw\n20 10 0:2 / /moved/place rw - fuse.sshfs remote rw\n";
        let table = gust_core::mount_table::MountInfo::parse(mounts).unwrap();
        assert_eq!(
            table.resolve(Path::new("/old/place")).unwrap().fs_type,
            "ext4"
        );
        assert_eq!(
            fstype_from_fdinfo(b"pos:\t0\nmnt_id:\t20\n", mounts).as_deref(),
            Some("fuse.sshfs")
        );
        assert_eq!(fstype_from_fdinfo(b"mnt_id:\t99\n", mounts), None);
        assert_eq!(
            fstype_from_fdinfo(b"mnt_id:\t20\nmnt_id:\t10\n", mounts),
            None
        );
        assert_eq!(fstype_from_fdinfo(b"mnt_id:\tinvalid\n", mounts), None);
        assert_eq!(fstype_from_fdinfo(b"mnt_id:\t20\n", b"truncated"), None);
    }
}
