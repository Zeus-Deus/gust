//! Links and permissions: small changes, each with a verified undo.
//!
//! Undo follows the repo rule: it re-reads the target's identity
//! (dev + inode + mtime) and refuses if anything changed since Gust made the
//! change. A link that was replaced by a real file, or a file whose mode
//! someone else changed again, is left alone and the refusal says why.
//!
//! Permission changes never set or clear setuid, setgid or sticky: those
//! bits turn a file into a privilege boundary, and a file manager toggle is
//! the wrong place to cross one (see DECISIONS).

use std::ffi::OsStr;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// dev + inode + mtime of a path, not following a final symlink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ident {
    pub dev: u64,
    pub ino: u64,
    pub mtime: i64,
    pub mtime_nsec: i64,
}

/// dev + inode + mtime of `p` (not following a final link), for undo checks.
pub fn ident_of(p: &Path) -> std::io::Result<Ident> {
    ident(p)
}

fn ident(p: &Path) -> std::io::Result<Ident> {
    let m = p.symlink_metadata()?;
    Ok(Ident {
        dev: m.dev(),
        ino: m.ino(),
        mtime: m.mtime(),
        mtime_nsec: m.mtime_nsec(),
    })
}

// ---------------------------------------------------------------- rename

/// A single rename and the exact object whose original name can be restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameMade {
    pub original: PathBuf,
    pub renamed: PathBuf,
    identity: crate::anchor::FileIdentity,
    parent_dev: u64,
    parent_ino: u64,
}

#[cfg(test)]
type RenameHook = std::cell::RefCell<Option<Box<dyn FnOnce()>>>;

#[cfg(test)]
thread_local! {
    // Scheduling only: reads, writes and renames still use production I/O.
    static RENAME_UNDO_AFTER_CHECK: RenameHook = const { std::cell::RefCell::new(None) };
    static RENAME_AFTER_COMMIT: RenameHook = const { std::cell::RefCell::new(None) };
}

/// Rename one entry without replacing anything, retaining a verified inverse.
pub fn rename_recorded(
    path: &Path,
    new_name: &OsStr,
) -> Result<RenameMade, crate::engine::OpError> {
    use crate::anchor::{resolve_parent, FileIdentity};
    crate::recursive::validate_name(new_name)?;
    let (parent, old_name) = resolve_parent(path)?;
    if old_name == new_name {
        return Err(crate::engine::OpError::Refused(
            "the name has not changed".into(),
        ));
    }
    let identity = FileIdentity::from_stat(&parent.stat_child(&old_name)?);
    let parent_identity = FileIdentity::from_stat(&parent.stat_self()?);
    rename_verified(&parent, &old_name, new_name, &identity)?;
    Ok(RenameMade {
        original: path.to_path_buf(),
        renamed: path.with_file_name(new_name),
        identity,
        parent_dev: parent_identity.dev,
        parent_ino: parent_identity.ino,
    })
}

/// Restore a name only while the parent and renamed object are still ours.
/// A refused inverse never consumes the caller's record.
pub fn undo_rename(made: &RenameMade) -> Result<(), String> {
    use crate::anchor::{resolve_parent, FileIdentity};
    let (parent, name) = resolve_parent(&made.renamed)
        .map_err(|_| "the renamed file's folder is no longer available".to_string())?;
    let parent_identity = parent
        .stat_self()
        .map_err(|e| crate::engine::plain_io_message(&e))?;
    if (parent_identity.st_dev, parent_identity.st_ino) != (made.parent_dev, made.parent_ino) {
        return Err("the folder was replaced since; left as it is".into());
    }
    let now = parent
        .stat_child(&name)
        .map(|st| FileIdentity::from_stat(&st))
        .map_err(|_| "the renamed item is already gone".to_string())?;
    if !made.identity.matches(&now) {
        return Err("the renamed item was changed or replaced since; left as it is".into());
    }
    let Some(original_name) = made.original.file_name() else {
        return Err("the original name is unavailable".into());
    };
    rename_verified(&parent, &name, original_name, &made.identity).map_err(|e| match e {
        crate::engine::OpError::Conflict(_) => {
            "the original name is occupied again; both items were kept".into()
        }
        other => other.to_string(),
    })
}

/// Verify what the kernel actually moved, not only the source stat sampled
/// before it. NOREPLACE is destination-atomic, not source-identity-conditional.
/// A racing replacement is refused and restored without clobbering new work.
fn rename_verified(
    parent: &crate::anchor::DirHandle,
    from: &OsStr,
    to: &OsStr,
    expected: &crate::anchor::FileIdentity,
) -> Result<(), crate::engine::OpError> {
    #[cfg(test)]
    RENAME_UNDO_AFTER_CHECK.with(|hook| {
        if let Some(run) = hook.borrow_mut().take() {
            run();
        }
    });
    rename_no_replace(parent, from, to)?;
    #[cfg(test)]
    RENAME_AFTER_COMMIT.with(|hook| {
        if let Some(run) = hook.borrow_mut().take() {
            run();
        }
    });
    if parent
        .stat_child(to)
        .is_ok_and(|st| expected.matches(&crate::anchor::FileIdentity::from_stat(&st)))
    {
        return Ok(());
    }
    let message = if rename_no_replace(parent, to, from).is_ok() {
        "the item changed during the rename; its previous name was restored"
    } else {
        "the item changed during the rename and its previous name could not be restored. Check both names before trying again. No existing name was replaced."
    };
    Err(crate::engine::OpError::Refused(message.into()))
}

