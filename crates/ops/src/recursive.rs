//! Recursive copy and move, anchored to held directory descriptors.
//!
//! Every level of the walk is entered with `openat(..., O_NOFOLLOW)` from its
//! parent's descriptor, so a directory swapped mid-operation cannot redirect
//! the traversal (DESIGN.md §6). Nothing here resolves a path string twice.

use crate::anchor::{resolve_parent, DirHandle, FileIdentity};
use crate::engine::{classify, copy_regular_file_with, copy_symlink, FileClass, OpError, OpResult};
use crate::journal::ConflictPolicy;
use gust_core::lister::CancelToken;
use rustix::fs::Mode;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// Depth limit for a recursive operation. Deep enough for any real tree,
/// shallow enough that a pathological one cannot exhaust descriptors.
pub const MAX_DEPTH: usize = 64;

/// What a recursive walk found, before anything is written.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Plan {
    /// Directories to create, parents first.
    pub dirs: Vec<PathBuf>,
    /// Regular files to copy, with their sizes.
    pub files: Vec<(PathBuf, u64)>,
    /// Symlinks to recreate.
    pub symlinks: Vec<PathBuf>,
    /// Entries refused by policy, with the reason. Always shown, never hidden.
    pub refused: Vec<(PathBuf, String)>,
    pub total_bytes: u64,
}

impl Plan {
    pub fn entry_count(&self) -> usize {
        self.dirs.len() + self.files.len() + self.symlinks.len()
    }
}

/// Walk a source tree and decide what would be done — without writing.
///
/// Paths in the returned plan are RELATIVE to the source root, so the caller
/// can join them onto a destination without re-resolving the source.
pub fn plan_tree(src_root: &Path, cancel: &CancelToken) -> OpResult<Plan> {
    let mut plan = Plan::default();
    let (parent, name) = resolve_parent(src_root)?;
    let st = parent.stat_child(&name)?;

    match classify(&st) {
        FileClass::Regular => {
            plan.files.push((PathBuf::from(&name), st.st_size as u64));
            plan.total_bytes += st.st_size as u64;
            return Ok(plan);
        }
        FileClass::Symlink => {
            plan.symlinks.push(PathBuf::from(&name));
            return Ok(plan);
        }
        FileClass::Special => {
            return Err(OpError::Refused(
                "special file (FIFO/socket/device) is not copied".into(),
            ))
        }
        FileClass::Directory => {}
    }

    let root = parent.open_child_dir(&name)?;
    walk(&root, PathBuf::new(), 0, &mut plan, cancel)?;
    Ok(plan)
}

fn walk(
    dir: &DirHandle,
    rel: PathBuf,
    depth: usize,
    plan: &mut Plan,
    cancel: &CancelToken,
) -> OpResult<()> {
    if cancel.checkpoint() {
        return Err(OpError::Cancelled);
    }
    if depth >= MAX_DEPTH {
        plan.refused
            .push((rel, format!("deeper than {MAX_DEPTH} levels")));
        return Ok(());
    }

    for (name, st) in read_dir_at(dir)? {
        if cancel.checkpoint() {
            return Err(OpError::Cancelled);
        }
        let child_rel = rel.join(&name);
        match classify(&st) {
            FileClass::Directory => {
                plan.dirs.push(child_rel.clone());
                // NOFOLLOW at every level: this is the anchoring guarantee.
                match dir.open_child_dir(&name) {
                    Ok(sub) => walk(&sub, child_rel, depth + 1, plan, cancel)?,
                    Err(e) => plan.refused.push((child_rel, format!("{e}"))),
                }
            }
            FileClass::Regular => {
                let size = st.st_size as u64;
                plan.files.push((child_rel, size));
                plan.total_bytes = plan.total_bytes.saturating_add(size);
            }
            FileClass::Symlink => plan.symlinks.push(child_rel),
            FileClass::Special => plan.refused.push((
                child_rel,
                "special file (FIFO/socket/device) is not copied".into(),
            )),
        }
    }
    Ok(())
}

