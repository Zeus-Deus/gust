//! Descriptor-bound per-volume Trash and identity receipts.
#[path = "trash_empty.rs"]
mod empty;
#[path = "trash_lineage.rs"]
pub(crate) mod lineage;
use super::{TrashDir, TrashEntry, TrashedItem};
use crate::{
    anchor::{self, DirHandle, FileIdentity},
    history_tree,
};
pub use empty::{
    prepare_empty, ConfirmedEmptyTrash, EmptyTrashError, EmptyTrashFailure, EmptyTrashOutcome,
    EmptyTrashPhase, EmptyTrashPlan, EmptyTrashProgress, EmptyTrashSummary,
};
use gust_core::{
    lister::CancelToken,
    mount_table::{MountInfo, MountTableError, MAX_MOUNTINFO_BYTES},
};
use rustix::{
    fd::{AsFd, BorrowedFd},
    fs::{Mode, OFlags},
};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, Read, Write},
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Component, Path, PathBuf},
};
#[derive(Debug, Clone, PartialEq)]
pub struct TrashLocation {
    pub trash: TrashDir,
    pub topdir: Option<PathBuf>,
}
/// Display projections are not mutation authority; private bindings are checked.
#[derive(Debug, Clone)]
pub struct TrashReceipt {
    pub item: TrashedItem,
    pub location: TrashLocation,
    canonical: TrashLocation,
    original: PathBuf,
    name: OsString,
    root_dir: FileIdentity,
    top_dir: Option<FileIdentity>,
    original_parent: Option<FileIdentity>,
    files_dir: FileIdentity,
    info_dir: FileIdentity,
    pub(crate) data: FileIdentity,
    pub(crate) info: FileIdentity,
    pub(crate) tree: BTreeMap<PathBuf, FileIdentity>,
    pub(crate) info_bytes: Vec<u8>,
    inventory_mount: Option<u64>,
}
#[derive(Debug, Clone)]
pub struct RestoredReceipt {
    pub original: PathBuf,
    pub(crate) trash: TrashReceipt,
    retained_info: PathBuf,
    retained_dir: FileIdentity,
    restored_parent: FileIdentity,
    created_parents: bool,
}
impl RestoredReceipt {
    /// Missing-parent restoration is supported, but those new parents have no inverse.
    pub fn history_supported(&self) -> bool {
        !self.created_parents
    }
}
/// An admitted operation may have metadata, intent or data at these locations.
/// Retain all of them for inspection; an error is not proof of no effects.
#[derive(Debug)]
pub struct TrashPartial {
    pub locations: Vec<PathBuf>,
    pub cause: String,
}
impl std::fmt::Display for TrashPartial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Trash recovery needs inspection at {:?}: {}",
            self.locations, self.cause
        )
    }
}
impl std::error::Error for TrashPartial {}
fn partial(error: io::Error, locations: Vec<PathBuf>) -> io::Error {
    let mut locations = locations;
    if let Some(prior) = error
        .get_ref()
        .and_then(|e| e.downcast_ref::<TrashPartial>())
    {
        locations.extend(prior.locations.iter().cloned());
    }
    locations.sort();
    locations.dedup();
    io::Error::new(
        error.kind(),
        TrashPartial {
            locations,
            cause: error.to_string(),
        },
    )
}
fn same_dir(dir: &DirHandle, expected: FileIdentity) -> io::Result<()> {
    let stat = dir.stat_self()?;
    if (stat.st_dev, stat.st_ino) != (expected.dev, expected.ino) {
        return Err(invalid("Trash directory was replaced"));
    }
    private(dir)
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn check(cancel: &CancelToken) -> io::Result<()> {
    if cancel.is_cancelled() {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Trash operation cancelled",
        ))
    } else {
        Ok(())
    }
}
fn private(dir: &DirHandle) -> io::Result<()> {
    let s = dir.stat_self()?;
    if s.st_uid != rustix::process::geteuid().as_raw() || s.st_mode & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Trash must be an owned private directory",
        ));
    }
    Ok(())
}
/// Explicit worker repair is needed; listing never changes permissions.
#[derive(Debug)]
pub struct LegacyTrashNeedsRepair {
    pub root: PathBuf,
}
impl std::fmt::Display for LegacyTrashNeedsRepair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Existing Trash at {:?} needs an explicit private-permission repair before it can be used; its data was preserved.", self.root)
    }
}
impl std::error::Error for LegacyTrashNeedsRepair {}
fn safe_legacy(dir: &DirHandle) -> io::Result<bool> {
    if private(dir).is_ok() {
        return Ok(false);
    }
    let s = dir.stat_self()?;
    if s.st_uid != rustix::process::geteuid().as_raw()
        || s.st_mode & 0o700 != 0o700
        || s.st_mode & 0o022 != 0
        || s.st_mode & 0o7000 != 0
    {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "Trash layout is not eligible for automatic private-permission repair; inspect ownership and directory permissions"));
    }
    Ok(true)
}
fn existing_private(path: &Path) -> io::Result<Option<DirHandle>> {
    match history_tree::open_dir(path) {
        Ok(d) => {
            if safe_legacy(&d)? {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    LegacyTrashNeedsRepair {
                        root: path.to_path_buf(),
                    },
                ));
            }
            Ok(Some(d))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
fn info_name(name: &OsStr) -> io::Result<OsString> {
    crate::recursive::validate_name(name).map_err(|e| invalid(&e.to_string()))?;
    let mut n = name.to_os_string();
    n.push(".trashinfo");
    Ok(n)
}
fn remap(
    tree: &BTreeMap<PathBuf, FileIdentity>,
    from: &Path,
    to: &Path,
) -> io::Result<BTreeMap<PathBuf, FileIdentity>> {
    tree.iter()
        .map(|(p, id)| {
            Ok((
                to.join(
                    p.strip_prefix(from)
                        .map_err(|_| invalid("Invalid Trash inventory"))?,
                ),
                *id,
            ))
        })
        .collect()
}
#[cfg(test)]
thread_local! {
    static INFO_READ_PROBE: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}
fn open_info(dir: &DirHandle, name: &OsStr) -> io::Result<(File, FileIdentity, Vec<u8>)> {
    open_info_with_mount(dir, name, None)
}
fn open_info_with_mount(
    dir: &DirHandle,
    name: &OsStr,
    mount: Option<u64>,
) -> io::Result<(File, FileIdentity, Vec<u8>)> {
    check_mount_at(dir, name, mount)?;
    let mut file = File::from(rustix::fs::openat(
        dir.as_fd(),
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    check_fd_mount(file.as_fd(), OsStr::new(""), mount)?;
    let st = rustix::fs::fstat(&file)?;
    if st.st_mode & 0o170000 != 0o100000
        || st.st_uid != rustix::process::geteuid().as_raw()
        || st.st_mode & 0o022 != 0
    {
        return Err(invalid("Trash information is not an owned regular record"));
    }
    let mut bytes = Vec::new();
    #[cfg(test)]
    INFO_READ_PROBE.with(|probe| {
        if let Some(n) = probe.get() {
            probe.set(Some(n + 1));
        }
    });
    Read::by_ref(&mut file)
        .take(65537)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err(invalid("Trash information exceeds its size bound"));
    }
    let id = FileIdentity::from_stat(&st);
    if !id.matches(&FileIdentity::from_stat(&rustix::fs::fstat(&file)?)) {
        return Err(invalid("Trash record changed while reading"));
    }
    check_fd_mount(file.as_fd(), OsStr::new(""), mount)?;
    check_mount_at(dir, name, mount)?;
    Ok((file, id, bytes))
}
fn decode(value: &[u8]) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < value.len() {
        if value[i] == b'%' {
            let pair = value
                .get(i + 1..i + 3)
                .ok_or_else(|| invalid("Invalid escaped Trash path"))?;
            let text =
                std::str::from_utf8(pair).map_err(|_| invalid("Invalid escaped Trash path"))?;
            out.push(
                u8::from_str_radix(text, 16).map_err(|_| invalid("Invalid escaped Trash path"))?,
            );
            i += 3;
        } else {
            out.push(value[i]);
            i += 1;
        }
    }
    if out.contains(&0) {
        return Err(invalid("NUL in Trash path"));
    }
    Ok(out)
}
fn parse_info(
    bytes: &[u8],
    location: &TrashLocation,
) -> io::Result<(PathBuf, String, Option<crate::journal::RecordedIdentity>)> {
    let mut lines = bytes.split(|b| *b == b'\n');
    if lines.next() != Some(b"[Trash Info]".as_slice()) {
        return Err(invalid("Invalid Trash record header"));
    }
    let (mut path, mut date, mut identity) = (None, None, None);
    for line in lines {
        if path.is_none() {
            if let Some(v) = line.strip_prefix(b"Path=") {
                path = Some(PathBuf::from(OsString::from_vec(decode(v)?)));
            }
        }
        if date.is_none() {
            if let Some(v) = line.strip_prefix(b"DeletionDate=") {
                date = Some(
                    String::from_utf8(v.to_vec()).map_err(|_| invalid("Invalid deletion date"))?,
                );
            }
        }
        if identity.is_none() {
            if let Some(v) = line.strip_prefix(b"X-Gust-Identity=") {
                identity =
                    Some(serde_json::from_slice(v).map_err(|_| invalid("Invalid Trash identity"))?);
            }
        }
    }
    let path = path.ok_or_else(|| invalid("Missing Trash path"))?;
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
    {
        return Err(invalid("Unsafe Trash path"));
    }
    let original = if let Some(top) = &location.topdir {
        if path.is_absolute() {
            return Err(invalid("Volume Trash requires a relative restore path"));
        }
        top.join(path)
    } else if path.is_absolute() {
        path
    } else {
        return Err(invalid("Home Trash requires an absolute restore path"));
    };
    Ok((
        original,
        date.ok_or_else(|| invalid("Missing deletion date"))?,
        identity,
    ))
}
pub(crate) fn authenticated_info(
    parent: &DirHandle,
    name: &OsStr,
) -> io::Result<(FileIdentity, Vec<u8>)> {
    let (_, identity, bytes) = open_info(parent, name)?;
    Ok((identity, bytes))
}
pub(crate) fn verify_info_original(bytes: &[u8], base: &Path, original: &Path) -> io::Result<()> {
    // Home paths are absolute, volume paths are relative to a bound topdir.
    // A legacy record's exact original path establishes that relative root.
    let raw = bytes
        .split(|b| *b == b'\n')
        .find_map(|line| line.strip_prefix(b"Path="))
        .ok_or_else(|| invalid("Missing Trash path"))?;
    let path = PathBuf::from(OsString::from_vec(decode(raw)?));
    let topdir = if path.is_absolute() {
        None
    } else {
        let mut root = original.to_path_buf();
        for _ in path.components() {
            root.pop();
        }
        Some(root)
    };
    let location = TrashLocation {
        trash: TrashDir::at(base),
        topdir,
    };
    if parse_info(bytes, &location)?.0 != original {
        return Err(invalid("Legacy Trash original changed"));
    }
    Ok(())
}
impl TrashLocation {
    fn handles(&self, create: bool) -> io::Result<(DirHandle, DirHandle)> {
        self.handles_mode(create, false)
    }
    fn handles_mode(&self, create: bool, allow_legacy: bool) -> io::Result<(DirHandle, DirHandle)> {
        let mounts = mounts()?;
        local_location(self, &mounts)?;
        if self.trash.files != self.trash.root().join("files")
            || self.trash.info != self.trash.root().join("info")
        {
            return Err(invalid("Invalid Trash directory layout"));
        }
        if let Some(top) = &self.topdir {
            let uid = rustix::process::geteuid().as_raw();
            let shared = top.join(".Trash");
            if self.trash.root() == shared.join(uid.to_string()) {
                let dir = history_tree::open_dir(&shared)?;
                if dir.stat_self()?.st_mode & 0o1000 == 0 {
                    return Err(invalid("Shared Trash has no sticky bit"));
                }
            } else if self.trash.root() != top.join(format!(".Trash-{uid}")) {
                return Err(invalid("Invalid volume Trash root"));
            }
        }
        let root = if create {
            history_tree::ensure_dir(self.trash.root())?
        } else {
            history_tree::open_dir(self.trash.root())?
        };
        self.directory_mode(&root, allow_legacy)?;
        for name in ["files", "info"] {
            if create {
                match rustix::fs::mkdirat(root.as_fd(), name, Mode::from_bits_truncate(0o700)) {
                    Ok(()) => rustix::fs::fsync(root.as_fd())?,
                    Err(rustix::io::Errno::EXIST) => {}
                    Err(e) => return Err(e.into()),
                }
            }
            self.directory_mode(&root.open_child_dir(OsStr::new(name))?, allow_legacy)?;
        }
        let files = root.open_child_dir(OsStr::new("files"))?;
        let info = root.open_child_dir(OsStr::new("info"))?;
        let dev = root.stat_self()?.st_dev;
        if files.stat_self()?.st_dev != dev || info.stat_self()?.st_dev != dev {
            return Err(invalid("Trash directories must share a filesystem"));
        }
        if let Some(top) = &self.topdir {
            if history_tree::open_dir(top)?.stat_self()?.st_dev != dev {
                return Err(invalid("Trash is outside its bound volume"));
            }
        }
        Ok((files, info))
    }
    fn directory_mode(&self, dir: &DirHandle, allow_legacy: bool) -> io::Result<()> {
        if safe_legacy(dir)? && !allow_legacy {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                LegacyTrashNeedsRepair {
                    root: self.trash.root().to_path_buf(),
                },
            ));
        }
        Ok(())
    }
    /// Explicit worker command. Discovery/listing never invokes this repair.
    /// Only held owned non-writable-by-others legacy directories are tightened.
    /// Caller retains a forward marker and durable pre-effect history barrier.
    pub fn repair_legacy_permissions(&self, cancel: &CancelToken) -> io::Result<()> {
        check(cancel)?;
        let (files, info) = self.handles_mode(false, true)?;
        let (parent, name) = anchor::resolve_parent(self.trash.root())?;
        let root = parent.open_child_dir(&name)?;
        safe_legacy(&root)?;
        let dirs = [&root, &files, &info];
        let mut expected = [
            FileIdentity::from_stat(&root.stat_self()?),
            FileIdentity::from_stat(&files.stat_self()?),
            FileIdentity::from_stat(&info.stat_self()?),
        ];
        let parent_id = FileIdentity::from_stat(&parent.stat_self()?);
        let verify = |expected: &[FileIdentity; 3]| -> io::Result<()> {
            let (named_parent, _) = anchor::resolve_parent(self.trash.root())?;
            let named_parent_id = FileIdentity::from_stat(&named_parent.stat_self()?);
            if (parent_id.dev, parent_id.ino) != (named_parent_id.dev, named_parent_id.ino) {
                return Err(invalid("Trash repair parent was replaced"));
            }
            let named = [
                parent.stat_child(&name)?,
                root.stat_child(OsStr::new("files"))?,
                root.stat_child(OsStr::new("info"))?,
            ];
            for i in 0..3 {
                if FileIdentity::from_stat(&named[i]) != expected[i]
                    || FileIdentity::from_stat(&dirs[i].stat_self()?) != expected[i]
                {
                    return Err(invalid("Trash layout changed during permission repair"));
                }
                safe_legacy(dirs[i])?;
            }
            Ok(())
        };
        let mut changed = false;
        let result = (|| {
            verify(&expected)?;
            for i in [1, 2, 0] {
                check(cancel)?;
                verify(&expected)?;
                if expected[i].mode & 0o077 == 0 {
                    continue;
                }
                #[cfg(test)]
                BEFORE_LEGACY_REPAIR.with(|hook| {
                    let run = hook.borrow_mut().take();
                    if let Some(run) = run {
                        run();
                    }
                });
                check(cancel)?;
                rustix::fs::fchmod(dirs[i].as_fd(), Mode::from_bits_truncate(0o700))?;
                changed = true;
                #[cfg(test)]
                AFTER_LEGACY_REPAIR.with(|hook| {
                    let run = hook.borrow_mut().take();
                    if let Some(run) = run {
                        run();
                    }
                });
                expected[i].mode = (expected[i].mode & !0o7777) | 0o700;
                rustix::fs::fsync(dirs[i].as_fd())?;
                verify(&expected)?;
            }
            Ok(())
        })();
        result.map_err(|error| {
            if changed {
                partial(
                    error,
                    vec![
                        self.trash.root().to_path_buf(),
                        self.trash.files.clone(),
                        self.trash.info.clone(),
                    ],
                )
            } else {
                error
            }
        })
    }
    pub fn trash_path(&self, path: &Path, cancel: &CancelToken) -> io::Result<TrashReceipt> {
        check(cancel)?;
        let mount_table = mounts()?;
        local_path(path, &mount_table)?;
        local_location(self, &mount_table)?;
        same_local_mount(
            self.trash.root(),
            mount_table.resolve(path).map_err(mount_error)?.id,
            &mount_table,
        )?;
        if path.starts_with(self.trash.root()) || self.trash.root().starts_with(path) {
            return Err(invalid(
                "A Trash directory or its ancestor cannot be trashed",
            ));
        }
        let (source, source_name) = anchor::resolve_parent(path)?;
        let tree = history_tree::snapshot_at(&source, &source_name, path, cancel)?;
        let data = *tree
            .get(path)
            .ok_or_else(|| invalid("Missing Trash identity"))?;
        let original = if let Some(top) = &self.topdir {
            let relative = path
                .strip_prefix(top)
                .map_err(|_| invalid("Source is outside selected volume"))?;
            if relative.as_os_str().is_empty() {
                return Err(invalid("A volume root cannot be trashed"));
            }
            relative
        } else {
            path
        };
        if nearest_dev(self.trash.root())? != data.dev {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Trash is unavailable on this source volume",
            ));
        }
        let (files, info) = self.handles(true)?;
        let root_dir =
            FileIdentity::from_stat(&history_tree::open_dir(self.trash.root())?.stat_self()?);
        let top_dir = self
            .topdir
            .as_ref()
            .map(|p| {
                history_tree::open_dir(p)
                    .and_then(|d| d.stat_self())
                    .map(|s| FileIdentity::from_stat(&s))
            })
            .transpose()?;
        if data.dev != files.stat_self()?.st_dev {
            return Err(invalid("Source and Trash are on different volumes"));
        }
        let base = source_name.as_bytes();
        let base = OsStr::from_bytes(&base[..base.len().min(220)]);
        for n in 0..10000 {
            check(cancel)?;
            let mut name = base.to_os_string();
            if n != 0 {
                name.push(format!(".{}-{n}", std::process::id()));
            }
            match files.stat_child(&name) {
                Ok(_) => continue,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            let iname = info_name(&name)?;
            let mut record = match rustix::fs::openat(
                info.as_fd(),
                &iname,
                OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_bits_truncate(0o600),
            ) {
                Ok(fd) => File::from(fd),
                Err(rustix::io::Errno::EXIST) => continue,
                Err(e) => return Err(e.into()),
            };
            let info_path = self.trash.info.join(&iname);
            let target = self.trash.files.join(&name);
            let locations = vec![path.into(), target.clone(), info_path.clone()];
            // The fsynced exclusive record is the forward restore intent. Never
            // unlink its publicly mutable name on an uncertain error.
            let admitted = (|| -> io::Result<TrashReceipt> {
                let bytes = format!(
                    "[Trash Info]\nPath={}\nDeletionDate={}\nX-Gust-Identity={}\n",
                    super::percent_encode(original.as_os_str().as_bytes()),
                    super::iso_now(),
                    serde_json::to_string(&crate::journal::RecordedIdentity::from(data))?
                )
                .into_bytes();
                record.write_all(&bytes)?;
                record.sync_all()?;
                rustix::fs::fsync(info.as_fd())?;
                let info_id = FileIdentity::from_stat(&rustix::fs::fstat(&record)?);
                let (_, now, read) = open_info(&info, &iname)?;
                if now != info_id || read != bytes {
                    return Err(invalid("Trash intent changed before data move"));
                }
                move_checked(
                    &source,
                    &source_name,
                    path,
                    &files,
                    &name,
                    &target,
                    &tree,
                    cancel,
                )?;
                let receipt = TrashReceipt {
                    item: TrashedItem {
                        original: path.into(),
                        trashed_name: name.clone(),
                        info_file: info_path.clone(),
                    },
                    location: self.clone(),
                    canonical: self.clone(),
                    original: path.into(),
                    name: name.clone(),
                    root_dir,
                    top_dir,
                    original_parent: Some(FileIdentity::from_stat(&source.stat_self()?)),
                    files_dir: FileIdentity::from_stat(&files.stat_self()?),
                    info_dir: FileIdentity::from_stat(&info.stat_self()?),
                    data,
                    info: info_id,
                    tree: remap(&tree, path, &target)?,
                    info_bytes: bytes,
                    inventory_mount: None,
                };
                receipt.verify(cancel)?;
                Ok(receipt)
            })();
            return admitted.map_err(|e| partial(e, locations));
        }
        Err(io::Error::other("No free Trash slot is available"))
    }
    pub fn receipt_named(&self, name: &OsStr, cancel: &CancelToken) -> io::Result<TrashReceipt> {
        self.receipt_named_inner(name, cancel, true, None)
    }
    fn receipt_named_inner(
        &self,
        name: &OsStr,
        cancel: &CancelToken,
        restore_parent: bool,
        mount: Option<u64>,
    ) -> io::Result<TrashReceipt> {
        check(cancel)?;
        let iname = info_name(name)?;
        let (files, info) = self.handles(false)?;
        let (_, info_id, bytes) = open_info_with_mount(&info, &iname, mount)?;
        let (original, _, recorded) = parse_info(&bytes, self)?;
        if original.starts_with(self.trash.root()) || self.trash.root().starts_with(&original) {
            return Err(invalid("Unsafe Trash restore destination"));
        }
        let path = self.trash.files.join(name);
        let tree = snapshot_at_with_mount(&files, name, &path, cancel, mount)?;
        let data = *tree
            .get(&path)
            .ok_or_else(|| invalid("Missing Trash data"))?;
        if recorded.is_some_and(|id| !id.matches(&data)) {
            let binding = lineage::Binding::new(
                self,
                name,
                FileIdentity::from_stat(&files.stat_self()?),
                FileIdentity::from_stat(&info.stat_self()?),
                info_id,
                bytes.clone(),
            );
            if !binding.matches_with_mount(&info, &tree, &mut 0, mount)? {
                return Err(invalid("Trash slot was replaced or changed"));
            }
        }
        let receipt = TrashReceipt {
            item: TrashedItem {
                original: original.clone(),
                trashed_name: name.into(),
                info_file: self.trash.info.join(iname),
            },
            location: self.clone(),
            canonical: self.clone(),
            original_parent: if restore_parent {
                optional_dir(
                    original
                        .parent()
                        .ok_or_else(|| invalid("Missing restore parent"))?,
                )?
            } else {
                None
            },
            original,
            name: name.into(),
            root_dir: FileIdentity::from_stat(
                &history_tree::open_dir(self.trash.root())?.stat_self()?,
            ),
            top_dir: self
                .topdir
                .as_ref()
                .map(|p| {
                    history_tree::open_dir(p)
                        .and_then(|d| d.stat_self())
                        .map(|s| FileIdentity::from_stat(&s))
                })
                .transpose()?,
            files_dir: FileIdentity::from_stat(&files.stat_self()?),
            info_dir: FileIdentity::from_stat(&info.stat_self()?),
            data,
            info: info_id,
            tree,
            info_bytes: bytes,
            inventory_mount: mount,
        };
        receipt.verify(cancel)?;
        Ok(receipt)
    }
    pub fn restore(
        &self,
        receipt: &TrashReceipt,
        cancel: &CancelToken,
    ) -> io::Result<RestoredReceipt> {
        check(cancel)?;
        if self != &receipt.canonical {
            return Err(invalid("Receipt belongs to another Trash"));
        }
        receipt.verify(cancel)?;
        let (files, info) = self.handles(false)?;
        same_dir(&files, receipt.files_dir)?;
        same_dir(&info, receipt.info_dir)?;
        let original = &receipt.original;
        let parent_path = original
            .parent()
            .ok_or_else(|| invalid("Missing restore parent"))?;
        let target_name = original
            .file_name()
            .ok_or_else(|| invalid("Missing restore name"))?;
        let initial_parent = match history_tree::open_dir(parent_path) {
            Ok(parent) => {
                if let Some(expected) = receipt.original_parent {
                    inode_dir(&parent, expected)?;
                }
                ensure_absent(&parent, target_name)?;
                Some(parent)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        let created_parents = initial_parent.is_none();
        // Prepare intent before creating missing original parents or moving data.
        let (vault, vault_path) = recovery_vault(&info, &self.trash.info)?;
        let retained_info = vault_path.join("info");
        let source_path = self.trash.files.join(&receipt.name);
        let info_path = self.trash.info.join(info_name(&receipt.name)?);
        let evidence = vec![
            original.clone(),
            source_path.clone(),
            info_path.clone(),
            vault_path.clone(),
        ];
        let result = (|| -> io::Result<RestoredReceipt> {
            let intent = serde_json::to_vec(&serde_json::json!({"version":1,
                "original":original.as_os_str().as_bytes(),"data":source_path.as_os_str().as_bytes(),
                "info":info_path.as_os_str().as_bytes(),"data_identity":crate::journal::RecordedIdentity::from(receipt.data),
                "info_identity":crate::journal::RecordedIdentity::from(receipt.info)}))?;
            let mut file = File::from(rustix::fs::openat(
                vault.as_fd(),
                "intent.json",
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )?);
            file.write_all(&intent)?;
            file.sync_all()?;
            rustix::fs::fsync(vault.as_fd())?;
            rustix::fs::fsync(info.as_fd())?;
            check(cancel)?;
            #[cfg(test)]
            BEFORE_RESTORE_PARENT.with(|hook| {
                let hook = hook.borrow_mut().take();
                if let Some(hook) = hook {
                    hook()
                }
            });
            let target = match initial_parent {
                Some(target) => {
                    // An absent-parent receipt may meet a newly existing parent.
                    // Hold that exact object rather than recreating its public
                    // path if another actor renames it after this admission.
                    inode_dir(
                        &history_tree::open_dir(parent_path)?,
                        FileIdentity::from_stat(&target.stat_self()?),
                    )?;
                    target
                }
                None => history_tree::ensure_dir(parent_path)?,
            };
            if target.stat_self()?.st_dev != receipt.data.dev {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Restore would cross volumes",
                ));
            }
            receipt.verify(cancel)?;
            move_checked(
                &files,
                &receipt.name,
                &source_path,
                &target,
                target_name,
                original,
                &receipt.tree,
                cancel,
            )?;
            let info_tree = BTreeMap::from([(info_path.clone(), receipt.info)]);
            // After the data effect, complete record retention even if the
            // caller cancels. No success is reported before both are durable.
            move_checked(
                &info,
                &info_name(&receipt.name)?,
                &info_path,
                &vault,
                OsStr::new("info"),
                &retained_info,
                &info_tree,
                &CancelToken::new(),
            )?;
            let restored = RestoredReceipt {
                original: original.clone(),
                trash: receipt.clone(),
                retained_info,
                retained_dir: FileIdentity::from_stat(&vault.stat_self()?),
                restored_parent: FileIdentity::from_stat(&target.stat_self()?),
                created_parents,
            };
            restored.verify(&CancelToken::new())?;
            Ok(restored)
        })();
        result.map_err(|e| partial(e, evidence))
    }
    pub fn list(&self) -> io::Result<Vec<TrashEntry>> {
        let (files, info) = self.handles(false)?;
        let mut out = Vec::new();
        let mut seen = 0usize;
        let mut metadata = 0usize;
        let mut lineage_nodes = 0usize;
        for entry in rustix::fs::Dir::read_from(info.as_fd())? {
            seen += 1;
            if seen > 200_002 {
                return Err(invalid("Trash listing exceeds entry limit"));
            }
            let entry = entry?;
            let Some(raw) = entry.file_name().to_bytes().strip_suffix(b".trashinfo") else {
                continue;
            };
            let name = OsStr::from_bytes(raw);
            let iname = info_name(name)?;
            let Ok((_, info_id, bytes)) = open_info(&info, &iname) else {
                continue;
            };
            metadata = metadata.saturating_add(bytes.len());
            if metadata > 32 * 1024 * 1024 {
                return Err(invalid("Trash listing exceeds metadata limit"));
            }
            let Ok((original, deleted, recorded)) = parse_info(&bytes, self) else {
                continue;
            };
            let Ok(stat) = files.stat_child(name) else {
                continue;
            };
            if recorded.is_some_and(|id| !id.matches(&FileIdentity::from_stat(&stat))) {
                let tree = history_tree::snapshot_at(
                    &files,
                    name,
                    &self.trash.files.join(name),
                    &CancelToken::new(),
                )?;
                lineage_nodes = lineage_nodes.saturating_add(tree.len());
                metadata = tree.keys().fold(metadata, |total, path| {
                    total.saturating_add(path.as_os_str().as_bytes().len() + 128)
                });
                if metadata > 32 * 1024 * 1024 || lineage_nodes > 200_000 {
                    return Err(invalid("Trash listing exceeds lineage limit"));
                }
                let binding = lineage::Binding::new(
                    self,
                    name,
                    FileIdentity::from_stat(&files.stat_self()?),
                    FileIdentity::from_stat(&info.stat_self()?),
                    info_id,
                    bytes,
                );
                if !binding.matches(&info, &tree, &mut metadata)? {
                    continue;
                }
            }
            out.push(TrashEntry {
                trashed_name: name.to_os_string(),
                original,
                deleted,
            });
            if out.len() > 200_000 {
                return Err(invalid("Trash listing exceeds entry limit"));
            }
        }
        out.sort_by(|a, b| {
            b.deleted
                .cmp(&a.deleted)
                .then_with(|| a.trashed_name.cmp(&b.trashed_name))
        });
        Ok(out)
    }
}
fn mount_error(error: MountTableError) -> io::Error {
    let kind = match error {
        MountTableError::Ambiguous | MountTableError::NoMount => io::ErrorKind::Unsupported,
        _ => io::ErrorKind::InvalidData,
    };
    io::Error::new(kind, format!("Trash mount topology refused: {error:?}"))
}
fn mounts() -> io::Result<MountInfo> {
    let file = File::open("/proc/self/mountinfo")?;
    let mut bytes = Vec::new();
    file.take(MAX_MOUNTINFO_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    parse_mounts(&bytes)
}
fn parse_mounts(bytes: &[u8]) -> io::Result<MountInfo> {
    MountInfo::parse(bytes).map_err(mount_error)
}
fn local_fstype(kind: &str) -> bool {
    matches!(
        kind,
        "ext2"
            | "ext3"
            | "ext4"
            | "btrfs"
            | "xfs"
            | "tmpfs"
            | "ramfs"
            | "overlay"
            | "zfs"
            | "bcachefs"
            | "f2fs"
            | "exfat"
            | "vfat"
            | "msdos"
            | "ntfs3"
    )
}
fn local_path(path: &Path, mounts: &MountInfo) -> io::Result<()> {
    let mount = mounts.resolve(path).map_err(mount_error)?;
    if local_fstype(&mount.fs_type) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Automatic Trash is available only on known local filesystems",
        ))
    }
}
fn local_location(location: &TrashLocation, mounts: &MountInfo) -> io::Result<()> {
    let mount = mounts
        .resolve(location.trash.root())
        .map_err(mount_error)?
        .id;
    same_local_mount(location.trash.root(), mount, mounts)?;
    for path in [&location.trash.files, &location.trash.info] {
        same_local_mount(path, mount, mounts)?;
    }
    if let Some(top) = &location.topdir {
        same_local_mount(top, mount, mounts)?;
    }
    Ok(())
}
fn same_local_mount(path: &Path, expected: u64, mounts: &MountInfo) -> io::Result<()> {
    local_path(path, mounts)?;
    if mounts.resolve(path).map_err(mount_error)?.id != expected {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Trash requires one rename-compatible mount",
        ));
    }
    Ok(())
}
fn nearest_dev(path: &Path) -> io::Result<u64> {
    for path in path.ancestors() {
        match history_tree::open_dir(path) {
            Ok(d) => return Ok(d.stat_self()?.st_dev),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Err(invalid("No existing location ancestor"))
}
#[cfg(test)]
thread_local! {
    static SHARED_OPEN_PROBE:std::cell::Cell<Option<bool>>=const {std::cell::Cell::new(None)};
}
fn open_shared(path: &Path) -> io::Result<DirHandle> {
    #[cfg(test)]
    if SHARED_OPEN_PROBE.with(|probe| match probe.get() {
        Some(_) => {
            probe.set(Some(true));
            true
        }
        None => false,
    }) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "test probe; no candidate I/O",
        ));
    }
    history_tree::open_dir(path)
}
fn candidates(top: &Path, mounts: &MountInfo) -> Vec<TrashLocation> {
    let uid = rustix::process::geteuid().as_raw();
    let shared = top.join(".Trash");
    let mut out = Vec::new();
    if mounts
        .resolve(top)
        .is_ok_and(|top| same_local_mount(&shared, top.id, mounts).is_ok())
        && open_shared(&shared).is_ok_and(|d| d.stat_self().is_ok_and(|s| s.st_mode & 0o1000 != 0))
    {
        out.push(TrashLocation {
            trash: TrashDir::at(shared.join(uid.to_string())),
            topdir: Some(top.to_path_buf()),
        });
    }
    out.push(TrashLocation {
        trash: TrashDir::at(top.join(format!(".Trash-{uid}"))),
        topdir: Some(top.to_path_buf()),
    });
    out
}
fn usable(location: &TrashLocation, dev: u64) -> io::Result<()> {
    if let Some(root) = existing_private(location.trash.root())? {
        if root.stat_self()?.st_dev != dev {
            return Err(invalid("Trash root is on another filesystem"));
        }
        for name in ["files", "info"] {
            match root.open_child_dir(OsStr::new(name)) {
                Ok(dir) => {
                    location.directory_mode(&dir, false)?;
                    if dir.stat_self()?.st_dev != dev {
                        return Err(invalid("Trash child is on another filesystem"));
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}
/// Read-only selection. Directory creation belongs to the admitted mutation.
pub fn select_location(path: &Path, home: &TrashDir) -> io::Result<TrashLocation> {
    let mounts = mounts()?;
    select_location_with(path, home, &mounts)
}
fn select_location_with(
    path: &Path,
    home: &TrashDir,
    mounts: &MountInfo,
) -> io::Result<TrashLocation> {
    local_path(path, mounts)?;
    let source_mount = mounts.resolve(path).map_err(mount_error)?.id;
    let (parent, name) = anchor::resolve_parent(path)?;
    let dev = parent.stat_child(&name)?.st_dev;
    let home_location = TrashLocation {
        trash: home.clone(),
        topdir: None,
    };
    if same_local_mount(home.root(), source_mount, mounts).is_ok()
        && local_location(&home_location, mounts).is_ok()
        && nearest_dev(home.root())? == dev
    {
        usable(&home_location, dev)?;
        return Ok(home_location);
    }
    let top = mounts
        .resolve(path)
        .map_err(mount_error)?
        .mount_point
        .clone();
    for location in candidates(&top, mounts) {
        if same_local_mount(location.trash.root(), source_mount, mounts).is_ok()
            && local_location(&location, mounts).is_ok()
            && usable(&location, dev).is_ok()
        {
            return Ok(location);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "No safe same-volume Trash location is available",
    ))
}
fn inventory_roots(mounts: &MountInfo) -> io::Result<Vec<PathBuf>> {
    Ok(mounts
        .visible_mounts()
        .map_err(mount_error)?
        .into_iter()
        .filter(|m| local_fstype(&m.fs_type))
        .map(|m| m.mount_point.clone())
        .collect())
}
/// Existing locations only; listing never creates directories on mounted media.
pub fn locations(home: &TrashDir) -> io::Result<Vec<TrashLocation>> {
    locations_with(home, &mounts()?)
}
fn locations_with(home: &TrashDir, mounts: &MountInfo) -> io::Result<Vec<TrashLocation>> {
    let mut out = Vec::new();
    let home_location = TrashLocation {
        trash: home.clone(),
        topdir: None,
    };
    // Refuse an incomplete inventory before touching any candidate directory.
    let roots = inventory_roots(mounts)?;
    if local_location(&home_location, mounts).is_ok()
        && home_location.handles_mode(false, true).is_ok()
    {
        out.push(home_location);
    }
    for top in roots {
        for location in candidates(&top, mounts) {
            if local_location(&location, mounts).is_ok()
                && location.handles_mode(false, true).is_ok()
                && !out.contains(&location)
            {
                out.push(location);
            }
        }
    }
    Ok(out)
}

fn inode_dir(dir: &DirHandle, expected: FileIdentity) -> io::Result<()> {
    let now = dir.stat_self()?;
    if (now.st_dev, now.st_ino) != (expected.dev, expected.ino) {
        return Err(invalid("Bound directory was replaced"));
    }
    Ok(())
}
fn optional_dir(path: &Path) -> io::Result<Option<FileIdentity>> {
    match history_tree::open_dir(path) {
        Ok(dir) => Ok(Some(FileIdentity::from_stat(&dir.stat_self()?))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
fn ensure_absent(parent: &DirHandle, name: &OsStr) -> io::Result<()> {
    match parent.stat_child(name) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Restore name is occupied; all work was preserved",
        )),
        Err(e) => Err(e),
    }
}
#[derive(Debug)]
struct InventoryFailure(crate::permanent::DeleteFailure);
impl std::fmt::Display for InventoryFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Trash mount-aware inventory refused: {:?}", self.0)
    }
}
impl std::error::Error for InventoryFailure {}
fn inventory_error(reason: crate::permanent::DeleteFailure) -> io::Error {
    use crate::permanent::DeleteFailure;
    let kind = match reason {
        DeleteFailure::Cancelled => io::ErrorKind::Interrupted,
        DeleteFailure::Unsupported => io::ErrorKind::Unsupported,
        DeleteFailure::Io(kind) => kind,
        _ => io::ErrorKind::InvalidData,
    };
    io::Error::new(kind, InventoryFailure(reason))
}
fn check_fd_mount(fd: BorrowedFd<'_>, name: &OsStr, mount: Option<u64>) -> io::Result<()> {
    if let Some(mount) = mount {
        if crate::permanent::fd_mount_id(fd, OsStr::new("")).map_err(inventory_error)? != mount
            || crate::permanent::fd_mount_id(fd, name).map_err(inventory_error)? != mount
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Trash mount binding changed",
            ));
        }
    }
    Ok(())
}
fn check_mount_at(dir: &DirHandle, name: &OsStr, mount: Option<u64>) -> io::Result<()> {
    check_fd_mount(dir.as_fd(), name, mount)
}
fn snapshot_at_with_mount(
    parent: &DirHandle,
    name: &OsStr,
    path: &Path,
    cancel: &CancelToken,
    mount: Option<u64>,
) -> io::Result<BTreeMap<PathBuf, FileIdentity>> {
    match mount {
        Some(mount) => crate::permanent::inventory_at(parent, name, mount, cancel)
            .map(|tree| {
                tree.into_iter()
                    .map(|(relative, id)| (path.join(relative), id))
                    .collect()
            })
            .map_err(inventory_error),
        None => history_tree::snapshot_at(parent, name, path, cancel),
    }
}
fn verify_at(
    parent: &DirHandle,
    name: &OsStr,
    path: &Path,
    tree: &BTreeMap<PathBuf, FileIdentity>,
    cancel: &CancelToken,
) -> io::Result<()> {
    verify_at_with_mount(parent, name, path, tree, cancel, None)
}
fn verify_at_with_mount(
    parent: &DirHandle,
    name: &OsStr,
    path: &Path,
    tree: &BTreeMap<PathBuf, FileIdentity>,
    cancel: &CancelToken,
    mount: Option<u64>,
) -> io::Result<()> {
    let current = snapshot_at_with_mount(parent, name, path, cancel, mount)?;
    if &current != tree {
        return Err(invalid("Trash subtree changed; current work was preserved"));
    }
    Ok(())
}
#[cfg(test)]
type MoveHook = Box<dyn FnOnce()>;
#[cfg(test)]
thread_local! {
    static AFTER_LEGACY_REPAIR:std::cell::RefCell<Option<MoveHook>>=const { std::cell::RefCell::new(None) };
    static BEFORE_LEGACY_REPAIR:std::cell::RefCell<Option<MoveHook>>=const { std::cell::RefCell::new(None) };
    static AFTER_MOVE:std::cell::RefCell<Option<MoveHook>>=const { std::cell::RefCell::new(None) };
    static BEFORE_MOVE:std::cell::RefCell<Option<MoveHook>>=const { std::cell::RefCell::new(None) };
    static BEFORE_RESTORE_PARENT:std::cell::RefCell<Option<MoveHook>>=const { std::cell::RefCell::new(None) };
}
/// NOREPLACE protects target occupancy. Held-parent pre/post inventories bind
/// the source for the tested race; a mismatching move is restored only while
/// the observed moved inode is unchanged and its old name remains free.
#[allow(clippy::too_many_arguments)]
fn move_checked(
    source: &DirHandle,
    name: &OsStr,
    from: &Path,
    target: &DirHandle,
    to_name: &OsStr,
    to: &Path,
    tree: &BTreeMap<PathBuf, FileIdentity>,
    cancel: &CancelToken,
) -> io::Result<()> {
    move_checked_on_mount(source, name, from, target, to_name, to, tree, cancel, None)
}
#[allow(clippy::too_many_arguments)]
fn move_checked_on_mount(
    source: &DirHandle,
    name: &OsStr,
    from: &Path,
    target: &DirHandle,
    to_name: &OsStr,
    to: &Path,
    tree: &BTreeMap<PathBuf, FileIdentity>,
    cancel: &CancelToken,
    mount: Option<u64>,
) -> io::Result<()> {
    check_mount_at(source, OsStr::new(""), mount)?;
    check_mount_at(target, OsStr::new(""), mount)?;
    verify_at_with_mount(source, name, from, tree, cancel, mount)?;
    ensure_absent(target, to_name)?;
    check(cancel)?;
    #[cfg(test)]
    BEFORE_MOVE.with(|h| {
        if let Some(f) = h.borrow_mut().take() {
            f()
        }
    });
    check(cancel)?;
    anchor::rename_noreplace(source, name, target, to_name)?;
    #[cfg(test)]
    AFTER_MOVE.with(|h| {
        let f = h.borrow_mut().take();
        if let Some(f) = f {
            f()
        }
    });
    let evidence = vec![from.into(), to.into()];
    check_mount_at(target, to_name, mount).map_err(|e| partial(e, evidence.clone()))?;
    let moved = target
        .stat_child(to_name)
        .map(|s| FileIdentity::from_stat(&s))
        .map_err(|e| partial(e, evidence.clone()))?;
    let mapped = remap(tree, from, to)?;
    if let Err(error) = verify_at_with_mount(target, to_name, to, &mapped, cancel, mount) {
        let recover = (|| -> io::Result<()> {
            if FileIdentity::from_stat(&target.stat_child(to_name)?) != moved {
                return Err(invalid("Moved item changed before recovery"));
            }
            anchor::rename_noreplace(target, to_name, source, name)?;
            if FileIdentity::from_stat(&source.stat_child(name)?) != moved {
                return Err(invalid("Restored name changed during recovery"));
            }
            rustix::fs::fsync(target.as_fd())?;
            rustix::fs::fsync(source.as_fd())?;
            Ok(())
        })();
        return Err(partial(
            io::Error::new(
                error.kind(),
                match recover {
                    Ok(()) => format!("{error}; observed item restored without replacement"),
                    Err(e) => format!("{error}; recovery refused: {e}"),
                },
            ),
            evidence,
        ));
    }
    rustix::fs::fsync(source.as_fd()).map_err(|e| partial(e.into(), evidence.clone()))?;
    rustix::fs::fsync(target.as_fd()).map_err(|e| partial(e.into(), evidence))?;
    Ok(())
}
fn recovery_vault(info: &DirHandle, info_path: &Path) -> io::Result<(DirHandle, PathBuf)> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    for _ in 0..64 {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let name = format!(
            ".gust-restore-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        match rustix::fs::mkdirat(info.as_fd(), name.as_str(), Mode::from_bits_truncate(0o700)) {
            Ok(()) => {
                let path = info_path.join(&name);
                let opened = (|| -> io::Result<DirHandle> {
                    let dir = info.open_child_dir(OsStr::new(&name))?;
                    private(&dir)?;
                    rustix::fs::fsync(info.as_fd())?;
                    Ok(dir)
                })();
                return opened
                    .map(|dir| (dir, path.clone()))
                    .map_err(|e| partial(e, vec![path]));
            }
            Err(rustix::io::Errno::EXIST) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Err(io::Error::other(
        "Cannot reserve private Trash recovery storage",
    ))
}
impl TrashReceipt {
    /// Complete retained subtree count; no filesystem access.
    pub fn inventory_len(&self) -> usize {
        self.tree.len()
    }
    /// Conservative cache accounting charge, not an allocator/RSS reading.
    /// Includes every owned path/slot capacity, info buffer capacity and a
    /// per-node allowance for BTreeMap allocation/slack. Saturates on overflow.
    pub fn retained_bytes(&self) -> usize {
        let capacities = [
            self.item.original.capacity(),
            self.item.trashed_name.capacity(),
            self.item.info_file.capacity(),
            self.location.trash.files.capacity(),
            self.location.trash.info.capacity(),
            self.location.topdir.as_ref().map_or(0, PathBuf::capacity),
            self.canonical.trash.files.capacity(),
            self.canonical.trash.info.capacity(),
            self.canonical.topdir.as_ref().map_or(0, PathBuf::capacity),
            self.original.capacity(),
            self.name.capacity(),
            self.info_bytes.capacity(),
        ];
        let owned = capacities
            .into_iter()
            .fold(std::mem::size_of::<Self>(), usize::saturating_add);
        self.tree.keys().fold(owned, |total, path| {
            total.saturating_add(path.capacity()).saturating_add(2048)
        })
    }
    /// Worker-only read-only preflight. If false, reserve a durable history
    /// barrier before restoring missing parents. Races still require the
    /// caller's forward marker and the terminal receipt's history flag.
    pub fn restore_history_supported(&self, cancel: &CancelToken) -> io::Result<bool> {
        self.verify(cancel)?;
        let path = self
            .original
            .parent()
            .ok_or_else(|| invalid("Missing restore parent"))?;
        match history_tree::open_dir(path) {
            Ok(parent) => {
                if let Some(expected) = self.original_parent {
                    inode_dir(&parent, expected)?;
                }
                Ok(true)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }
    fn bindings(&self) -> io::Result<(DirHandle, DirHandle)> {
        if self.location != self.canonical
            || self.item.original != self.original
            || self.item.trashed_name != self.name
            || self.item.info_file != self.canonical.trash.info.join(info_name(&self.name)?)
        {
            return Err(invalid("Trash display receipt was modified"));
        }
        let (files, info) = self.canonical.handles(false)?;
        same_dir(
            &history_tree::open_dir(self.canonical.trash.root())?,
            self.root_dir,
        )?;
        if let (Some(path), Some(expected)) = (&self.canonical.topdir, self.top_dir) {
            inode_dir(&history_tree::open_dir(path)?, expected)?;
        }
        same_dir(&files, self.files_dir)?;
        same_dir(&info, self.info_dir)?;
        let (original, _, recorded) = parse_info(&self.info_bytes, &self.canonical)?;
        if original != self.original {
            return Err(invalid("Trash receipt has invalid original binding"));
        }
        if recorded.is_some_and(|id| !id.matches(&self.data))
            && !self.lineage_binding().matches_with_mount(
                &info,
                &self.tree,
                &mut 0,
                self.inventory_mount,
            )?
        {
            return Err(invalid("Trash receipt has no matching directory lineage"));
        }
        Ok((files, info))
    }
    pub(crate) fn lineage_binding(&self) -> lineage::Binding {
        lineage::Binding::new(
            &self.canonical,
            &self.name,
            self.files_dir,
            self.info_dir,
            self.info,
            self.info_bytes.clone(),
        )
    }
    pub(crate) fn verify(&self, cancel: &CancelToken) -> io::Result<()> {
        let (files, info) = self.bindings()?;
        let (_, id, bytes) =
            open_info_with_mount(&info, &info_name(&self.name)?, self.inventory_mount)?;
        if id != self.info || bytes != self.info_bytes {
            return Err(invalid("Trash information changed"));
        }
        let path = self.canonical.trash.files.join(&self.name);
        verify_at_with_mount(
            &files,
            &self.name,
            &path,
            &self.tree,
            cancel,
            self.inventory_mount,
        )?;
        if self.tree.get(&path) != Some(&self.data) {
            return Err(invalid("Trash receipt root mismatch"));
        }
        Ok(())
    }
    pub(crate) fn data_path(&self) -> PathBuf {
        self.canonical.trash.files.join(&self.name)
    }
    pub(crate) fn info_path(&self) -> PathBuf {
        self.canonical
            .trash
            .info
            .join(info_name(&self.name).expect("private admitted Trash name"))
    }
    pub(crate) fn verify_original_parent(&self) -> io::Result<()> {
        let parent = history_tree::open_dir(
            self.original
                .parent()
                .ok_or_else(|| invalid("Missing original parent"))?,
        )?;
        inode_dir(
            &parent,
            self.original_parent
                .ok_or_else(|| invalid("Original parent has no identity receipt"))?,
        )
    }
    pub(crate) fn original_path(&self) -> &Path {
        &self.original
    }
}
impl RestoredReceipt {
    pub(crate) fn verify(&self, cancel: &CancelToken) -> io::Result<()> {
        let (_, info) = self.trash.bindings()?;
        if self.original != self.trash.original {
            return Err(invalid("Restored display path was modified"));
        }
        let (parent, name) = anchor::resolve_parent(&self.retained_info)?;
        same_dir(&parent, self.retained_dir)?;
        let (_, id, bytes) = open_info(&parent, &name)?;
        if id != self.trash.info || bytes != self.trash.info_bytes {
            return Err(invalid("Retained Trash record changed"));
        }
        let (target, name) = anchor::resolve_parent(&self.original)?;
        inode_dir(&target, self.restored_parent)?;
        let mapped = remap(&self.trash.tree, &self.trash.data_path(), &self.original)?;
        verify_at(&target, &name, &self.original, &mapped, cancel)?;
        ensure_absent(&info, &info_name(&self.trash.name)?)?;
        Ok(())
    }
    pub(crate) fn info_path(&self) -> &Path {
        &self.retained_info
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};
    fn fixture() -> (PathBuf, TrashLocation, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "gust-trash-race-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("work")).unwrap();
        let file = root.join("work/file");
        fs::write(&file, b"original").unwrap();
        let l = TrashLocation {
            trash: TrashDir::at(root.join("Trash")),
            topdir: None,
        };
        (root, l, file)
    }
    #[test]
    fn absent_parent_receipt_never_recreates_a_parent_renamed_after_initial_open() {
        let (root, location, file) = fixture();
        let initial = location.trash_path(&file, &CancelToken::new()).unwrap();
        let parent = file.parent().unwrap().to_path_buf();
        fs::remove_dir(&parent).unwrap();
        let receipt = location
            .receipt_named(&initial.item.trashed_name, &CancelToken::new())
            .unwrap();
        assert!(receipt.original_parent.is_none());
        fs::create_dir(&parent).unwrap();
        let parent_id = FileIdentity::of_path(&parent).unwrap();
        assert!(receipt
            .restore_history_supported(&CancelToken::new())
            .unwrap());
        let kept = root.join("actor-parent");
        BEFORE_RESTORE_PARENT.with(|hook| {
            let parent = parent.clone();
            let kept = kept.clone();
            *hook.borrow_mut() = Some(Box::new(move || fs::rename(parent, kept).unwrap()));
        });
        let result = location.restore(&receipt, &CancelToken::new());
        assert!(
            result.is_err(),
            "a renamed parent must refuse, not report a reversible recreation: {result:?}"
        );
        let error = result.unwrap_err();
        assert!(error
            .get_ref()
            .unwrap()
            .downcast_ref::<TrashPartial>()
            .is_some());
        assert!(!parent.exists());
        assert_eq!(FileIdentity::of_path(&kept).unwrap(), parent_id);
        assert_eq!(
            fs::read(location.trash.files.join(&receipt.name)).unwrap(),
            b"original"
        );
        assert_eq!(
            fs::read(&receipt.item.info_file).unwrap(),
            receipt.info_bytes
        );
        assert!(fs::read_dir(&location.trash.info)
            .unwrap()
            .any(|entry| entry.unwrap().path().join("intent.json").exists()));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn mismatching_source_is_restored_without_adopting_a_receipt() {
        let (root, l, file) = fixture();
        let source = file.clone();
        let kept = root.join("kept");
        let keep = kept.clone();
        BEFORE_MOVE.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                fs::rename(&source, &keep).unwrap();
                fs::write(&source, b"replacement").unwrap();
            }))
        });
        let e = l.trash_path(&file, &CancelToken::new()).unwrap_err();
        assert!(e
            .get_ref()
            .unwrap()
            .downcast_ref::<TrashPartial>()
            .is_some());
        assert_eq!(fs::read(&file).unwrap(), b"replacement");
        assert_eq!(fs::read(&kept).unwrap(), b"original");
        assert!(l.list().unwrap().is_empty());
        assert!(l.trash.info.join("file.trashinfo").exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn blocked_identity_bound_recovery_reports_all_locations() {
        let (root, l, file) = fixture();
        let source = file.clone();
        let keep = root.join("kept");
        let kept = keep.clone();
        BEFORE_MOVE.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || {
                fs::rename(&source, &keep).unwrap();
                fs::write(&source, b"replacement").unwrap();
            }))
        });
        AFTER_MOVE.with(|h| {
            *h.borrow_mut() = Some(Box::new({
                let file = file.clone();
                move || fs::write(file, b"new work").unwrap()
            }))
        });
        let e = l.trash_path(&file, &CancelToken::new()).unwrap_err();
        let p = e.get_ref().unwrap().downcast_ref::<TrashPartial>().unwrap();
        assert!(p.locations.contains(&file));
        assert!(p.locations.contains(&l.trash.files.join("file")));
        assert_eq!(fs::read(&file).unwrap(), b"new work");
        assert_eq!(fs::read(kept).unwrap(), b"original");
        assert_eq!(
            fs::read(l.trash.files.join("file")).unwrap(),
            b"replacement"
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn cancellation_after_durable_info_retains_intent_and_source() {
        let (root, l, file) = fixture();
        let c = CancelToken::new();
        let stop = c.clone();
        BEFORE_MOVE.with(|h| *h.borrow_mut() = Some(Box::new(move || stop.cancel())));
        let e = l.trash_path(&file, &c).unwrap_err();
        assert!(e
            .get_ref()
            .unwrap()
            .downcast_ref::<TrashPartial>()
            .is_some());
        assert_eq!(fs::read(file).unwrap(), b"original");
        assert!(l.trash.info.join("file.trashinfo").exists());
        assert!(l.list().unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn mount_decode_is_shared_single_pass_and_network_is_skipped() {
        let mounts=parse_mounts(b"1 0 0:1 / / rw - ext4 disk rw\n2 1 0:2 / /a\\040b\\134040c\\011d\\012e\xff rw - ext4 disk rw\n3 2 0:3 / /a\\040b\\134040c\\011d\\012e\xff/remote rw - fuse.sshfs ssh rw\n").unwrap();
        let raw = Path::new(OsStr::from_bytes(b"/a b\\040c\td\ne\xff"));
        assert_eq!(mounts.resolve(raw).unwrap().mount_point, raw);
        assert!(local_path(raw, &mounts).is_ok());
        assert!(local_path(&raw.join("remote/file"), &mounts).is_err());
    }
    fn table(rows: &[(u64, u64, &str, &str)], reverse: bool) -> Vec<u8> {
        let mut rows: Vec<_> = rows
            .iter()
            .map(|(id, parent, path, kind)| {
                format!("{id} {parent} 0:1 / {path} rw - {kind} source rw\n")
            })
            .collect();
        if reverse {
            rows.reverse();
        }
        rows.concat().into_bytes()
    }
    #[test]
    fn visible_overmount_and_hidden_descendants_are_refused_before_trash_discovery() {
        for kind in ["fuse.custom-peer", "nfs4", "cifs", "unknownfs"] {
            for reverse in [false, true] {
                for equal in [false, true] {
                    let mut rows = vec![
                        (1, 0, "/", "ext4"),
                        (2, 1, "/synthetic/share/deep", "ext4"),
                        (3, 1, "/synthetic/share", "ext4"),
                        (4, 1, "/synthetic/local", "btrfs"),
                    ];
                    if equal {
                        rows.push((5, 3, "/synthetic/share", kind));
                    } else {
                        rows.retain(|r| r.0 != 3);
                        rows.push((5, 1, "/synthetic/share", kind));
                    }
                    let mounts = parse_mounts(&table(&rows, reverse)).unwrap();
                    assert!(
                        local_path(Path::new("/synthetic/share/deep/file"), &mounts).is_err(),
                        "{kind} reverse={reverse} equal={equal}"
                    );
                    assert!(local_path(Path::new("/synthetic/local/file"), &mounts).is_ok());
                    let roots = inventory_roots(&mounts).unwrap();
                    assert!(
                        !roots.iter().any(|p| p.starts_with("/synthetic/share")),
                        "{roots:?}"
                    );
                    assert!(roots.contains(&PathBuf::from("/synthetic/local")));
                }
            }
        }
    }
    #[test]
    fn ambiguous_topology_refuses_inventory_but_keeps_unrelated_paths_available() {
        for reverse in [false, true] {
            let mounts = parse_mounts(&table(
                &[
                    (1, 0, "/", "ext4"),
                    (2, 1, "/synthetic/share", "ext4"),
                    (3, 1, "/synthetic/share", "fuse.custom-peer"),
                    (4, 1, "/synthetic/local", "btrfs"),
                ],
                reverse,
            ))
            .unwrap();
            assert!(local_path(Path::new("/synthetic/share/file"), &mounts).is_err());
            assert!(local_path(Path::new("/synthetic/local/file"), &mounts).is_ok());
            assert!(inventory_roots(&mounts).is_err());
        }
    }
    #[test]
    fn shared_trash_is_classified_before_any_candidate_open() {
        let mounts = parse_mounts(&table(
            &[
                (1, 0, "/", "ext4"),
                (2, 1, "/synthetic/volume/.Trash", "fuse.custom-peer"),
            ],
            false,
        ))
        .unwrap();
        SHARED_OPEN_PROBE.with(|probe| probe.set(Some(false)));
        let _ = candidates(Path::new("/synthetic/volume"), &mounts);
        assert_eq!(
            SHARED_OPEN_PROBE.with(|probe| probe.replace(None)),
            Some(false)
        );
    }
    #[test]
    fn local_over_autofs_stays_available_without_record_order_assumptions() {
        for reverse in [false, true] {
            let mounts = parse_mounts(&table(
                &[
                    (1, 0, "/", "ext4"),
                    (2, 1, "/synthetic/home", "autofs"),
                    (3, 2, "/synthetic/home", "btrfs"),
                ],
                reverse,
            ))
            .unwrap();
            assert!(local_path(Path::new("/synthetic/home/file"), &mounts).is_ok());
            assert!(inventory_roots(&mounts)
                .unwrap()
                .contains(&PathBuf::from("/synthetic/home")));
        }
    }
    #[test]
    fn root_stacks_refuse_all_trash_paths_before_discovery() {
        for kind in ["fuse.sshfs", "nfs4", "cifs"] {
            for root_parent in [0, 1] {
                for child in [false, true] {
                    let mut rows = vec![(1, root_parent, "/", "ext4"), (2, 1, "/", "tmpfs")];
                    if child {
                        rows.push((3, 1, "/synthetic/remote", kind));
                    } else {
                        rows[0].3 = kind;
                    }
                    for reverse in [false, true] {
                        let mounts = parse_mounts(&table(&rows, reverse)).unwrap();
                        assert!(local_path(Path::new("/synthetic/remote/file"), &mounts).is_err());
                        assert!(local_path(Path::new("/synthetic/local/file"), &mounts).is_err());
                        assert!(inventory_roots(&mounts).is_err());
                        SHARED_OPEN_PROBE.with(|probe| probe.set(Some(false)));
                        let _ = candidates(Path::new("/synthetic/local"), &mounts);
                        assert_eq!(
                            SHARED_OPEN_PROBE.with(|probe| probe.replace(None)),
                            Some(false)
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn late_slot_collision_preserves_source_new_work_and_intent() {
        let (root, l, file) = fixture();
        let occupied = l.trash.files.join("file");
        let target = occupied.clone();
        BEFORE_MOVE.with(|h| {
            *h.borrow_mut() = Some(Box::new(move || fs::write(target, b"new work").unwrap()))
        });
        let e = l.trash_path(&file, &CancelToken::new()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert!(e
            .get_ref()
            .unwrap()
            .downcast_ref::<TrashPartial>()
            .is_some());
        assert_eq!(fs::read(&file).unwrap(), b"original");
        assert_eq!(fs::read(occupied).unwrap(), b"new work");
        assert!(l.trash.info.join("file.trashinfo").exists());
        assert!(l.list().unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unsupported_atomic_publish_never_substitutes_delete_or_overwrite() {
        let (root, l, file) = fixture();
        crate::anchor::UNSUPPORTED_ERRNO.with(|e| e.set(Some(rustix::io::Errno::OPNOTSUPP)));
        let e = l.trash_path(&file, &CancelToken::new()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Unsupported);
        assert_eq!(fs::read(&file).unwrap(), b"original");
        assert!(!l.trash.files.join("file").exists());
        assert!(l.trash.info.join("file.trashinfo").exists());
        assert!(e
            .get_ref()
            .unwrap()
            .downcast_ref::<TrashPartial>()
            .is_some());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn symlinked_shared_trash_is_never_traversed() {
        let (root, l, file) = fixture();
        let shared = root.join(".Trash");
        fs::create_dir(root.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), &shared).unwrap();
        let shared = TrashLocation {
            trash: TrashDir::at(shared.join(rustix::process::geteuid().as_raw().to_string())),
            topdir: Some(root.clone()),
        };
        assert!(shared.trash_path(&file, &CancelToken::new()).is_err());
        assert!(!l.trash.root().exists());
        assert_eq!(fs::read_dir(root.join("elsewhere")).unwrap().count(), 0);
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn equal_devices_on_distinct_mounts_choose_source_volume_trash() {
        let (root, home, file) = fixture();
        for reverse in [false, true] {
            let home_top = root.join("Trash");
            let volume = root.join("work");
            let rows = [
                (1, 0, "/", "ext4"),
                (2, 1, volume.to_str().unwrap(), "ext4"),
            ];
            let mounts = parse_mounts(&table(&rows, reverse)).unwrap();
            assert_eq!(
                nearest_dev(&home_top).unwrap(),
                FileIdentity::of_path(&file).unwrap().dev
            );
            let chosen = select_location_with(&file, &home.trash, &mounts).unwrap();
            assert_eq!(
                chosen.topdir,
                Some(volume),
                "device equality cannot choose home across bind mounts"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn network_home_is_skipped_without_access_for_local_volume_trash() {
        let (root, _, file) = fixture();
        let home_top = root.join("remote-home");
        std::os::unix::fs::symlink(root.join("missing"), &home_top).unwrap();
        let home = TrashDir::at(home_top.join("Trash"));
        for kind in ["nfs4", "fuse.sshfs", "unknownfs"] {
            for reverse in [false, true] {
                let volume = root.join("work");
                let rows = [
                    (1, 0, "/", "ext4"),
                    (2, 1, home_top.to_str().unwrap(), kind),
                    (3, 1, volume.to_str().unwrap(), "ext4"),
                ];
                let mounts = parse_mounts(&table(&rows, reverse)).unwrap();
                let result = select_location_with(&file, &home, &mounts);
                assert!(
                    result.is_ok(),
                    "unsupported home must not block a local volume: {result:?}"
                );
                assert_eq!(result.unwrap().topdir, Some(volume));
            }
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn separately_mounted_trash_children_refuse_even_on_equal_devices() {
        let rows = [
            (1, 0, "/", "ext4"),
            (2, 1, "/volume", "ext4"),
            (3, 2, "/volume/.Trash-0/files", "ext4"),
        ];
        let location = TrashLocation {
            trash: TrashDir::at("/volume/.Trash-0"),
            topdir: Some(PathBuf::from("/volume")),
        };
        for reverse in [false, true] {
            assert!(
                local_location(&location, &parse_mounts(&table(&rows, reverse)).unwrap()).is_err(),
                "distinct files mount must refuse before candidate access"
            );
        }
    }
    #[test]
    fn legacy_owned_layout_is_discovered_without_mutation() {
        let (root, location, _) = fixture();
        for dir in [
            location.trash.root(),
            &location.trash.files,
            &location.trash.info,
        ] {
            fs::create_dir_all(dir).unwrap();
            fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(location.trash.files.join("old"), b"legacy data").unwrap();
        let rows = [
            (1, 0, "/", "unknownfs"),
            (2, 1, root.to_str().unwrap(), "ext4"),
        ];
        let mounts = parse_mounts(&table(&rows, false)).unwrap();
        let locations = locations_with(&location.trash, &mounts).unwrap();
        assert!(
            locations.contains(&location),
            "legacy owned Trash must stay discoverable for explicit repair"
        );
        for dir in [
            location.trash.root(),
            &location.trash.files,
            &location.trash.info,
        ] {
            assert_eq!(
                fs::metadata(dir).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
        assert_eq!(
            fs::read(location.trash.files.join("old")).unwrap(),
            b"legacy data"
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn legacy_repair_cancellation_after_one_chmod_retains_partial_evidence() {
        let (root, location, _) = fixture();
        for dir in [
            location.trash.root(),
            &location.trash.files,
            &location.trash.info,
        ] {
            fs::create_dir_all(dir).unwrap();
            fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let cancel = CancelToken::new();
        AFTER_LEGACY_REPAIR.with(|hook| {
            let stop = cancel.clone();
            *hook.borrow_mut() = Some(Box::new(move || stop.cancel()));
        });
        let error = location.repair_legacy_permissions(&cancel).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(error
            .get_ref()
            .unwrap()
            .downcast_ref::<TrashPartial>()
            .is_some());
        assert_eq!(
            fs::metadata(&location.trash.files)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(location.trash.root())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        location
            .repair_legacy_permissions(&CancelToken::new())
            .unwrap();
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn legacy_repair_uses_original_descriptors_and_never_chmods_a_replacement() {
        let (root, location, _) = fixture();
        for dir in [
            location.trash.root(),
            &location.trash.files,
            &location.trash.info,
        ] {
            fs::create_dir_all(dir).unwrap();
            fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(location.trash.files.join("old"), b"keep").unwrap();
        let kept = root.join("retained-old-trash");
        BEFORE_LEGACY_REPAIR.with(|hook| {
            let path = location.trash.root().to_path_buf();
            let kept = kept.clone();
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::rename(&path, &kept).unwrap();
                for dir in [&path, &path.join("files"), &path.join("info")] {
                    fs::create_dir_all(dir).unwrap();
                    fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
                }
                fs::write(path.join("files/later"), b"later work").unwrap();
            }));
        });
        let error = location
            .repair_legacy_permissions(&CancelToken::new())
            .unwrap_err();
        assert!(error
            .get_ref()
            .unwrap()
            .downcast_ref::<TrashPartial>()
            .is_some());
        assert_eq!(fs::read(kept.join("files/old")).unwrap(), b"keep");
        assert_eq!(
            fs::read(location.trash.files.join("later")).unwrap(),
            b"later work"
        );
        for dir in [
            location.trash.root(),
            &location.trash.files,
            &location.trash.info,
        ] {
            assert_eq!(
                fs::metadata(dir).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
}