/// Check name occupancy and commit atomically, including for folders.
fn rename_no_replace(
    parent: &crate::anchor::DirHandle,
    from: &OsStr,
    to: &OsStr,
) -> Result<(), crate::engine::OpError> {
    use rustix::io::Errno;
    match rustix::fs::renameat_with(
        parent.as_fd(),
        from,
        parent.as_fd(),
        to,
        rustix::fs::RenameFlags::NOREPLACE,
    ) {
        Ok(()) => Ok(()),
        Err(Errno::EXIST) | Err(Errno::NOTEMPTY) => {
            Err(crate::engine::OpError::Conflict(PathBuf::from(to)))
        }
        Err(Errno::INVAL) | Err(Errno::NOSYS) | Err(Errno::OPNOTSUPP) => {
            Err(crate::engine::OpError::Refused(
                "This filesystem cannot rename safely without replacing an existing name".into(),
            ))
        }
        Err(e) => Err(crate::engine::OpError::Io(e.into())),
    }
}

// ---------------------------------------------------------------- links

/// A link Gust made, and what it needs to take it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkMade {
    pub link: PathBuf,
    pub target: PathBuf,
    pub ident: Ident,
}

/// The name a new link to `target` gets in `dir`: "name (link)", then
/// "name (link 2)"… Never an existing name.
pub fn link_name_for(dir: &Path, target: &Path) -> std::ffi::OsString {
    let base = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "link".into());
    for i in 1..1000 {
        let cand = if i == 1 {
            format!("{base} (link)")
        } else {
            format!("{base} (link {i})")
        };
        if dir.join(&cand).symlink_metadata().is_err() {
            return cand.into();
        }
    }
    format!("{base} (link {})", std::process::id()).into()
}

/// Make a symbolic link `dir/name` pointing at `target` (absolute). Never
/// replaces anything: `symlink(2)` fails on an existing name.
pub fn create_link(target: &Path, dir: &Path, name: &OsStr) -> Result<LinkMade, String> {
    crate::recursive::validate_name(name).map_err(|e| format!("{e}"))?;
    if !target.is_absolute() {
        return Err("a link needs a full path to point at".into());
    }
    let link = dir.join(name);
    match std::os::unix::fs::symlink(target, &link) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err("something with that name already exists".into())
        }
        Err(e) => return Err(crate::engine::plain_io_message(&e)),
    }
    let ident = ident(&link).map_err(|e| crate::engine::plain_io_message(&e))?;
    Ok(LinkMade {
        link,
        target: target.to_path_buf(),
        ident,
    })
}

/// Remove a link Gust made, only if it is still that very link.
pub fn undo_link(made: &LinkMade) -> Result<(), String> {
    let now = ident(&made.link).map_err(|_| "the link is already gone".to_string())?;
    if now != made.ident {
        return Err("the link was changed or replaced since; left as it is".into());
    }
    match std::fs::read_link(&made.link) {
        Ok(t) if t == made.target => {}
        _ => return Err("the link now points somewhere else; left as it is".into()),
    }
    std::fs::remove_file(&made.link).map_err(|e| crate::engine::plain_io_message(&e))
}

// ---------------------------------------------------------- permissions

/// Only the nine rwx bits. Special bits are neither set nor cleared.
const RWX: u32 = 0o777;
const SPECIAL: u32 = 0o7000;

/// A requested change: either an exact mode, or bits to add and remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeSpec {
    Exact(u32),
    Change { add: u32, remove: u32 },
}

