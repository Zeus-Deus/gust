//! Irreversible deletion with owned staging, truthful counts and cancellation.
use super::{DeleteRefusal, Removed};
use crate::{
    anchor::{self, DirHandle, FileIdentity},
    journal::RecordedIdentity,
};
use gust_core::lister::CancelToken;
use rustix::fs::{AtFlags, Mode, OFlags};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs::File,
    io::{self, Write},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletePhase {
    Checking,
    Deleting,
    Finished,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeleteProgress {
    pub phase: DeletePhase,
    pub total: usize,
    pub removed: Removed,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteFailure {
    Refused(DeleteRefusal),
    Cancelled,
    Changed,
    Limit,
    Unsupported,
    Io(io::ErrorKind),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteError {
    pub reason: DeleteFailure,
    pub removed: Removed,
    pub retained: Vec<PathBuf>,
}
impl std::fmt::Display for DeleteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.reason {
            DeleteFailure::Refused(reason) => f.write_str(&reason.message())?,
            DeleteFailure::Cancelled => f.write_str("Permanent deletion cancelled.")?,
            DeleteFailure::Changed => f.write_str("An item changed during permanent deletion.")?,
            DeleteFailure::Limit => {
                f.write_str("Permanent deletion reached its resource limit.")?
            }
            DeleteFailure::Unsupported => {
                f.write_str("This location cannot safely perform permanent deletion.")?
            }
            DeleteFailure::Io(kind) => {
                write!(f, "Permanent deletion stopped after an I/O error ({kind}).")?
            }
        }
        write!(
            f,
            " {} files and {} folders removed",
            self.removed.files, self.removed.dirs
        )?;
        if !self.retained.is_empty() {
            write!(f, "; inspect {:?}", self.retained)?;
        }
        Ok(())
    }
}
impl std::error::Error for DeleteError {}
fn io_failure(error: io::Error) -> DeleteFailure {
    if matches!(
        error.kind(),
        io::ErrorKind::Unsupported | io::ErrorKind::CrossesDevices
    ) {
        DeleteFailure::Unsupported
    } else {
        DeleteFailure::Io(error.kind())
    }
}
fn check(cancel: &CancelToken) -> Result<(), DeleteFailure> {
    if cancel.is_cancelled() {
        Err(DeleteFailure::Cancelled)
    } else {
        Ok(())
    }
}
fn identity(parent: &DirHandle, name: &OsStr) -> Result<FileIdentity, DeleteFailure> {
    parent
        .stat_child(name)
        .map(|s| FileIdentity::from_stat(&s))
        .map_err(io_failure)
}
type Tree = BTreeMap<PathBuf, FileIdentity>;
fn mount_id(parent: &DirHandle, name: &OsStr) -> Result<u64, DeleteFailure> {
    #[cfg(test)]
    if NO_MOUNT_ID.with(|fail| fail.get()) {
        return Err(DeleteFailure::Unsupported);
    }
    let stat = rustix::fs::statx(
        parent.as_fd(),
        name,
        AtFlags::EMPTY_PATH | AtFlags::SYMLINK_NOFOLLOW | AtFlags::NO_AUTOMOUNT,
        rustix::fs::StatxFlags::MNT_ID,
    )
    .map_err(|error| io_failure(error.into()))?;
    if stat.stx_mask & rustix::fs::StatxFlags::MNT_ID.bits() == 0 {
        return Err(DeleteFailure::Unsupported);
    }
    #[cfg(test)]
    if CROSS_MOUNT.with(|cross| cross.get()) && name == OsStr::new("mounted") {
        return Ok(stat.stx_mnt_id.wrapping_add(1));
    }
    Ok(stat.stx_mnt_id)
}
fn snapshot(
    parent: &DirHandle,
    name: &OsStr,
    cancel: &CancelToken,
    progress: &mut impl FnMut(DeleteProgress),
) -> Result<Tree, DeleteFailure> {
    struct Walk<'a, F> {
        tree: Tree,
        metadata: usize,
        dev: u64,
        mount: u64,
        cancel: &'a CancelToken,
        progress: &'a mut F,
    }
    impl<F: FnMut(DeleteProgress)> Walk<'_, F> {
        fn visit(
            &mut self,
            parent: &DirHandle,
            name: &OsStr,
            relative: &Path,
            depth: usize,
        ) -> Result<(), DeleteFailure> {
            check(self.cancel)?;
            self.metadata = self
                .metadata
                .saturating_add(relative.as_os_str().as_bytes().len() + 128);
            if depth > 64 || self.tree.len() >= 200_000 || self.metadata > 32 * 1024 * 1024 {
                return Err(DeleteFailure::Limit);
            }
            let id = identity(parent, name)?;
            if id.dev != self.dev || mount_id(parent, name)? != self.mount {
                return Err(DeleteFailure::Unsupported);
            }
            self.tree.insert(relative.to_path_buf(), id);
            (self.progress)(DeleteProgress {
                phase: DeletePhase::Checking,
                total: self.tree.len(),
                removed: Removed::default(),
            });
            check(self.cancel)?;
            if id.mode & 0o170000 == 0o040000 {
                let dir = parent.open_child_dir(name).map_err(io_failure)?;
                if FileIdentity::from_stat(&dir.stat_self().map_err(io_failure)?) != id
                    || mount_id(&dir, OsStr::new(""))? != self.mount
                {
                    return Err(DeleteFailure::Changed);
                }
                for entry in
                    rustix::fs::Dir::read_from(dir.as_fd()).map_err(|e| io_failure(e.into()))?
                {
                    let entry = entry.map_err(|e| io_failure(e.into()))?;
                    let name = entry.file_name().to_bytes();
                    if matches!(name, b"." | b"..") {
                        continue;
                    }
                    let name = OsStr::from_bytes(name);
                    self.visit(&dir, name, &relative.join(name), depth + 1)?;
                }
                if FileIdentity::from_stat(&dir.stat_self().map_err(io_failure)?) != id {
                    return Err(DeleteFailure::Changed);
                }
            }
            if identity(parent, name)? != id || mount_id(parent, name)? != self.mount {
                return Err(DeleteFailure::Changed);
            }
            Ok(())
        }
    }
    let dev = identity(parent, name)?.dev;
    let mut walk = Walk {
        tree: Tree::new(),
        metadata: 0,
        dev,
        mount: mount_id(parent, OsStr::new(""))?,
        cancel,
        progress,
    };
    walk.visit(parent, name, Path::new(""), 0)?;
    Ok(walk.tree)
}
fn stage(
    parent: &DirHandle,
    folder: &Path,
    retained: &mut Vec<PathBuf>,
) -> io::Result<(DirHandle, PathBuf)> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    for _ in 0..64 {
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let name = format!(
            ".gust-delete-{}-{time}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        match rustix::fs::mkdirat(
            parent.as_fd(),
            name.as_str(),
            Mode::from_bits_truncate(0o700),
        ) {
            Ok(()) => {
                retained.push(folder.join(&name));
                let dir = parent.open_child_dir(OsStr::new(&name))?;
                let stat = dir.stat_self()?;
                if stat.st_uid != rustix::process::geteuid().as_raw()
                    || stat.st_mode & 0o777 != 0o700
                {
                    return Err(io::Error::other(
                        "Delete staging is not owned private storage",
                    ));
                }
                rustix::fs::fsync(parent.as_fd())?;
                return Ok((dir, folder.join(name)));
            }
            Err(rustix::io::Errno::EXIST) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err(io::Error::other("Cannot reserve delete staging"))
}
fn stage_still_named(
    parent: &DirHandle,
    stage: &DirHandle,
    path: &Path,
) -> Result<(), DeleteFailure> {
    let held = stage.stat_self().map_err(io_failure)?;
    let named = parent
        .stat_child(path.file_name().ok_or(DeleteFailure::Changed)?)
        .map_err(io_failure)?;
    if (held.st_dev, held.st_ino) != (named.st_dev, named.st_ino)
        || held.st_mode & 0o777 != 0o700
        || held.st_uid != rustix::process::geteuid().as_raw()
    {
        return Err(DeleteFailure::Changed);
    }
    Ok(())
}
struct Deletion<'a, F> {
    tree: &'a Tree,
    children: BTreeMap<&'a Path, Vec<&'a Path>>,
    mount: u64,
    cancel: &'a CancelToken,
    removed: &'a mut Removed,
    progress: &'a mut F,
}
impl<F: FnMut(DeleteProgress)> Deletion<'_, F> {
    fn remove(
        &mut self,
        parent: &DirHandle,
        name: &OsStr,
        relative: &Path,
    ) -> Result<(), DeleteFailure> {
        check(self.cancel)?;
        let expected = *self.tree.get(relative).ok_or(DeleteFailure::Changed)?;
        if identity(parent, name)? != expected || mount_id(parent, name)? != self.mount {
            return Err(DeleteFailure::Changed);
        }
        let is_dir = expected.mode & 0o170000 == 0o040000;
        if is_dir {
            let dir = parent.open_child_dir(name).map_err(io_failure)?;
            if FileIdentity::from_stat(&dir.stat_self().map_err(io_failure)?) != expected
                || mount_id(&dir, OsStr::new(""))? != self.mount
            {
                return Err(DeleteFailure::Changed);
            }
            // Only admitted children are visited. New work makes the final
            // rmdir refuse; adjacency avoids rescanning the tree per folder.
            for child in self.children.get(relative).cloned().unwrap_or_default() {
                self.remove(
                    &dir,
                    child.file_name().ok_or(DeleteFailure::Changed)?,
                    child,
                )?;
            }
            let now = FileIdentity::from_stat(&dir.stat_self().map_err(io_failure)?);
            if (now.dev, now.ino, now.mode) != (expected.dev, expected.ino, expected.mode)
                || identity(parent, name)? != now
            {
                return Err(DeleteFailure::Changed);
            }
            rustix::fs::fsync(dir.as_fd()).map_err(|e| io_failure(e.into()))?;
        }
        check(self.cancel)?;
        if mount_id(parent, name)? != self.mount {
            return Err(DeleteFailure::Changed);
        }
        rustix::fs::unlinkat(
            parent.as_fd(),
            name,
            if is_dir {
                AtFlags::REMOVEDIR
            } else {
                AtFlags::empty()
            },
        )
        .map_err(|e| io_failure(e.into()))?;
        if is_dir {
            self.removed.dirs += 1
        } else {
            self.removed.files += 1
        }
        (self.progress)(DeleteProgress {
            phase: DeletePhase::Deleting,
            total: self.tree.len(),
            removed: *self.removed,
        });
        rustix::fs::fsync(parent.as_fd()).map_err(|e| io_failure(e.into()))?;
        Ok(())
    }
}
/// Requires explicit irreversible confirmation and a durable caller barrier.
/// Cancellation/error retains any partial private tree; it does not restore
/// bytes already removed or promise a deadline for blocking filesystem I/O.
pub fn delete_permanently_with_cancel(
    folder: &Path,
    target: &Path,
    cancel: &CancelToken,
) -> Result<Removed, DeleteError> {
    delete_permanently_with_progress(folder, target, None, cancel, |_| {})
}
/// APP supplies the identity retained by its confirmation packet. Progress is
/// worker-owned and contains only memory values; callbacks may request cancel.
pub fn delete_permanently_with_progress(
    folder: &Path,
    target: &Path,
    expected: Option<RecordedIdentity>,
    cancel: &CancelToken,
    mut progress: impl FnMut(DeleteProgress),
) -> Result<Removed, DeleteError> {
    let mut removed = Removed::default();
    let mut retained = Vec::new();
    let result = (|| -> Result<Removed, DeleteFailure> {
        check(cancel)?;
        super::check(folder, target).map_err(DeleteFailure::Refused)?;
        let (parent, name) = anchor::resolve_parent(target).map_err(io_failure)?;
        let tree = snapshot(&parent, &name, cancel, &mut progress)?;
        let root = *tree.get(Path::new("")).ok_or(DeleteFailure::Changed)?;
        if expected.is_some_and(|expected| !expected.matches(&root)) {
            return Err(DeleteFailure::Changed);
        }
        check(cancel)?;
        retained.push(target.to_path_buf());
        let (stage, path) = stage(&parent, folder, &mut retained).map_err(io_failure)?;
        let intent = serde_json::to_vec(&serde_json::json!({"version":1,"original":target.as_os_str().as_bytes(),"root":RecordedIdentity::from(root),"entries":tree.len(),"irreversible":true})).map_err(|e| io_failure(e.into()))?;
        let mut file = File::from(
            rustix::fs::openat(
                stage.as_fd(),
                "intent.json",
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )
            .map_err(|e| io_failure(e.into()))?,
        );
        file.write_all(&intent).map_err(io_failure)?;
        file.sync_all().map_err(io_failure)?;
        rustix::fs::fsync(stage.as_fd()).map_err(|e| io_failure(e.into()))?;
        if snapshot(&parent, &name, cancel, &mut |_| {})? != tree {
            return Err(DeleteFailure::Changed);
        }
        #[cfg(test)]
        before_claim();
        check(cancel)?;
        stage_still_named(&parent, &stage, &path)?;
        let item = OsStr::new("item");
        anchor::rename_noreplace(&parent, &name, &stage, item).map_err(io_failure)?;
        #[cfg(test)]
        after_claim();
        let moved = identity(&stage, item)?;
        let validation = snapshot(&stage, item, cancel, &mut |_| {}).and_then(|now| {
            if now == tree {
                Ok(())
            } else {
                Err(DeleteFailure::Changed)
            }
        });
        if let Err(error) = validation {
            if identity(&stage, item).is_ok_and(|now| now == moved)
                && anchor::rename_noreplace(&stage, item, &parent, &name).is_ok()
            {
                if identity(&parent, &name)? != moved {
                    return Err(DeleteFailure::Changed);
                }
                rustix::fs::fsync(parent.as_fd()).map_err(|e| io_failure(e.into()))?;
                rustix::fs::fsync(stage.as_fd()).map_err(|e| io_failure(e.into()))?;
            }
            return Err(error);
        }
        rustix::fs::fsync(parent.as_fd()).map_err(|e| io_failure(e.into()))?;
        rustix::fs::fsync(stage.as_fd()).map_err(|e| io_failure(e.into()))?;
        progress(DeleteProgress {
            phase: DeletePhase::Deleting,
            total: tree.len(),
            removed,
        });
        stage_still_named(&parent, &stage, &path)?;
        let mut children: BTreeMap<&Path, Vec<&Path>> = BTreeMap::new();
        for path in tree.keys().filter(|path| !path.as_os_str().is_empty()) {
            children
                .entry(path.parent().ok_or(DeleteFailure::Changed)?)
                .or_default()
                .push(path.as_path());
        }
        Deletion {
            tree: &tree,
            children,
            mount: mount_id(&stage, OsStr::new(""))?,
            cancel,
            removed: &mut removed,
            progress: &mut progress,
        }
        .remove(&stage, item, Path::new(""))?;
        progress(DeleteProgress {
            phase: DeletePhase::Finished,
            total: tree.len(),
            removed,
        });
        Ok(removed)
    })();
    result.map_err(|reason| DeleteError {
        reason,
        removed,
        retained,
    })
}
#[cfg(test)]
thread_local! {
    static BEFORE_CLAIM: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static AFTER_CLAIM: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static NO_MOUNT_ID: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static CROSS_MOUNT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
#[cfg(test)]
fn before_claim() {
    BEFORE_CLAIM.with(|hook| {
        let run = hook.borrow_mut().take();
        if let Some(run) = run {
            run()
        }
    });
}
#[cfg(test)]
fn after_claim() {
    AFTER_CLAIM.with(|hook| {
        let run = hook.borrow_mut().take();
        if let Some(run) = run {
            run()
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "gust-permanent-race-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn changed_claim_restores_only_the_observed_object_without_clobber() {
        for blocked in [false, true] {
            let f = Fixture::new();
            let path = f.0.join("file");
            let kept = f.0.join("kept");
            fs::write(&path, b"original").unwrap();
            let expected = FileIdentity::of_path(&path).unwrap().into();
            BEFORE_CLAIM.with(|hook| {
                let path = path.clone();
                let kept = kept.clone();
                *hook.borrow_mut() = Some(Box::new(move || {
                    fs::rename(&path, kept).unwrap();
                    fs::write(path, b"replacement").unwrap();
                }));
            });
            if blocked {
                AFTER_CLAIM.with(|hook| {
                    let path = path.clone();
                    *hook.borrow_mut() =
                        Some(Box::new(move || fs::write(path, b"later work").unwrap()));
                });
            }
            let error = delete_permanently_with_progress(
                &f.0,
                &path,
                Some(expected),
                &CancelToken::new(),
                |_| {},
            )
            .unwrap_err();
            assert_eq!(error.reason, DeleteFailure::Changed);
            assert_eq!(error.removed, Removed::default());
            assert_eq!(fs::read(kept).unwrap(), b"original");
            assert_eq!(
                fs::read(&path).unwrap(),
                if blocked {
                    b"later work".as_slice()
                } else {
                    b"replacement".as_slice()
                }
            );
            let stage = error
                .retained
                .iter()
                .find(|p| {
                    p.file_name()
                        .is_some_and(|n| n.as_bytes().starts_with(b".gust-delete-"))
                })
                .unwrap();
            assert!(stage.join("intent.json").exists());
            if blocked {
                assert_eq!(fs::read(stage.join("item")).unwrap(), b"replacement");
            } else {
                assert!(!stage.join("item").exists());
            }
        }
    }
    #[test]
    fn absent_mount_identity_refuses_before_staging_or_unlinking() {
        let f = Fixture::new();
        let path = f.0.join("file");
        fs::write(&path, b"keep").unwrap();
        NO_MOUNT_ID.with(|flag| flag.set(true));
        let result = delete_permanently_with_cancel(&f.0, &path, &CancelToken::new());
        NO_MOUNT_ID.with(|flag| flag.set(false));
        let error = result.unwrap_err();
        assert_eq!(error.reason, DeleteFailure::Unsupported);
        assert!(error.retained.is_empty());
        assert_eq!(fs::read(path).unwrap(), b"keep");
    }
    #[test]
    fn same_device_child_mount_identity_refuses_before_traversal() {
        let f = Fixture::new();
        let root = f.0.join("tree");
        fs::create_dir_all(root.join("mounted")).unwrap();
        fs::write(root.join("mounted/file"), b"keep").unwrap();
        CROSS_MOUNT.with(|flag| flag.set(true));
        let result = delete_permanently_with_cancel(&f.0, &root, &CancelToken::new());
        CROSS_MOUNT.with(|flag| flag.set(false));
        let error = result.unwrap_err();
        assert_eq!(error.reason, DeleteFailure::Unsupported);
        assert!(error.retained.is_empty());
        assert_eq!(fs::read(root.join("mounted/file")).unwrap(), b"keep");
    }
    #[test]
    fn unsupported_no_replace_preserves_the_source_and_durable_intent() {
        let f = Fixture::new();
        let path = f.0.join("file");
        fs::write(&path, b"keep").unwrap();
        crate::anchor::UNSUPPORTED_ERRNO.with(|fail| fail.set(Some(rustix::io::Errno::OPNOTSUPP)));
        let error = delete_permanently_with_cancel(&f.0, &path, &CancelToken::new()).unwrap_err();
        assert_eq!(error.reason, DeleteFailure::Unsupported);
        assert_eq!(error.removed, Removed::default());
        assert_eq!(fs::read(path).unwrap(), b"keep");
        assert!(error
            .retained
            .iter()
            .any(|p| p.join("intent.json").exists()));
    }
    #[test]
    fn changed_staging_name_refuses_before_claiming_public_data() {
        let f = Fixture::new();
        let path = f.0.join("file");
        fs::write(&path, b"keep").unwrap();
        BEFORE_CLAIM.with(|hook| {
            let root = f.0.clone();
            *hook.borrow_mut() = Some(Box::new(move || {
                let stage = fs::read_dir(&root)
                    .unwrap()
                    .filter_map(Result::ok)
                    .find(|entry| entry.file_name().as_bytes().starts_with(b".gust-delete-"))
                    .unwrap()
                    .path();
                fs::rename(&stage, root.join("kept-intent")).unwrap();
                fs::create_dir(&stage).unwrap();
                fs::write(stage.join("later"), b"new work").unwrap();
            }));
        });
        let error = delete_permanently_with_cancel(&f.0, &path, &CancelToken::new()).unwrap_err();
        assert_eq!(error.reason, DeleteFailure::Changed);
        assert_eq!(error.removed, Removed::default());
        assert_eq!(fs::read(path).unwrap(), b"keep");
        assert!(f.0.join("kept-intent/intent.json").exists());
        let replacement = error
            .retained
            .iter()
            .find(|p| {
                p.file_name()
                    .is_some_and(|n| n.as_bytes().starts_with(b".gust-delete-"))
            })
            .unwrap();
        assert_eq!(fs::read(replacement.join("later")).unwrap(), b"new work");
    }
}
