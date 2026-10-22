//! Complete bounded identity inventories. A read error is never an empty tree.
use crate::anchor::{self, DirHandle, FileIdentity};
use gust_core::lister::CancelToken;
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    io,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};
pub(crate) fn open_dir(path: &Path) -> io::Result<DirHandle> {
    if path == Path::new("/") {
        DirHandle::open_root()
    } else {
        let (parent, name) = anchor::resolve_parent(path)?;
        parent.open_child_dir(&name)
    }
}
pub(crate) fn snapshot(
    root: &Path,
    cancel: &CancelToken,
) -> io::Result<BTreeMap<PathBuf, FileIdentity>> {
    let (parent, name) = anchor::resolve_parent(root)?;
    snapshot_at(&parent, &name, root, cancel)
}

pub(crate) fn snapshot_at(
    parent: &DirHandle,
    name: &OsStr,
    root: &Path,
    cancel: &CancelToken,
) -> io::Result<BTreeMap<PathBuf, FileIdentity>> {
    fn check(cancel: &CancelToken) -> io::Result<()> {
        if cancel.is_cancelled() {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Identity snapshot cancelled",
            ))
        } else {
            Ok(())
        }
    }
    fn walk(
        parent: &DirHandle,
        name: &OsStr,
        path: &Path,
        depth: usize,
        out: &mut BTreeMap<PathBuf, FileIdentity>,
        metadata: &mut usize,
        cancel: &CancelToken,
    ) -> io::Result<()> {
        check(cancel)?;
        *metadata = metadata.saturating_add(path.as_os_str().as_bytes().len() + 128);
        if depth > 64 || out.len() >= 200_000 || *metadata > 32 * 1024 * 1024 {
            return Err(io::Error::other(
                "Identity inventory exceeds resource limits",
            ));
        }
        let before = FileIdentity::from_stat(&parent.stat_child(name)?);
        out.insert(path.to_path_buf(), before);
        if before.mode & 0o170000 == 0o040000 {
            let dir = parent.open_child_dir(name)?;
            if !before.matches(&FileIdentity::from_stat(&dir.stat_self()?)) {
                return Err(io::Error::other(
                    "Directory changed during identity inventory",
                ));
            }
            for entry in rustix::fs::Dir::read_from(dir.as_fd())? {
                let entry = entry?;
                let bytes = entry.file_name().to_bytes();
                if bytes == b"." || bytes == b".." {
                    continue;
                }
                let name = OsStr::from_bytes(bytes);
                walk(
                    &dir,
                    name,
                    &path.join(name),
                    depth + 1,
                    out,
                    metadata,
                    cancel,
                )?;
            }
            if !before.matches(&FileIdentity::from_stat(&dir.stat_self()?)) {
                return Err(io::Error::other(
                    "Directory changed during identity inventory",
                ));
            }
        }
        if !before.matches(&FileIdentity::from_stat(&parent.stat_child(name)?)) {
            return Err(io::Error::other("Item changed during identity inventory"));
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(parent, name, root, 0, &mut out, &mut 0, cancel)?;
    Ok(out)
}

/// Create missing components relative to held parents, never through a link.
pub(crate) fn ensure_dir(path: &Path) -> io::Result<DirHandle> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "An absolute directory is required",
        ));
    }
    for component in path.components() {
        if !matches!(
            component,
            std::path::Component::RootDir
                | std::path::Component::Normal(_)
                | std::path::Component::CurDir
        ) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Unsafe directory component",
            ));
        }
    }
    let mut dir = DirHandle::open_root()?;
    for component in path.components() {
        if let std::path::Component::Normal(name) = component {
            match rustix::fs::mkdirat(
                dir.as_fd(),
                name,
                rustix::fs::Mode::from_bits_truncate(0o700),
            ) {
                Ok(()) => rustix::fs::fsync(dir.as_fd())?,
                Err(rustix::io::Errno::EXIST) => {}
                Err(e) => return Err(e.into()),
            }
            dir = dir.open_child_dir(name)?;
        }
    }
    Ok(dir)
}