impl ModeSpec {
    /// "644", "0755", "u+x", "go-w", "a+r,u+w", "+x" (= a+x).
    pub fn parse(s: &str) -> Result<ModeSpec, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("type a mode such as 644 or u+x".into());
        }
        if s.chars().all(|c| c.is_ascii_digit()) {
            let v = u32::from_str_radix(s, 8)
                .map_err(|_| "a number mode uses only the digits 0–7".to_string())?;
            if v & SPECIAL != 0 || v > 0o7777 {
                return Err("setuid, setgid and sticky are not changed from here".into());
            }
            return Ok(ModeSpec::Exact(v & RWX));
        }
        let (mut add, mut remove) = (0u32, 0u32);
        for clause in s.split(',') {
            let pos = clause
                .find(['+', '-'])
                .ok_or_else(|| format!("'{clause}' needs a + or -"))?;
            let (who, rest) = clause.split_at(pos);
            let op = &rest[..1];
            let perms = &rest[1..];
            let mut who_mask = 0u32;
            let who = if who.is_empty() { "a" } else { who };
            for c in who.chars() {
                who_mask |= match c {
                    'u' => 0o700,
                    'g' => 0o070,
                    'o' => 0o007,
                    'a' => 0o777,
                    _ => return Err(format!("'{c}' is not u, g, o or a")),
                };
            }
            let mut bits = 0u32;
            for c in perms.chars() {
                bits |= match c {
                    'r' => 0o444,
                    'w' => 0o222,
                    'x' => 0o111,
                    's' | 't' => {
                        return Err("setuid, setgid and sticky are not changed from here".into())
                    }
                    _ => return Err(format!("'{c}' is not r, w or x")),
                };
            }
            if bits == 0 {
                return Err(format!("'{clause}' changes nothing"));
            }
            if op == "+" {
                add |= bits & who_mask;
                remove &= !(bits & who_mask);
            } else {
                remove |= bits & who_mask;
                add &= !(bits & who_mask);
            }
        }
        Ok(ModeSpec::Change { add, remove })
    }

    /// The mode `old` becomes. Special bits of `old` are kept as they are.
    pub fn apply(self, old: u32) -> u32 {
        let keep = old & !RWX;
        match self {
            ModeSpec::Exact(m) => keep | (m & RWX),
            ModeSpec::Change { add, remove } => keep | (((old & RWX) | add) & !remove),
        }
    }
}

/// One file's mode change, as done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeChanged {
    pub path: PathBuf,
    pub old: u32,
    pub new: u32,
    pub ident: Ident,
}

/// Outcome of a permissions change over several files.
#[derive(Debug, Default, Clone)]
pub struct ModeOutcome {
    pub changed: Vec<ModeChanged>,
    /// Already had that mode: nothing to do, nothing to undo.
    pub unchanged: usize,
    pub failed: Vec<String>,
}

/// Apply `spec` to each path. Symlinks are skipped (chmod would follow
/// them and change the file they point at, which may not be here at all).
pub fn set_modes(paths: &[PathBuf], spec: ModeSpec) -> ModeOutcome {
    let mut out = ModeOutcome::default();
    for p in paths {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let md = match p.symlink_metadata() {
            Ok(m) => m,
            Err(_) => {
                out.failed.push(format!("{name}: no longer there"));
                continue;
            }
        };
        if md.file_type().is_symlink() {
            out.failed
                .push(format!("{name}: a link has no permissions of its own"));
            continue;
        }
        let old = md.mode() & 0o7777;
        let new = spec.apply(old);
        if new == old {
            out.unchanged += 1;
            continue;
        }
        if let Err(e) = std::fs::set_permissions(p, std::fs::Permissions::from_mode(new)) {
            out.failed
                .push(format!("{name}: {}", crate::engine::plain_io_message(&e)));
            continue;
        }
        match ident(p) {
            Ok(ident) => out.changed.push(ModeChanged {
                path: p.clone(),
                old,
                new,
                ident,
            }),
            Err(e) => out
                .failed
                .push(format!("{name}: {}", crate::engine::plain_io_message(&e))),
        }
    }
    out
}

/// Put each mode back, file by file, only where the file is the same one
/// and still has the mode Gust gave it. Returns (restored, refusals).
pub fn undo_modes(changes: &[ModeChanged]) -> (usize, Vec<String>) {
    let mut back = 0;
    let mut refused = Vec::new();
    for c in changes {
        let name = c
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (now, md) = match (ident(&c.path), c.path.symlink_metadata()) {
            (Ok(i), Ok(m)) => (i, m),
            _ => {
                refused.push(format!("{name}: no longer there"));
                continue;
            }
        };
        if now != c.ident {
            refused.push(format!("{name}: changed since; left as it is"));
            continue;
        }
        if md.mode() & 0o7777 != c.new {
            refused.push(format!(
                "{name}: its permissions were changed again; left as they are"
            ));
            continue;
        }
        match std::fs::set_permissions(&c.path, std::fs::Permissions::from_mode(c.old)) {
            Ok(()) => back += 1,
            Err(e) => refused.push(format!("{name}: {}", crate::engine::plain_io_message(&e))),
        }
    }
    (back, refused)
}

// ------------------------------------------------------------- ownership

/// A requested owner change: `user`, `user:group` or `:group`, by name or
/// number. Names are looked up in `/etc/passwd` and `/etc/group` directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerSpec {
    pub uid: Option<u32>,
    pub gid: Option<u32>,
}

fn id_from(db: &str, name: &str) -> Option<u32> {
    if let Ok(n) = name.parse::<u32>() {
        return Some(n);
    }
    db.lines().find_map(|l| {
        let mut f = l.split(':');
        let n = f.next()?;
        let _pw = f.next()?;
        let id: u32 = f.next()?.parse().ok()?;
        (n == name).then_some(id)
    })
}