/// List a directory through its descriptor, without re-resolving its path.
fn read_dir_at(dir: &DirHandle) -> OpResult<Vec<(OsString, rustix::fs::Stat)>> {
    // Duplicate the fd: reading the directory consumes its offset, and the
    // handle is reused afterwards for openat.
    let dup = rustix::io::dup(dir.as_fd())?;
    let mut out = Vec::new();
    let iter = rustix::fs::Dir::read_from(&dup)?;
    for entry in iter {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_bytes() == b"." || name.to_bytes() == b".." {
            continue;
        }
        let name =
            OsString::from(std::os::unix::ffi::OsStrExt::from_bytes(name.to_bytes()) as &OsStr);
        match dir.stat_child(&name) {
            Ok(st) => out.push((name, st)),
            Err(_) => continue, // vanished mid-walk; skip rather than fail all
        }
    }
    Ok(out)
}

/// Progress callback: (entries done, entries total, bytes done, bytes total).
pub type ProgressFn<'a> = &'a mut dyn FnMut(usize, usize, u64, u64);

/// Copy a whole tree. Directories are created before their contents, and a
/// per-entry failure is recorded without aborting unrelated work.
pub fn copy_tree(
    src_root: &Path,
    dst_root: &Path,
    policy: ConflictPolicy,
    cancel: &CancelToken,
    progress: ProgressFn<'_>,
) -> OpResult<Plan> {
    copy_tree_opts(src_root, dst_root, policy, cancel, progress, false)
}

/// [`copy_tree`], optionally writing every name inside the tree under its
/// FAT-safe spelling (`fat::safe_rel`), for a FAT destination.
pub fn copy_tree_opts(
    src_root: &Path,
    dst_root: &Path,
    policy: ConflictPolicy,
    cancel: &CancelToken,
    progress: ProgressFn<'_>,
    fat_names: bool,
) -> OpResult<Plan> {
    let plan = plan_tree(src_root, cancel)?;
    let out = |rel: &Path| -> PathBuf {
        if fat_names {
            dst_root.join(crate::fat::safe_rel(rel))
        } else {
            dst_root.join(rel)
        }
    };

    let src_is_dir = {
        let (p, n) = resolve_parent(src_root)?;
        classify(&p.stat_child(&n)?) == FileClass::Directory
    };

    // KeepBoth means "never onto an existing name". The job picks a free
    // name before calling; an occupied root here is somebody else's.
    if policy == ConflictPolicy::KeepBoth {
        let (p, n) = resolve_parent(dst_root)?;
        if p.stat_child(&n).is_ok() {
            return Err(OpError::Conflict(dst_root.to_path_buf()));
        }
    }

    let total = plan.entry_count();
    let mut done = 0usize;
    let mut bytes_done = 0u64;
    progress(done, total, bytes_done, plan.total_bytes);

    if src_is_dir {
        // Create the destination root itself, then mirror the tree.
        create_dir_anchored(dst_root)?;
        for rel in &plan.dirs {
            if cancel.checkpoint() {
                return Err(OpError::Cancelled);
            }
            create_dir_anchored(&out(rel))?;
            done += 1;
            progress(done, total, bytes_done, plan.total_bytes);
        }
        for (rel, size) in &plan.files {
            if cancel.checkpoint() {
                return Err(OpError::Cancelled);
            }
            let (sd, sn) = resolve_parent(&src_root.join(rel))?;
            let (dd, dn) = resolve_parent(&out(rel))?;
            let before = bytes_done;
            copy_regular_file_with(&sd, &sn, &dd, &dn, policy, cancel, &mut |w| {
                progress(done, total, before.saturating_add(w), plan.total_bytes)
            })?;
            done += 1;
            bytes_done = bytes_done.saturating_add(*size);
            progress(done, total, bytes_done, plan.total_bytes);
        }
        for rel in &plan.symlinks {
            if cancel.checkpoint() {
                return Err(OpError::Cancelled);
            }
            let (sd, sn) = resolve_parent(&src_root.join(rel))?;
            let (dd, dn) = resolve_parent(&out(rel))?;
            copy_symlink(&sd, &sn, &dd, &dn)?;
            done += 1;
            progress(done, total, bytes_done, plan.total_bytes);
        }
    } else {
        // Single file or symlink.
        let (sd, sn) = resolve_parent(src_root)?;
        let (dd, dn) = resolve_parent(dst_root)?;
        let st = sd.stat_child(&sn)?;
        match classify(&st) {
            FileClass::Symlink => copy_symlink(&sd, &sn, &dd, &dn)?,
            _ => {
                copy_regular_file_with(&sd, &sn, &dd, &dn, policy, cancel, &mut |w| {
                    progress(0, total.max(1), w, plan.total_bytes)
                })?;
                bytes_done = st.st_size as u64;
            }
        }
        done = 1;
        progress(done, total.max(1), bytes_done, plan.total_bytes);
    }

    Ok(plan)
}

