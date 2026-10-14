//! Setting colour tags, with a one-step Undo (DESIGN §6.25).
//!
//! The whole stored value of each file is remembered before it is changed,
//! so Undo puts back exactly what was there (absent included), and only on
//! a file that is still the same object (dev + inode) whose tags have not
//! been changed again since.

use crate::attrs::{ident_of, Ident};
use gust_core::tags::{self, ATTR};
use std::path::{Path, PathBuf};

/// What one file's tags were and became.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagChanged {
    pub path: PathBuf,
    pub before: Option<Vec<u8>>,
    pub after: Option<Vec<u8>>,
    pub ident: Ident,
}

#[derive(Debug, Default, Clone)]
pub struct TagOutcome {
    pub changed: Vec<TagChanged>,
    pub unchanged: usize,
    pub failed: Vec<String>,
}

/// What to do to each file's colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagOp {
    /// Colour 1–7: added to every file if any lacks it, else removed from
    /// all — the way a toggle over a mixed selection is expected to work.
    Toggle(usize),
    /// Every colour removed (names Gust does not know are kept).
    Clear,
}

fn name_of(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn write(path: &Path, value: &Option<Vec<u8>>) -> std::io::Result<()> {
    match value {
        Some(v) => rustix::fs::lsetxattr(path, ATTR, v, rustix::fs::XattrFlags::empty())
            .map_err(Into::into),
        None => match rustix::fs::lremovexattr(path, ATTR) {
            Ok(()) => Ok(()),
            Err(e) if e == rustix::io::Errno::NODATA => Ok(()),
            Err(e) => Err(e.into()),
        },
    }
}

fn plain(e: &std::io::Error) -> String {
    if e.raw_os_error() == Some(rustix::io::Errno::NOTSUP.raw_os_error()) {
        "this drive cannot hold tags".into()
    } else if e.raw_os_error() == Some(rustix::io::Errno::PERM.raw_os_error()) {
        // Linux allows user.* attributes on files and folders only.
        "only files and folders can be tagged".into()
    } else {
        crate::engine::plain_io_message(e)
    }
}

/// Apply `op` to every path. Each file is read, changed and written on its
/// own; one that cannot be tagged is reported and the rest still are.
pub fn set_tags(paths: &[PathBuf], op: TagOp) -> TagOutcome {
    let mut out = TagOutcome::default();
    let mut current: Vec<(PathBuf, Option<Vec<u8>>)> = Vec::new();
    for p in paths {
        match tags::read_raw(p) {
            Ok(v) => current.push((p.clone(), v)),
            Err(e) => out.failed.push(format!("{}: {}", name_of(p), plain(&e))),
        }
    }
    // A toggle turns the colour ON unless every file already has it.
    let turn_on = match op {
        TagOp::Toggle(n) => !current.iter().all(|(_, raw)| {
            let names = raw.as_deref().map(tags::parse).unwrap_or_default();
            tags::mask_of(&names) & (1 << (n.saturating_sub(1))) != 0
        }),
        TagOp::Clear => false,
    };
    for (p, before) in current {
        let names = before.as_deref().map(tags::parse).unwrap_or_default();
        let next = match op {
            TagOp::Toggle(n) => tags::with_colour(&names, n, turn_on),
            TagOp::Clear => tags::without_colours(&names),
        };
        let after = if next.is_empty() {
            None
        } else {
            Some(tags::join(&next))
        };
        if after == before || (after.is_none() && before.as_deref() == Some(b"")) {
            out.unchanged += 1;
            continue;
        }
        let ident = match ident_of(&p) {
            Ok(i) => i,
            Err(e) => {
                out.failed.push(format!("{}: {}", name_of(&p), plain(&e)));
                continue;
            }
        };
        match write(&p, &after) {
            Ok(()) => out.changed.push(TagChanged {
                path: p,
                before,
                after,
                ident,
            }),
            Err(e) => out.failed.push(format!("{}: {}", name_of(&p), plain(&e))),
        }
    }
    out
}

/// Put every change back, refusing any file that is no longer the same
/// object or whose tags were changed again since.
pub fn undo_tags(changes: &[TagChanged]) -> (usize, Vec<String>) {
    let mut back = 0;
    let mut refused = Vec::new();
    for c in changes {
        let name = name_of(&c.path);
        match ident_of(&c.path) {
            Ok(i) if i.dev == c.ident.dev && i.ino == c.ident.ino => {}
            Ok(_) => {
                refused.push(format!("{name}: replaced since; left as it is"));
                continue;
            }
            Err(_) => {
                refused.push(format!("{name}: no longer there"));
                continue;
            }
        }
        match tags::read_raw(&c.path) {
            Ok(now) if now == c.after => {}
            _ => {
                refused.push(format!(
                    "{name}: its tags were changed again; left as they are"
                ));
                continue;
            }
        }
        match write(&c.path, &c.before) {
            Ok(()) => back += 1,
            Err(e) => refused.push(format!("{name}: {}", plain(&e))),
        }
    }
    (back, refused)
}

/// Carry the tags of the file being copied onto its copy (an open file
/// descriptor, so it lands on the staged file before it gets its name).
/// A destination that cannot hold them keeps the copy and says nothing:
/// the bytes are what was asked for.
pub(crate) fn carry(src: &impl rustix::fd::AsFd, dst: &impl rustix::fd::AsFd) {
    let mut buf = vec![0u8; tags::MAX_VALUE];
    if let Ok(n) = rustix::fs::fgetxattr(src, ATTR, &mut buf[..]) {
        let _ = rustix::fs::fsetxattr(dst, ATTR, &buf[..n], rustix::fs::XattrFlags::empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gust-tags-ops-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn supported(d: &Path) -> bool {
        let f = d.join(".probe");
        std::fs::write(&f, "").unwrap();
        let ok = rustix::fs::lsetxattr(&f, ATTR, b"x", rustix::fs::XattrFlags::empty()).is_ok();
        std::fs::remove_file(&f).ok();
        ok
    }

    fn raw(p: &Path) -> Option<String> {
        tags::read_raw(p)
            .unwrap()
            .map(|v| String::from_utf8(v).unwrap())
    }

    #[test]
    fn tag_then_undo_puts_back_exactly_what_was_there() {
        let d = tmp("undo");
        if !supported(&d) {
            eprintln!("skipped: temp dir has no user xattrs");
            return;
        }
        let a = d.join("a.txt");
        let b = d.join("b.txt");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        // b already carries a Dolphin word tag.
        write(&b, &Some(b"work".to_vec())).unwrap();

        let out = set_tags(&[a.clone(), b.clone()], TagOp::Toggle(1));
        assert!(out.failed.is_empty(), "{:?}", out.failed);
        assert_eq!(out.changed.len(), 2);
        assert_eq!(raw(&a).as_deref(), Some("Red"));
        assert_eq!(raw(&b).as_deref(), Some("work,Red"));

        let (back, refused) = undo_tags(&out.changed);
        assert_eq!((back, refused.len()), (2, 0), "{refused:?}");
        assert_eq!(raw(&a), None, "an untagged file is untagged again");
        assert_eq!(raw(&b).as_deref(), Some("work"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_mixed_toggle_turns_the_colour_on_for_all_then_off_for_all() {
        let d = tmp("mixed");
        if !supported(&d) {
            return;
        }
        let a = d.join("a");
        let b = d.join("b");
        std::fs::write(&a, "").unwrap();
        std::fs::write(&b, "").unwrap();
        write(&a, &Some(b"Blue".to_vec())).unwrap();
        set_tags(&[a.clone(), b.clone()], TagOp::Toggle(5));
        assert_eq!(raw(&a).as_deref(), Some("Blue"));
        assert_eq!(raw(&b).as_deref(), Some("Blue"));
        set_tags(&[a.clone(), b.clone()], TagOp::Toggle(5));
        assert_eq!((raw(&a), raw(&b)), (None, None));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn undo_refuses_tags_changed_again_since() {
        let d = tmp("again");
        if !supported(&d) {
            return;
        }
        let a = d.join("a");
        std::fs::write(&a, "").unwrap();
        let out = set_tags(std::slice::from_ref(&a), TagOp::Toggle(4));
        write(&a, &Some(b"Green,Gray".to_vec())).unwrap();
        let (back, refused) = undo_tags(&out.changed);
        assert_eq!(back, 0);
        assert_eq!(refused.len(), 1);
        assert_eq!(
            raw(&a).as_deref(),
            Some("Green,Gray"),
            "left as the user made it"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn undo_refuses_a_replaced_file() {
        let d = tmp("replaced");
        if !supported(&d) {
            return;
        }
        let a = d.join("a");
        std::fs::write(&a, "").unwrap();
        let out = set_tags(std::slice::from_ref(&a), TagOp::Toggle(2));
        // Something else now lives at that name (same tag value, new inode).
        let other = d.join("other");
        std::fs::write(&other, "").unwrap();
        write(&other, &Some(b"Orange".to_vec())).unwrap();
        std::fs::rename(&other, &a).unwrap();
        let (back, refused) = undo_tags(&out.changed);
        assert_eq!(back, 0);
        assert!(refused[0].contains("replaced"), "{refused:?}");
        assert_eq!(raw(&a).as_deref(), Some("Orange"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn clear_keeps_word_tags() {
        let d = tmp("clear");
        if !supported(&d) {
            return;
        }
        let a = d.join("a");
        std::fs::write(&a, "").unwrap();
        write(&a, &Some(b"Red,work,Blue".to_vec())).unwrap();
        let out = set_tags(std::slice::from_ref(&a), TagOp::Clear);
        assert_eq!(out.changed.len(), 1);
        assert_eq!(raw(&a).as_deref(), Some("work"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_symlink_is_reported_not_followed() {
        let d = tmp("link");
        if !supported(&d) {
            return;
        }
        let target = d.join("target");
        std::fs::write(&target, "").unwrap();
        let link = d.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let out = set_tags(std::slice::from_ref(&link), TagOp::Toggle(1));
        assert!(out.changed.is_empty());
        assert_eq!(out.failed.len(), 1, "{:?}", out.failed);
        assert_eq!(raw(&target), None, "the link's target was not tagged");
        std::fs::remove_dir_all(&d).ok();
    }
}