impl OwnerSpec {
    /// Parse against the given passwd/group text (tests pass fixtures).
    pub fn parse_with(s: &str, passwd: &str, group: &str) -> Result<OwnerSpec, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("type an owner (user), a group (:group) or both (user:group)".into());
        }
        let (u, g) = match s.split_once(':') {
            Some((u, g)) => (u.trim(), Some(g.trim())),
            None => (s, None),
        };
        let uid = if u.is_empty() {
            None
        } else {
            Some(id_from(passwd, u).ok_or_else(|| format!("there is no user called {u}"))?)
        };
        let gid = match g {
            None => None,
            Some("") => return Err("after the colon, name a group".into()),
            Some(g) => {
                Some(id_from(group, g).ok_or_else(|| format!("there is no group called {g}"))?)
            }
        };
        if uid.is_none() && gid.is_none() {
            return Err("name a user, a group, or both".into());
        }
        Ok(OwnerSpec { uid, gid })
    }

    pub fn parse(s: &str) -> Result<OwnerSpec, String> {
        let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
        let group = std::fs::read_to_string("/etc/group").unwrap_or_default();
        Self::parse_with(s, &passwd, &group)
    }
}

/// One ownership change, and what undo needs to take it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerChanged {
    pub path: PathBuf,
    pub old: (u32, u32),
    pub new: (u32, u32),
    pub ident: Ident,
}

#[derive(Debug, Default, Clone)]
pub struct OwnerOutcome {
    pub changed: Vec<OwnerChanged>,
    pub unchanged: usize,
    pub failed: Vec<String>,
}