/// Create one directory through its parent's descriptor. Existing is fine.
fn create_dir_anchored(path: &Path) -> OpResult<()> {
    let (parent, name) = resolve_parent(path)?;
    match rustix::fs::mkdirat(parent.as_fd(), &name, Mode::from_bits_truncate(0o700)) {
        Ok(()) => Ok(()),
        Err(rustix::io::Errno::EXIST) => {
            // Must be a real directory, not a symlink someone swapped in.
            let st = parent.stat_child(&name)?;
            if classify(&st) == FileClass::Directory {
                Ok(())
            } else {
                Err(OpError::Refused(format!(
                    "{} exists and is not a directory",
                    path.display()
                )))
            }
        }
        Err(e) => Err(e.into()),
    }
}

/// Move a tree. Tries an atomic rename first; falls back to copy-verify-remove
/// across filesystems, and keeps the source whenever the copy is not provably
/// complete (DESIGN.md §6).
pub fn move_tree(
    src_root: &Path,
    dst_root: &Path,
    policy: ConflictPolicy,
    cancel: &CancelToken,
    progress: ProgressFn<'_>,
) -> OpResult<()> {
    move_tree_opts(src_root, dst_root, policy, cancel, progress, false)
}

/// [`move_tree`] with FAT-safe names inside the copied tree.
pub fn move_tree_opts(
    src_root: &Path,
    dst_root: &Path,
    policy: ConflictPolicy,
    cancel: &CancelToken,
    progress: ProgressFn<'_>,
    fat_names: bool,
) -> OpResult<()> {
    let (sd, sn) = resolve_parent(src_root)?;
    let (dd, dn) = resolve_parent(dst_root)?;

    // Refuse to move a directory into itself: the rename would succeed on some
    // filesystems and produce an unreachable subtree.
    if dst_root.starts_with(src_root) {
        return Err(OpError::Refused("cannot move a folder into itself".into()));
    }

    if dd.stat_child(&dn).is_ok() {
        match policy {
            ConflictPolicy::Skip => return Ok(()),
            ConflictPolicy::Ask | ConflictPolicy::KeepBoth => {
                return Err(OpError::Conflict(dst_root.to_path_buf()))
            }
            ConflictPolicy::Replace => {}
        }
    }

    match rustix::fs::renameat(sd.as_fd(), &sn, dd.as_fd(), &dn) {
        Ok(()) => {
            progress(1, 1, 0, 0);
            return Ok(());
        }
        // Only EXDEV justifies the slow path; anything else is a real error.
        Err(rustix::io::Errno::XDEV) => {}
        Err(e) => return Err(e.into()),
    }

    // Cross-device: record identity of the WHOLE subtree, copy, verify nothing
    // inside changed, and only then remove the source.
    //
    // The root's own identity is not a subtree fingerprint: editing a child
    // does not change the parent directory's mtime or inode, so a root-only
    // check happily deletes a source tree containing an edit made after that
    // child was copied. Every descendant must be proven unchanged.
    let before = FileIdentity::from_stat(&sd.stat_child(&sn)?);
    let before_tree = crate::undo::snapshot_subtree(src_root);
    let plan = copy_tree_opts(src_root, dst_root, policy, cancel, progress, fat_names)?;

    let after = FileIdentity::from_stat(&sd.stat_child(&sn)?);
    if !before.matches(&after) {
        return Err(OpError::Refused(
            "source changed during move; the copy was kept and the source was NOT removed".into(),
        ));
    }
    if let Some(reason) = crate::undo::verify_subtree_unchanged(src_root, &before_tree) {
        // The copy stays; the source is kept. Losing the newer bytes is never
        // the fallback.
        return Err(OpError::Refused(format!(
            "something inside changed during the move ({reason}); \
             the copy was kept and the source was NOT removed"
        )));
    }
    if !plan.refused.is_empty() {
        return Err(OpError::Refused(format!(
            "{} entries could not be copied; source kept",
            plan.refused.len()
        )));
    }
    rustix::fs::fsync(dd.as_fd())?;
    remove_tree(src_root)?;
    Ok(())
}

