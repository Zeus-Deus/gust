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
    /// Last-known location hints; concurrent relocation can make these stale.
    pub retained: Vec<PathBuf>,
    pub locations_uncertain: bool,
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
            write!(
                f,
                "; last-known location hints (may have moved): {:?}",
                self.retained
            )?;
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
                .saturating_add(relative.as_os_str().as_bytes().len().saturating_mul(6) + 1024);
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
fn directory_key(id: FileIdentity) -> (u64, u64, u32) {
    (id.dev, id.ino, id.mode)
}
#[derive(Clone, Copy)]
struct Binding<'a> {
    parent: &'a DirHandle,
    name: &'a OsStr,
    held: &'a DirHandle,
    admitted: FileIdentity,
}
struct Namespace<'a> {
    parent: &'a DirHandle,
    stage: &'a DirHandle,
    stage_path: &'a Path,
    original: &'a Path,
    parent_identity: FileIdentity,
    mount: u64,
}
impl Namespace<'_> {
    fn verify(&self, chain: &[Binding<'_>]) -> Result<(), DeleteFailure> {
        // Re-resolve only to verify that the retained parent is still named;
        // all mutation remains descriptor-relative to the admitted handles.
        let (current, _) = anchor::resolve_parent(self.original).map_err(io_failure)?;
        let named_parent = FileIdentity::from_stat(&current.stat_self().map_err(io_failure)?);
        let held_parent = FileIdentity::from_stat(&self.parent.stat_self().map_err(io_failure)?);
        if directory_key(named_parent) != directory_key(self.parent_identity)
            || directory_key(held_parent) != directory_key(self.parent_identity)
            || mount_id(&current, OsStr::new(""))? != self.mount
        {
            return Err(DeleteFailure::Changed);
        }
        stage_still_named(self.parent, self.stage, self.stage_path)?;
        if mount_id(self.stage, OsStr::new(""))? != self.mount
            || mount_id(
                self.parent,
                self.stage_path.file_name().ok_or(DeleteFailure::Changed)?,
            )? != self.mount
        {
            return Err(DeleteFailure::Changed);
        }
        for binding in chain {
            let held = FileIdentity::from_stat(&binding.held.stat_self().map_err(io_failure)?);
            let named = identity(binding.parent, binding.name)?;
            if directory_key(held) != directory_key(binding.admitted)
                || directory_key(named) != directory_key(held)
                || mount_id(binding.parent, binding.name)? != self.mount
                || mount_id(binding.held, OsStr::new(""))? != self.mount
            {
                return Err(DeleteFailure::Changed);
            }
        }
        Ok(())
    }
}
fn sync_directory(dir: &DirHandle) -> Result<(), DeleteFailure> {
    rustix::fs::fsync(dir.as_fd()).map_err(|e| io_failure(e.into()))
}
fn write_intent(dir: &DirHandle, bytes: &[u8]) -> Result<(), DeleteFailure> {
    let mut file = File::from(
        rustix::fs::openat(
            dir.as_fd(),
            "intent.json",
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_bits_truncate(0o600),
        )
        .map_err(|e| io_failure(e.into()))?,
    );
    file.write_all(bytes).map_err(io_failure)?;
    file.sync_all().map_err(io_failure)?;
    sync_directory(dir)
}
struct Deletion<'a, F> {
    tree: &'a Tree,
    children: BTreeMap<&'a Path, Vec<&'a Path>>,
    namespace: Namespace<'a>,
    cancel: &'a CancelToken,
    removed: &'a mut Removed,
    progress: &'a mut F,
    next_cell: usize,
}
impl<F: FnMut(DeleteProgress)> Deletion<'_, F> {
    fn verify(&self, chain: &[Binding<'_>]) -> Result<(), DeleteFailure> {
        check(self.cancel)?;
        self.namespace.verify(chain)
    }
    fn remove(
        &mut self,
        parent: &DirHandle,
        name: &OsStr,
        relative: &Path,
        chain: &[Binding<'_>],
    ) -> Result<(), DeleteFailure> {
        self.verify(chain)?;
        let expected = *self.tree.get(relative).ok_or(DeleteFailure::Changed)?;
        if identity(parent, name)? != expected || mount_id(parent, name)? != self.namespace.mount {
            return Err(DeleteFailure::Changed);
        }
        let is_dir = expected.mode & 0o170000 == 0o040000;
        let mut claimed_identity = expected;
        if is_dir {
            let dir = parent.open_child_dir(name).map_err(io_failure)?;
            if FileIdentity::from_stat(&dir.stat_self().map_err(io_failure)?) != expected
                || mount_id(&dir, OsStr::new(""))? != self.namespace.mount
            {
                return Err(DeleteFailure::Changed);
            }
            let mut descendants = chain.to_vec();
            descendants.push(Binding {
                parent,
                name,
                held: &dir,
                admitted: expected,
            });
            // Known children only. New work survives: the final private rmdir
            // refuses a nonempty directory rather than walking extra entries.
            for child in self.children.get(relative).cloned().unwrap_or_default() {
                self.remove(
                    &dir,
                    child.file_name().ok_or(DeleteFailure::Changed)?,
                    child,
                    &descendants,
                )?;
            }
            self.verify(&descendants)?;
            let now = FileIdentity::from_stat(&dir.stat_self().map_err(io_failure)?);
            if directory_key(now) != directory_key(expected) || identity(parent, name)? != now {
                return Err(DeleteFailure::Changed);
            }
            sync_directory(&dir)?;
            claimed_identity = now;
        }
        self.verify(chain)?;
        let cell_name = format!("entry-{:06}", self.next_cell);
        self.next_cell += 1;
        rustix::fs::mkdirat(
            self.namespace.stage.as_fd(),
            cell_name.as_str(),
            Mode::from_bits_truncate(0o700),
        )
        .map_err(|e| io_failure(e.into()))?;
        let cell = self
            .namespace
            .stage
            .open_child_dir(OsStr::new(&cell_name))
            .map_err(io_failure)?;
        let cell_birth = FileIdentity::from_stat(&cell.stat_self().map_err(io_failure)?);
        let owner = cell.stat_self().map_err(io_failure)?;
        if owner.st_uid != rustix::process::geteuid().as_raw()
            || cell_birth.mode & 0o777 != 0o700
            || mount_id(&cell, OsStr::new(""))? != self.namespace.mount
        {
            return Err(DeleteFailure::Changed);
        }
        sync_directory(self.namespace.stage)?;
        let intent = serde_json::to_vec(&serde_json::json!({
            "version":2, "relative":relative.as_os_str().as_bytes(),
            "admitted":RecordedIdentity::from(expected),
            "claimed":RecordedIdentity::from(claimed_identity), "mode":claimed_identity.mode,
            "irreversible":true,
        }))
        .map_err(|e| io_failure(e.into()))?;
        write_intent(&cell, &intent)?;
        self.verify(chain)?;
        stage_still_named(self.namespace.stage, &cell, Path::new(&cell_name))?;
        if identity(parent, name)? != claimed_identity
            || mount_id(parent, name)? != self.namespace.mount
        {
            return Err(DeleteFailure::Changed);
        }
        // This is the former final-check/unlink cut. A retained source-parent
        // FD can replace its old leaf here; that name is not our cell entry.
        #[cfg(test)]
        before_unlink();
        let item = OsStr::new("item");
        anchor::rename_noreplace(parent, name, &cell, item).map_err(io_failure)?;
        #[cfg(test)]
        after_entry_claim();
        sync_directory(parent)?;
        sync_directory(&cell)?;
        let moved = identity(&cell, item)?;
        if moved != claimed_identity || mount_id(&cell, item)? != self.namespace.mount {
            // A wrong claim is never unlinked. Return only the same observed
            // object into an unoccupied old name; otherwise retain the cell.
            if self.namespace.verify(chain).is_ok()
                && identity(&cell, item).is_ok_and(|now| now == moved)
                && anchor::rename_noreplace(&cell, item, parent, name).is_ok()
            {
                sync_directory(parent)?;
                sync_directory(&cell)?;
            }
            return Err(DeleteFailure::Changed);
        }
        self.verify(chain)?;
        stage_still_named(self.namespace.stage, &cell, Path::new(&cell_name))?;
        if directory_key(FileIdentity::from_stat(
            &cell.stat_self().map_err(io_failure)?,
        )) != directory_key(cell_birth)
            || identity(&cell, item)? != claimed_identity
            || mount_id(&cell, item)? != self.namespace.mount
        {
            return Err(DeleteFailure::Changed);
        }
        rustix::fs::unlinkat(
            cell.as_fd(),
            item,
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
        sync_directory(&cell)?;
        self.verify(chain)?;
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
        let parent_identity = FileIdentity::from_stat(&parent.stat_self().map_err(io_failure)?);
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
        let namespace = Namespace {
            parent: &parent,
            stage: &stage,
            stage_path: &path,
            original: target,
            parent_identity,
            mount: mount_id(&stage, OsStr::new(""))?,
        };
        let mut deletion = Deletion {
            tree: &tree,
            children,
            namespace,
            cancel,
            removed: &mut removed,
            progress: &mut progress,
            next_cell: 0,
        };
        deletion.remove(&stage, item, Path::new(""), &[])?;
        deletion.namespace.verify(&[])?;
        match stage.stat_child(item) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            _ => return Err(DeleteFailure::Changed),
        }
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
        locations_uncertain: !retained.is_empty(),
        retained,
    })
}
#[cfg(test)]
thread_local! {
    static BEFORE_CLAIM: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static AFTER_CLAIM: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static BEFORE_UNLINK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static AFTER_ENTRY_CLAIM: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
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
fn before_unlink() {
    BEFORE_UNLINK.with(|hook| {
        let run = hook.borrow_mut().take();
        if let Some(run) = run {
            run()
        }
    });
}

#[cfg(test)]
fn after_entry_claim() {
    AFTER_ENTRY_CLAIM.with(|hook| {
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
    #[test]
    fn final_entry_file_replacement_through_old_parent_fd_is_never_unlinked() {
        final_entry_case(false);
    }
    #[test]
    fn final_entry_directory_replacement_through_old_parent_fd_is_never_unlinked() {
        final_entry_case(true);
    }
    fn final_entry_case(directory: bool) {
        use rustix::fd::AsFd;
        {
            let f = Fixture::new();
            let root = f.0.join("tree");
            fs::create_dir(&root).unwrap();
            if directory {
                fs::create_dir(root.join("leaf")).unwrap();
            } else {
                fs::write(root.join("leaf"), b"admitted").unwrap();
            }
            let held = File::open(&root).unwrap();
            let outside = File::open(&f.0).unwrap();
            let observe = held.try_clone().unwrap();
            BEFORE_UNLINK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    rustix::fs::renameat(held.as_fd(), "leaf", outside.as_fd(), "original")
                        .unwrap();
                    if directory {
                        rustix::fs::mkdirat(held.as_fd(), "leaf", Mode::from_bits_truncate(0o700))
                            .unwrap();
                    } else {
                        let mut replacement = File::from(
                            rustix::fs::openat(
                                held.as_fd(),
                                "leaf",
                                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL,
                                Mode::from_bits_truncate(0o600),
                            )
                            .unwrap(),
                        );
                        replacement.write_all(b"replacement").unwrap();
                    }
                }));
            });
            let result = delete_permanently_with_cancel(&f.0, &root, &CancelToken::new());
            let replacement =
                rustix::fs::statat(observe.as_fd(), "leaf", AtFlags::SYMLINK_NOFOLLOW);
            assert!(
                replacement.is_ok(),
                "final replacement must survive: {result:?}"
            );
            if !directory {
                assert_eq!(fs::read(f.0.join("original")).unwrap(), b"admitted");
            } else {
                assert!(f.0.join("original").is_dir());
            }
            let error = result.expect_err("wrong final claim must refuse");
            assert_eq!(error.reason, DeleteFailure::Changed);
            assert_eq!(error.removed, Removed::default());
        }
    }
    #[test]
    fn cancellation_after_final_claim_retains_item_and_synced_intent_without_unlink() {
        let f = Fixture::new();
        let path = f.0.join("file");
        fs::write(&path, b"keep claimed bytes").unwrap();
        let cancel = CancelToken::new();
        AFTER_ENTRY_CLAIM.with(|hook| {
            let cancel = cancel.clone();
            *hook.borrow_mut() = Some(Box::new(move || cancel.cancel()));
        });
        let error = delete_permanently_with_cancel(&f.0, &path, &cancel).unwrap_err();
        assert_eq!(error.reason, DeleteFailure::Cancelled);
        assert_eq!(error.removed, Removed::default());
        assert!(error.locations_uncertain);
        let stage = error.retained.last().unwrap();
        assert_eq!(
            fs::read(stage.join("entry-000000/item")).unwrap(),
            b"keep claimed bytes"
        );
        let intent: serde_json::Value =
            serde_json::from_slice(&fs::read(stage.join("entry-000000/intent.json")).unwrap())
                .unwrap();
        assert_eq!(intent["relative"], serde_json::json!([]));
        assert_eq!(intent["irreversible"], true);
    }
    #[test]
    fn blocked_wrong_final_claim_keeps_replacement_and_later_name_without_clobber() {
        use rustix::fd::AsFd;
        let f = Fixture::new();
        let root = f.0.join("tree");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("leaf"), b"admitted").unwrap();
        let held = File::open(&root).unwrap();
        let later = held.try_clone().unwrap();
        let outside = File::open(&f.0).unwrap();
        BEFORE_UNLINK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                rustix::fs::renameat(held.as_fd(), "leaf", outside.as_fd(), "original").unwrap();
                let mut file = File::from(
                    rustix::fs::openat(
                        held.as_fd(),
                        "leaf",
                        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL,
                        Mode::from_bits_truncate(0o600),
                    )
                    .unwrap(),
                );
                file.write_all(b"replacement").unwrap();
            }));
        });
        AFTER_ENTRY_CLAIM.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let mut file = File::from(
                    rustix::fs::openat(
                        later.as_fd(),
                        "leaf",
                        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL,
                        Mode::from_bits_truncate(0o600),
                    )
                    .unwrap(),
                );
                file.write_all(b"later work").unwrap();
            }));
        });
        let error = delete_permanently_with_cancel(&f.0, &root, &CancelToken::new()).unwrap_err();
        assert_eq!(error.reason, DeleteFailure::Changed);
        assert_eq!(error.removed, Removed::default());
        let stage = error.retained.last().unwrap();
        assert_eq!(fs::read(f.0.join("original")).unwrap(), b"admitted");
        assert_eq!(fs::read(stage.join("item/leaf")).unwrap(), b"later work");
        assert_eq!(
            fs::read(stage.join("entry-000000/item")).unwrap(),
            b"replacement"
        );
        assert!(stage.join("entry-000000/intent.json").exists());
    }
}