fn chown_nofollow(p: &Path, uid: u32, gid: u32) -> std::io::Result<()> {
    use rustix::fs::{chownat, AtFlags, CWD};
    use rustix::process::{Gid, Uid};
    chownat(
        CWD,
        p,
        Some(Uid::from_raw(uid)),
        Some(Gid::from_raw(gid)),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(std::io::Error::from)
}

/// Change owner and/or group of each path. Links are skipped (their own
/// owner is meaningless for access), and a file with setuid/setgid is
/// refused: the kernel clears those bits on a chown, which would change a
/// privilege boundary as a side effect (same rule as D19). Without admin
/// rights the kernel allows only a group you belong to; anything else comes
/// back as a plain refusal, per file.
pub fn set_owners(paths: &[PathBuf], spec: OwnerSpec) -> OwnerOutcome {
    let mut out = OwnerOutcome::default();
    for p in paths {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let md = match p.symlink_metadata() {
            Ok(m) => m,
            Err(_) => {
                out.failed.push(format!("{name}: no longer there"));
                continue;
            }
        };
        if md.file_type().is_symlink() {
            out.failed
                .push(format!("{name}: a link has no owner of its own to change"));
            continue;
        }
        if md.is_file() && md.mode() & 0o6000 != 0 {
            out.failed.push(format!(
                "{name}: has setuid/setgid, which a change of owner would remove; left as it is"
            ));
            continue;
        }
        let old = (md.uid(), md.gid());
        let new = (spec.uid.unwrap_or(old.0), spec.gid.unwrap_or(old.1));
        if new == old {
            out.unchanged += 1;
            continue;
        }
        if let Err(e) = chown_nofollow(p, new.0, new.1) {
            let why = if e.kind() == std::io::ErrorKind::PermissionDenied {
                if new.0 != old.0 {
                    "changing the owner needs administrator rights (Open as administrator)"
                        .to_string()
                } else {
                    "you can only give it a group you belong to".to_string()
                }
            } else {
                crate::engine::plain_io_message(&e)
            };
            out.failed.push(format!("{name}: {why}"));
            continue;
        }
        match ident(p) {
            Ok(ident) => out.changed.push(OwnerChanged {
                path: p.clone(),
                old,
                new,
                ident,
            }),
            Err(e) => out
                .failed
                .push(format!("{name}: {}", crate::engine::plain_io_message(&e))),
        }
    }
    out
}

/// Put each owner back, only where the file is the same one and still has
/// the owner Gust gave it. Returns (restored, refusals).
pub fn undo_owners(changes: &[OwnerChanged]) -> (usize, Vec<String>) {
    let mut back = 0;
    let mut refused = Vec::new();
    for c in changes {
        let name = c
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (now, md) = match (ident(&c.path), c.path.symlink_metadata()) {
            (Ok(i), Ok(m)) => (i, m),
            _ => {
                refused.push(format!("{name}: no longer there"));
                continue;
            }
        };
        if now != c.ident {
            refused.push(format!("{name}: changed since; left as it is"));
            continue;
        }
        if (md.uid(), md.gid()) != c.new {
            refused.push(format!(
                "{name}: its owner was changed again; left as it is"
            ));
            continue;
        }
        match chown_nofollow(&c.path, c.old.0, c.old.1) {
            Ok(()) => back += 1,
            Err(e) => refused.push(format!("{name}: {}", crate::engine::plain_io_message(&e))),
        }
    }
    (back, refused)
}

/// "rwxr-xr-x" for the nine bits.
pub fn rwx_string(mode: u32) -> String {
    let mut s = String::with_capacity(9);
    for shift in [6, 3, 0] {
        let b = (mode >> shift) & 7;
        s.push(if b & 4 != 0 { 'r' } else { '-' });
        s.push(if b & 2 != 0 { 'w' } else { '-' });
        s.push(if b & 1 != 0 { 'x' } else { '-' });
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gust-attrs-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }
    fn mode(p: &Path) -> u32 {
        p.symlink_metadata().unwrap().mode() & 0o7777
    }

    #[test]
    fn a_single_rename_is_undone_with_its_contents() {
        let d = tmp("rename-undo");
        fs::write(d.join("before.txt"), b"original").unwrap();
        let made = rename_recorded(&d.join("before.txt"), OsStr::new("after.txt")).unwrap();
        assert!(!made.original.exists());
        undo_rename(&made).unwrap();
        assert_eq!(fs::read(&made.original).unwrap(), b"original");
        assert!(!made.renamed.exists());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn rename_undo_refuses_a_source_replaced_between_check_and_commit() {
        let d = tmp("rename-commit-race");
        fs::write(d.join("before"), b"original identity").unwrap();
        let made = rename_recorded(&d.join("before"), OsStr::new("after")).unwrap();
        let root = d.clone();
        RENAME_UNDO_AFTER_CHECK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::rename(root.join("after"), root.join("saved-original")).unwrap();
                fs::write(root.join("after"), b"replacement writer's bytes").unwrap();
            }));
        });
        let result = undo_rename(&made);
        assert!(
            result.is_err(),
            "a replaced source must not be reported as successfully undone: {result:?}"
        );
        assert!(
            !made.original.exists(),
            "Undo must recover the replacement's name when it is free"
        );
        assert_eq!(
            fs::read(&made.renamed).unwrap(),
            b"replacement writer's bytes"
        );
        assert_eq!(
            fs::read(d.join("saved-original")).unwrap(),
            b"original identity"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_forward_rename_refuses_a_source_replaced_at_commit() {
        let d = tmp("rename-forward-race");
        fs::write(d.join("before"), b"original identity").unwrap();
        let root = d.clone();
        RENAME_UNDO_AFTER_CHECK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::rename(root.join("before"), root.join("saved-original")).unwrap();
                fs::write(root.join("before"), b"replacement writer's bytes").unwrap();
            }));
        });
        assert!(rename_recorded(&d.join("before"), OsStr::new("after")).is_err());
        assert!(!d.join("after").exists());
        assert_eq!(
            fs::read(d.join("before")).unwrap(),
            b"replacement writer's bytes"
        );
        assert_eq!(
            fs::read(d.join("saved-original")).unwrap(),
            b"original identity"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn rename_race_recovery_never_clobbers_a_newly_occupied_name() {
        let d = tmp("rename-rollback-occupied");
        fs::write(d.join("before"), b"original identity").unwrap();
        let made = rename_recorded(&d.join("before"), OsStr::new("after")).unwrap();
        let root = d.clone();
        RENAME_UNDO_AFTER_CHECK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::rename(root.join("after"), root.join("saved-original")).unwrap();
                fs::write(root.join("after"), b"replacement writer's bytes").unwrap();
            }));
        });
        let root = d.clone();
        RENAME_AFTER_COMMIT.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::write(root.join("after"), b"new work that blocks recovery").unwrap();
            }));
        });
        assert!(undo_rename(&made)
            .unwrap_err()
            .contains("could not be restored"));
        assert_eq!(
            fs::read(&made.original).unwrap(),
            b"replacement writer's bytes"
        );
        assert_eq!(
            fs::read(&made.renamed).unwrap(),
            b"new work that blocks recovery"
        );
        assert_eq!(
            fs::read(d.join("saved-original")).unwrap(),
            b"original identity"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_rename_never_replaces_an_existing_name() {
        let d = tmp("rename-clash");
        fs::write(d.join("before"), b"original").unwrap();
        fs::write(d.join("after"), b"bystander").unwrap();
        assert!(rename_recorded(&d.join("before"), OsStr::new("after")).is_err());
        assert_eq!(fs::read(d.join("before")).unwrap(), b"original");
        assert_eq!(fs::read(d.join("after")).unwrap(), b"bystander");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn rename_undo_preserves_an_occupied_name_and_can_be_retried() {
        let d = tmp("rename-occupied");
        fs::write(d.join("before"), b"original").unwrap();
        let made = rename_recorded(&d.join("before"), OsStr::new("after")).unwrap();
        fs::write(&made.original, b"new work").unwrap();
        assert!(undo_rename(&made).unwrap_err().contains("occupied again"));
        assert_eq!(fs::read(&made.original).unwrap(), b"new work");
        assert_eq!(fs::read(&made.renamed).unwrap(), b"original");
        fs::remove_file(&made.original).unwrap();
        undo_rename(&made).unwrap();
        assert_eq!(fs::read(&made.original).unwrap(), b"original");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn rename_undo_refuses_an_edited_target() {
        let d = tmp("rename-edited");
        fs::write(d.join("before"), b"original").unwrap();
        let made = rename_recorded(&d.join("before"), OsStr::new("after")).unwrap();
        fs::write(&made.renamed, b"edited after the rename").unwrap();
        assert!(undo_rename(&made)
            .unwrap_err()
            .contains("changed or replaced"));
        assert!(!made.original.exists());
        assert_eq!(fs::read(&made.renamed).unwrap(), b"edited after the rename");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn rename_undo_moves_a_symlink_without_touching_its_target() {
        let d = tmp("rename-symlink");
        fs::write(d.join("target"), b"keep target").unwrap();
        std::os::unix::fs::symlink("target", d.join("before")).unwrap();
        let made = rename_recorded(&d.join("before"), OsStr::new("after")).unwrap();
        undo_rename(&made).unwrap();
        assert_eq!(fs::read_link(&made.original).unwrap(), Path::new("target"));
        assert_eq!(fs::read(d.join("target")).unwrap(), b"keep target");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn rename_undo_refuses_a_replaced_parent_even_with_the_same_hardlink() {
        let d = tmp("rename-parent");
        fs::create_dir(d.join("folder")).unwrap();
        fs::write(d.join("folder/before"), b"keep bytes").unwrap();
        let made = rename_recorded(&d.join("folder/before"), OsStr::new("after")).unwrap();
        fs::rename(d.join("folder"), d.join("old-folder")).unwrap();
        fs::create_dir(d.join("folder")).unwrap();
        fs::hard_link(d.join("old-folder/after"), &made.renamed).unwrap();
        assert!(undo_rename(&made)
            .unwrap_err()
            .contains("folder was replaced"));
        assert!(!made.original.exists());
        assert_eq!(fs::read(&made.renamed).unwrap(), b"keep bytes");
        assert_eq!(fs::read(d.join("old-folder/after")).unwrap(), b"keep bytes");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn rename_undo_restores_a_folder_without_removing_its_contents() {
        let d = tmp("rename-folder");
        fs::create_dir_all(d.join("before/deep")).unwrap();
        fs::write(d.join("before/deep/work"), b"keep work").unwrap();
        let made = rename_recorded(&d.join("before"), OsStr::new("after")).unwrap();
        undo_rename(&made).unwrap();
        assert_eq!(fs::read(d.join("before/deep/work")).unwrap(), b"keep work");
        assert!(!made.renamed.exists());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn rename_undo_preserves_non_utf8_names() {
        use std::os::unix::ffi::OsStringExt;
        let d = tmp("rename-bytes");
        let before = std::ffi::OsString::from_vec(b"before-\xff".to_vec());
        let after = std::ffi::OsString::from_vec(b"after-\xfe".to_vec());
        fs::write(d.join(&before), b"byte name").unwrap();
        let made = rename_recorded(&d.join(&before), &after).unwrap();
        undo_rename(&made).unwrap();
        assert_eq!(fs::read(d.join(&before)).unwrap(), b"byte name");
        assert!(!d.join(&after).exists());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_link_is_made_and_undone() {
        let d = tmp("link");
        fs::write(d.join("real.txt"), b"x").unwrap();
        let name = link_name_for(&d, &d.join("real.txt"));
        assert_eq!(name, "real.txt (link)");
        let made = create_link(&d.join("real.txt"), &d, &name).unwrap();
        assert_eq!(fs::read_link(&made.link).unwrap(), d.join("real.txt"));
        assert_eq!(link_name_for(&d, &d.join("real.txt")), "real.txt (link 2)");
        undo_link(&made).unwrap();
        assert!(made.link.symlink_metadata().is_err());
        assert!(d.join("real.txt").exists(), "undo never touches the target");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_link_never_replaces_anything() {
        let d = tmp("clash");
        fs::write(d.join("taken"), b"keep").unwrap();
        assert!(create_link(&d.join("x"), &d, OsStr::new("taken")).is_err());
        assert_eq!(fs::read(d.join("taken")).unwrap(), b"keep");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn undo_refuses_a_link_that_was_replaced_by_a_file() {
        let d = tmp("replaced");
        fs::write(d.join("t"), b"x").unwrap();
        let made = create_link(&d.join("t"), &d, OsStr::new("l")).unwrap();
        fs::remove_file(&made.link).unwrap();
        fs::write(&made.link, b"user's new work").unwrap();
        let e = undo_link(&made).unwrap_err();
        assert!(e.contains("left as it is"), "{e}");
        assert_eq!(fs::read(&made.link).unwrap(), b"user's new work");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn undo_refuses_a_relinked_link() {
        let d = tmp("relink");
        fs::write(d.join("t"), b"x").unwrap();
        let made = create_link(&d.join("t"), &d, OsStr::new("l")).unwrap();
        fs::remove_file(&made.link).unwrap();
        std::os::unix::fs::symlink(d.join("elsewhere"), &made.link).unwrap();
        assert!(undo_link(&made).is_err());
        assert!(made.link.symlink_metadata().is_ok());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn mode_specs_parse_like_chmod() {
        assert_eq!(ModeSpec::parse("644").unwrap(), ModeSpec::Exact(0o644));
        assert_eq!(ModeSpec::parse("0755").unwrap(), ModeSpec::Exact(0o755));
        assert_eq!(ModeSpec::parse("u+x").unwrap().apply(0o644), 0o744);
        assert_eq!(ModeSpec::parse("+x").unwrap().apply(0o644), 0o755);
        assert_eq!(ModeSpec::parse("go-w").unwrap().apply(0o666), 0o644);
        assert_eq!(ModeSpec::parse("a+r,u+w").unwrap().apply(0o000), 0o644);
        assert_eq!(ModeSpec::parse("u+x,u-x").unwrap().apply(0o744), 0o644);
    }

    #[test]
    fn special_bits_are_never_touched() {
        assert!(ModeSpec::parse("4755").is_err());
        assert!(ModeSpec::parse("u+s").is_err());
        assert!(ModeSpec::parse("+t").is_err());
        // An existing setgid bit survives an exact change.
        assert_eq!(ModeSpec::Exact(0o700).apply(0o2755), 0o2700);
    }

    #[test]
    fn bad_specs_say_what_is_wrong() {
        for bad in ["", "9", "x", "u+q", "z+x", "u+"] {
            assert!(ModeSpec::parse(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn modes_change_and_undo_for_many_files() {
        let d = tmp("modes");
        let a = d.join("a.sh");
        let b = d.join("b.sh");
        let c = d.join("c.sh");
        for p in [&a, &b, &c] {
            fs::write(p, b"#!/bin/sh\n").unwrap();
            fs::set_permissions(p, fs::Permissions::from_mode(0o644)).unwrap();
        }
        fs::set_permissions(&c, fs::Permissions::from_mode(0o755)).unwrap();
        let out = set_modes(
            &[a.clone(), b.clone(), c.clone()],
            ModeSpec::parse("+x").unwrap(),
        );
        assert_eq!(out.changed.len(), 2);
        assert_eq!(out.unchanged, 1, "c already had it");
        assert_eq!(mode(&a), 0o755);
        let (back, refused) = undo_modes(&out.changed);
        assert_eq!((back, refused.len()), (2, 0));
        assert_eq!(mode(&a), 0o644);
        assert_eq!(mode(&c), 0o755, "the unchanged file is not 'restored'");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn undo_refuses_a_mode_someone_changed_again() {
        let d = tmp("again");
        let a = d.join("a");
        fs::write(&a, b"x").unwrap();
        fs::set_permissions(&a, fs::Permissions::from_mode(0o644)).unwrap();
        let out = set_modes(std::slice::from_ref(&a), ModeSpec::Exact(0o600));
        fs::set_permissions(&a, fs::Permissions::from_mode(0o640)).unwrap();
        let (back, refused) = undo_modes(&out.changed);
        assert_eq!(back, 0);
        assert!(refused[0].contains("changed again"), "{refused:?}");
        assert_eq!(mode(&a), 0o640);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn undo_refuses_a_replaced_file() {
        let d = tmp("swap");
        let a = d.join("a");
        fs::write(&a, b"x").unwrap();
        fs::set_permissions(&a, fs::Permissions::from_mode(0o644)).unwrap();
        let out = set_modes(std::slice::from_ref(&a), ModeSpec::Exact(0o600));
        fs::remove_file(&a).unwrap();
        fs::write(&a, b"new").unwrap();
        fs::set_permissions(&a, fs::Permissions::from_mode(0o600)).unwrap();
        let (back, refused) = undo_modes(&out.changed);
        assert_eq!(back, 0);
        assert!(refused[0].contains("changed since"), "{refused:?}");
        assert_eq!(mode(&a), 0o600);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_symlink_is_skipped_not_followed() {
        let d = tmp("follow");
        let t = d.join("target");
        fs::write(&t, b"x").unwrap();
        fs::set_permissions(&t, fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&t, d.join("l")).unwrap();
        let out = set_modes(&[d.join("l")], ModeSpec::Exact(0o777));
        assert!(out.changed.is_empty());
        assert_eq!(out.failed.len(), 1);
        assert_eq!(mode(&t), 0o600, "the link's target must not change");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn rwx_reads_like_ls() {
        assert_eq!(rwx_string(0o754), "rwxr-xr--");
        assert_eq!(rwx_string(0o000), "---------");
    }

    const PASSWD: &str = "root:x:0:0::/root:/bin/sh\nalice:x:1000:1000::/home/alice:/bin/sh\nbob:x:1001:1001::/home/bob:/bin/sh\n";
    const GROUP: &str = "root:x:0:\nwheel:x:998:alice\nalice:x:1000:\nmedia:x:1500:alice,bob\n";

    #[test]
    fn owner_specs_parse_names_numbers_and_groups() {
        let p = |s| OwnerSpec::parse_with(s, PASSWD, GROUP);
        assert_eq!(
            p("bob").unwrap(),
            OwnerSpec {
                uid: Some(1001),
                gid: None
            }
        );
        assert_eq!(
            p(":media").unwrap(),
            OwnerSpec {
                uid: None,
                gid: Some(1500)
            }
        );
        assert_eq!(
            p("alice:wheel").unwrap(),
            OwnerSpec {
                uid: Some(1000),
                gid: Some(998)
            }
        );
        assert_eq!(
            p("1001:1500").unwrap(),
            OwnerSpec {
                uid: Some(1001),
                gid: Some(1500)
            }
        );
        for bad in ["", ":", "carol", ":nogroup", "alice:"] {
            assert!(p(bad).is_err(), "{bad:?} should be refused");
        }
        assert!(p("carol").unwrap_err().contains("no user called carol"));
    }

    /// Change the group of several files to another group the test user is
    /// in, then undo. Needs a second group (the VM user is in `wheel`).
    #[test]
    fn group_changes_for_many_files_and_undoes() {
        let me_gid = rustix::process::getgid().as_raw();
        let other = rustix::process::getgroups()
            .unwrap_or_default()
            .into_iter()
            .map(|g| g.as_raw())
            .find(|g| *g != me_gid);
        let Some(other) = other else {
            gust_testkit::skip!("GROUPS", "the test user belongs to only one group");
        };
        let d = tmp("owners");
        let a = d.join("a.txt");
        let b = d.join("b.txt");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        fs::create_dir(d.join("sub")).unwrap();
        std::os::unix::fs::symlink(&a, d.join("l")).unwrap();
        let spec = OwnerSpec {
            uid: None,
            gid: Some(other),
        };
        let out = set_owners(&[a.clone(), b.clone(), d.join("sub"), d.join("l")], spec);
        assert_eq!(out.changed.len(), 3, "{:?}", out.failed);
        assert_eq!(out.failed.len(), 1, "the link is skipped, not followed");
        for p in [&a, &b] {
            assert_eq!(p.symlink_metadata().unwrap().gid(), other);
        }
        let (back, refused) = undo_owners(&out.changed);
        assert_eq!((back, refused.len()), (3, 0), "{refused:?}");
        for p in [&a, &b] {
            assert_eq!(p.symlink_metadata().unwrap().gid(), me_gid);
        }
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn owner_undo_refuses_a_file_that_was_replaced() {
        let me_gid = rustix::process::getgid().as_raw();
        let Some(other) = rustix::process::getgroups()
            .unwrap_or_default()
            .into_iter()
            .map(|g| g.as_raw())
            .find(|g| *g != me_gid)
        else {
            gust_testkit::skip!("GROUPS", "the test user belongs to only one group");
        };
        let d = tmp("ownreplaced");
        let a = d.join("a.txt");
        fs::write(&a, b"old").unwrap();
        let out = set_owners(
            std::slice::from_ref(&a),
            OwnerSpec {
                uid: None,
                gid: Some(other),
            },
        );
        assert_eq!(out.changed.len(), 1);
        fs::remove_file(&a).unwrap();
        fs::write(&a, b"user's new work").unwrap();
        let (back, refused) = undo_owners(&out.changed);
        assert_eq!(back, 0);
        assert!(refused[0].contains("no longer there") || refused[0].contains("left as it is"));
        assert_eq!(fs::read(&a).unwrap(), b"user's new work");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn owner_of_a_setuid_file_is_never_changed() {
        let d = tmp("ownsuid");
        let a = d.join("tool");
        fs::write(&a, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&a, fs::Permissions::from_mode(0o4755)).unwrap();
        let out = set_owners(
            std::slice::from_ref(&a),
            OwnerSpec {
                uid: None,
                gid: Some(0),
            },
        );
        assert!(out.changed.is_empty());
        assert!(out.failed[0].contains("setuid"), "{:?}", out.failed);
        assert_eq!(mode(&a), 0o4755);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_new_owner_without_rights_is_refused_in_words() {
        if rustix::process::getuid().is_root() {
            gust_testkit::skip!("NONROOT", "running as root: every chown is allowed");
        }
        let d = tmp("ownroot");
        let a = d.join("a.txt");
        fs::write(&a, b"x").unwrap();
        let out = set_owners(
            std::slice::from_ref(&a),
            OwnerSpec {
                uid: Some(0),
                gid: None,
            },
        );
        assert!(out.changed.is_empty());
        assert!(out.failed[0].contains("administrator"), "{:?}", out.failed);
        fs::remove_dir_all(&d).ok();
    }
}