/// Remove a tree, depth-first, through held descriptors.
pub fn remove_tree(root: &Path) -> OpResult<()> {
    let (parent, name) = resolve_parent(root)?;
    let st = parent.stat_child(&name)?;
    if classify(&st) != FileClass::Directory {
        rustix::fs::unlinkat(parent.as_fd(), &name, rustix::fs::AtFlags::empty())?;
        return Ok(());
    }
    let dir = parent.open_child_dir(&name)?;
    remove_contents(&dir, 0)?;
    rustix::fs::unlinkat(parent.as_fd(), &name, rustix::fs::AtFlags::REMOVEDIR)?;
    Ok(())
}

fn remove_contents(dir: &DirHandle, depth: usize) -> OpResult<()> {
    if depth >= MAX_DEPTH {
        return Err(OpError::Refused("tree too deep to remove".into()));
    }
    for (name, st) in read_dir_at(dir)? {
        if classify(&st) == FileClass::Directory {
            let sub = dir.open_child_dir(&name)?;
            remove_contents(&sub, depth + 1)?;
            rustix::fs::unlinkat(dir.as_fd(), &name, rustix::fs::AtFlags::REMOVEDIR)?;
        } else {
            rustix::fs::unlinkat(dir.as_fd(), &name, rustix::fs::AtFlags::empty())?;
        }
    }
    Ok(())
}

/// Rename within one directory. Atomic, and refuses to clobber by default.
pub fn rename_in_place(path: &Path, new_name: &OsStr, allow_replace: bool) -> OpResult<PathBuf> {
    validate_name(new_name)?;
    let (parent, old_name) = resolve_parent(path)?;
    if old_name == new_name {
        return Ok(path.to_path_buf());
    }
    if !allow_replace && parent.stat_child(new_name).is_ok() {
        return Err(OpError::Conflict(PathBuf::from(new_name)));
    }
    rustix::fs::renameat(parent.as_fd(), &old_name, parent.as_fd(), new_name)?;
    Ok(path.with_file_name(new_name))
}

/// Reject names the filesystem or the UI cannot represent safely.
pub fn validate_name(name: &OsStr) -> OpResult<()> {
    use std::os::unix::ffi::OsStrExt;
    let b = name.as_bytes();
    if b.is_empty() {
        return Err(OpError::Refused("name cannot be empty".into()));
    }
    if b == b"." || b == b".." {
        return Err(OpError::Refused("name cannot be '.' or '..'".into()));
    }
    if b.contains(&b'/') {
        return Err(OpError::Refused("name cannot contain '/'".into()));
    }
    if b.contains(&0) {
        return Err(OpError::Refused("name cannot contain a null byte".into()));
    }
    if b.len() > 255 {
        return Err(OpError::Refused("name is longer than 255 bytes".into()));
    }
    Ok(())
}

