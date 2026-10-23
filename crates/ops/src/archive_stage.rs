//! Exclusively created, descriptor-bound archive staging.
use crate::anchor::{DirHandle, FileIdentity};
use rustix::fs::{AtFlags, Mode, OFlags};
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) struct Stage {
    pub parent: DirHandle,
    pub dir: DirHandle,
    pub name: OsString,
    identity: FileIdentity,
}

impl Stage {
    pub fn create(destination: &Path) -> io::Result<Self> {
        let parent = open_dir(destination)?;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..64 {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_nanos();
            let name = OsString::from(format!(
                ".gust-extract-{}-{stamp}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match rustix::fs::mkdirat(parent.as_fd(), &name, Mode::from_bits_truncate(0o700)) {
                Ok(()) => {
                    let created = parent.stat_child(&name)?;
                    let dir = parent.open_child_dir(&name)?;
                    let identity = FileIdentity::from_stat(&dir.stat_self()?);
                    if identity.dev != created.st_dev || identity.ino != created.st_ino {
                        return Err(io::Error::other("archive staging changed during creation"));
                    }
                    return Ok(Self {
                        parent,
                        dir,
                        name,
                        identity,
                    });
                }
                Err(rustix::io::Errno::EXIST) => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(io::Error::other("cannot create private archive staging"))
    }

    pub fn cleanup(&self) -> io::Result<()> {
        let current = self.parent.stat_child(&self.name)?;
        if current.st_dev != self.identity.dev || current.st_ino != self.identity.ino {
            return Err(io::Error::other(
                "archive staging ownership changed; left in place",
            ));
        }
        clear(&self.dir)?;
        rustix::fs::unlinkat(self.parent.as_fd(), &self.name, AtFlags::REMOVEDIR)?;
        rustix::fs::fsync(self.parent.as_fd())?;
        Ok(())
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

fn clear(dir: &DirHandle) -> io::Result<()> {
    let iter = rustix::fs::Dir::read_from(dir.as_fd())?;
    for entry in iter {
        let entry = entry?;
        let raw = entry.file_name().to_bytes();
        if raw == b"." || raw == b".." {
            continue;
        }
        let name = <OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(raw);
        let st = dir.stat_child(name)?;
        let directory = st.st_mode & 0o170000 == 0o040000;
        if directory {
            clear(&dir.open_child_dir(name)?)?;
        }
        rustix::fs::unlinkat(
            dir.as_fd(),
            name,
            if directory {
                AtFlags::REMOVEDIR
            } else {
                AtFlags::empty()
            },
        )?;
    }
    Ok(())
}

pub(super) fn open_dir(path: &Path) -> io::Result<DirHandle> {
    if path == Path::new("/") {
        return DirHandle::open_root();
    }
    let (parent, name) = crate::anchor::resolve_parent(path)?;
    parent.open_child_dir(&name)
}

/// Open or create every directory relative to a held root; no path symlink
/// can redirect publication. Return all directory names actually created.
pub(super) fn parents(
    root: &DirHandle,
    path: &Path,
    create: bool,
) -> io::Result<(DirHandle, Vec<PathBuf>)> {
    parents_with(root, path, create, &mut |_, _| {})
}

pub(super) fn parents_with(
    root: &DirHandle,
    path: &Path,
    create: bool,
    on_created: &mut dyn FnMut(&Path, FileIdentity),
) -> io::Result<(DirHandle, Vec<PathBuf>)> {
    let mut dir = root.open_child_dir(OsStr::new("."))?;
    let mut rel = PathBuf::new();
    let mut made = Vec::new();
    for component in path.components() {
        let Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unsafe archive path",
            ));
        };
        rel.push(name);
        if create {
            match rustix::fs::mkdirat(dir.as_fd(), name, Mode::from_bits_truncate(0o700)) {
                Ok(()) => {
                    made.push(rel.clone());
                    on_created(&rel, FileIdentity::from_stat(&dir.stat_child(name)?));
                    rustix::fs::fsync(dir.as_fd())?;
                }
                Err(rustix::io::Errno::EXIST) => {}
                Err(e) => return Err(e.into()),
            }
        }
        dir = dir.open_child_dir(name)?;
    }
    Ok((dir, made))
}

pub(super) fn create_file(dir: &DirHandle, name: &OsStr) -> io::Result<std::fs::File> {
    Ok(std::fs::File::from(rustix::fs::openat(
        dir.as_fd(),
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_bits_truncate(0o600),
    )?))
}
