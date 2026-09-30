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

fn ident(p: &Path) -> std::io::Result<Ident> {
    let m = p.symlink_metadata()?;
    Ok(Ident {
        dev: m.dev(),
        ino: m.ino(),
        mtime: m.mtime(),
        mtime_nsec: m.mtime_nsec(),
    })
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
}