/// Pick a free "copy" name in `dir`, e.g. `notes.txt` → `notes (copy).txt`.
pub fn unique_copy_name(dir: &DirHandle, base: &OsStr) -> OpResult<OsString> {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let b = base.as_bytes();
    // Split on the last dot that is not a leading dot.
    let (stem, ext): (&[u8], &[u8]) = match b.iter().rposition(|c| *c == b'.') {
        Some(p) if p > 0 => (&b[..p], &b[p..]),
        _ => (b, b""),
    };
    for n in 1..10_000 {
        let mut cand = stem.to_vec();
        if n == 1 {
            cand.extend_from_slice(b" (copy)");
        } else {
            cand.extend_from_slice(format!(" (copy {n})").as_bytes());
        }
        cand.extend_from_slice(ext);
        let name = OsString::from_vec(cand);
        if dir.stat_child(&name).is_err() {
            return Ok(name);
        }
    }
    Err(OpError::Refused("cannot find a free name".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gust-rec-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn noop(_: usize, _: usize, _: u64, _: u64) {}

    #[test]
    fn plans_a_tree_without_writing_anything() {
        let d = tmp("plan");
        fs::create_dir_all(d.join("src/a/b")).unwrap();
        fs::write(d.join("src/one.txt"), b"12345").unwrap();
        fs::write(d.join("src/a/two.txt"), b"67").unwrap();
        std::os::unix::fs::symlink("one.txt", d.join("src/link")).unwrap();

        let plan = plan_tree(&d.join("src"), &CancelToken::new()).unwrap();
        assert_eq!(plan.dirs.len(), 2, "a and a/b");
        assert_eq!(plan.files.len(), 2);
        assert_eq!(plan.symlinks.len(), 1);
        assert_eq!(plan.total_bytes, 7);
        assert!(plan.refused.is_empty());
        // Nothing was created.
        assert!(!d.join("dst").exists());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn copies_a_whole_tree_with_contents() {
        let d = tmp("copytree");
        fs::create_dir_all(d.join("src/a/b")).unwrap();
        fs::write(d.join("src/one.txt"), b"hello").unwrap();
        fs::write(d.join("src/a/b/deep.txt"), b"deep").unwrap();
        std::os::unix::fs::symlink("one.txt", d.join("src/link")).unwrap();

        copy_tree(
            &d.join("src"),
            &d.join("dst"),
            ConflictPolicy::Replace,
            &CancelToken::new(),
            &mut noop,
        )
        .unwrap();

        assert_eq!(fs::read(d.join("dst/one.txt")).unwrap(), b"hello");
        assert_eq!(fs::read(d.join("dst/a/b/deep.txt")).unwrap(), b"deep");
        assert!(fs::symlink_metadata(d.join("dst/link"))
            .unwrap()
            .file_type()
            .is_symlink());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn copy_reports_progress_monotonically() {
        let d = tmp("progress");
        fs::create_dir_all(d.join("src")).unwrap();
        for i in 0..5 {
            fs::write(d.join(format!("src/f{i}")), vec![b'x'; 100]).unwrap();
        }
        let mut seen: Vec<(usize, usize)> = Vec::new();
        let mut cb = |done: usize, total: usize, _b: u64, _t: u64| seen.push((done, total));
        copy_tree(
            &d.join("src"),
            &d.join("dst"),
            ConflictPolicy::Replace,
            &CancelToken::new(),
            &mut cb,
        )
        .unwrap();
        assert!(seen.len() >= 5);
        assert!(
            seen.windows(2).all(|w| w[0].0 <= w[1].0),
            "progress must never go backwards: {seen:?}"
        );
        assert_eq!(seen.last().unwrap().0, 5);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn special_files_are_refused_not_copied() {
        let d = tmp("special");
        fs::create_dir_all(d.join("src")).unwrap();
        fs::write(d.join("src/ok.txt"), b"fine").unwrap();
        rustix::fs::mknodat(
            rustix::fs::CWD,
            d.join("src/pipe"),
            rustix::fs::FileType::Fifo,
            Mode::from_bits_truncate(0o600),
            0,
        )
        .unwrap();

        let plan = copy_tree(
            &d.join("src"),
            &d.join("dst"),
            ConflictPolicy::Replace,
            &CancelToken::new(),
            &mut noop,
        )
        .unwrap();

        assert_eq!(plan.refused.len(), 1, "the FIFO is reported, not silent");
        assert!(plan.refused[0].0.to_string_lossy().contains("pipe"));
        // The good file still arrived; the FIFO did not.
        assert_eq!(fs::read(d.join("dst/ok.txt")).unwrap(), b"fine");
        assert!(!d.join("dst/pipe").exists());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn cancelled_copy_stops_and_reports() {
        let d = tmp("cancel");
        fs::create_dir_all(d.join("src")).unwrap();
        for i in 0..20 {
            fs::write(d.join(format!("src/f{i}")), vec![b'x'; 1000]).unwrap();
        }
        let tok = CancelToken::new();
        tok.cancel();
        let err = copy_tree(
            &d.join("src"),
            &d.join("dst"),
            ConflictPolicy::Replace,
            &tok,
            &mut noop,
        )
        .unwrap_err();
        assert!(matches!(err, OpError::Cancelled));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn same_filesystem_move_is_a_rename() {
        let d = tmp("mvsame");
        fs::create_dir_all(d.join("src/sub")).unwrap();
        fs::write(d.join("src/sub/f.txt"), b"data").unwrap();
        move_tree(
            &d.join("src"),
            &d.join("dst"),
            ConflictPolicy::Replace,
            &CancelToken::new(),
            &mut noop,
        )
        .unwrap();
        assert!(!d.join("src").exists());
        assert_eq!(fs::read(d.join("dst/sub/f.txt")).unwrap(), b"data");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn move_refuses_to_put_a_folder_inside_itself() {
        let d = tmp("selfmove");
        fs::create_dir_all(d.join("src/sub")).unwrap();
        let err = move_tree(
            &d.join("src"),
            &d.join("src/sub/copy"),
            ConflictPolicy::Replace,
            &CancelToken::new(),
            &mut noop,
        )
        .unwrap_err();
        assert!(matches!(err, OpError::Refused(_)), "got {err:?}");
        assert!(d.join("src/sub").exists(), "nothing was moved");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn move_with_ask_policy_reports_conflict_and_changes_nothing() {
        let d = tmp("mvconflict");
        fs::create_dir_all(d.join("src")).unwrap();
        fs::write(d.join("src/f"), b"new").unwrap();
        fs::create_dir_all(d.join("dst")).unwrap();
        fs::write(d.join("dst/keep"), b"ORIGINAL").unwrap();

        let err = move_tree(
            &d.join("src"),
            &d.join("dst"),
            ConflictPolicy::Ask,
            &CancelToken::new(),
            &mut noop,
        )
        .unwrap_err();
        assert!(matches!(err, OpError::Conflict(_)));
        assert_eq!(fs::read(d.join("dst/keep")).unwrap(), b"ORIGINAL");
        assert!(d.join("src/f").exists(), "source untouched");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn rename_is_atomic_and_refuses_to_clobber() {
        let d = tmp("rename");
        fs::write(d.join("a.txt"), b"A").unwrap();
        fs::write(d.join("b.txt"), b"B").unwrap();

        let err = rename_in_place(&d.join("a.txt"), OsStr::new("b.txt"), false).unwrap_err();
        assert!(matches!(err, OpError::Conflict(_)));
        assert_eq!(fs::read(d.join("b.txt")).unwrap(), b"B", "not clobbered");

        let out = rename_in_place(&d.join("a.txt"), OsStr::new("c.txt"), false).unwrap();
        assert_eq!(out, d.join("c.txt"));
        assert_eq!(fs::read(d.join("c.txt")).unwrap(), b"A");
        assert!(!d.join("a.txt").exists());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn rename_to_the_same_name_is_a_no_op() {
        let d = tmp("samename");
        fs::write(d.join("a.txt"), b"A").unwrap();
        let out = rename_in_place(&d.join("a.txt"), OsStr::new("a.txt"), false).unwrap();
        assert_eq!(out, d.join("a.txt"));
        assert_eq!(fs::read(d.join("a.txt")).unwrap(), b"A");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn invalid_names_are_rejected() {
        assert!(validate_name(OsStr::new("")).is_err());
        assert!(validate_name(OsStr::new(".")).is_err());
        assert!(validate_name(OsStr::new("..")).is_err());
        assert!(validate_name(OsStr::new("a/b")).is_err());
        assert!(validate_name(OsStr::new(&"x".repeat(256))).is_err());
        assert!(validate_name(OsStr::new("perfectly fine.txt")).is_ok());
        // A name that is only unusual must still be allowed.
        assert!(validate_name(OsStr::new("..hidden")).is_ok());
    }

    #[test]
    fn copy_name_avoids_collisions_and_keeps_the_extension() {
        let d = tmp("copyname");
        fs::write(d.join("notes.txt"), b"x").unwrap();
        let (dir, _) = resolve_parent(&d.join("anything")).unwrap();

        let first = unique_copy_name(&dir, OsStr::new("notes.txt")).unwrap();
        assert_eq!(first, OsStr::new("notes (copy).txt"));

        fs::write(d.join("notes (copy).txt"), b"x").unwrap();
        let second = unique_copy_name(&dir, OsStr::new("notes.txt")).unwrap();
        assert_eq!(second, OsStr::new("notes (copy 2).txt"));

        // A dotfile has no extension to preserve.
        let dotted = unique_copy_name(&dir, OsStr::new(".bashrc")).unwrap();
        assert_eq!(dotted, OsStr::new(".bashrc (copy)"));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn remove_tree_deletes_everything_it_owns() {
        let d = tmp("rm");
        fs::create_dir_all(d.join("t/a/b")).unwrap();
        fs::write(d.join("t/a/b/f"), b"x").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", d.join("t/a/link")).unwrap();
        remove_tree(&d.join("t")).unwrap();
        assert!(!d.join("t").exists());
        // The symlink target must be untouched.
        assert!(Path::new("/etc/passwd").exists());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn walk_does_not_follow_symlinked_directories() {
        // A symlink to a parent would loop forever if followed.
        let d = tmp("symloop");
        fs::create_dir_all(d.join("src/real")).unwrap();
        fs::write(d.join("src/real/f"), b"x").unwrap();
        std::os::unix::fs::symlink(d.join("src"), d.join("src/real/loop")).unwrap();

        let plan = plan_tree(&d.join("src"), &CancelToken::new()).unwrap();
        // "loop" is recorded as a symlink, never descended into.
        assert_eq!(plan.symlinks.len(), 1);
        assert_eq!(plan.files.len(), 1);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn copying_into_an_existing_non_directory_is_refused() {
        let d = tmp("notdir");
        fs::create_dir_all(d.join("src")).unwrap();
        fs::write(d.join("src/f"), b"x").unwrap();
        // Destination path already exists as a FILE.
        fs::write(d.join("dst"), b"i am a file").unwrap();
        let err = copy_tree(
            &d.join("src"),
            &d.join("dst"),
            ConflictPolicy::Replace,
            &CancelToken::new(),
            &mut noop,
        )
        .unwrap_err();
        assert!(matches!(err, OpError::Refused(_)), "got {err:?}");
        assert_eq!(fs::read(d.join("dst")).unwrap(), b"i am a file");
        fs::remove_dir_all(&d).ok();
    }
}
