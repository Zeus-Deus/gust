//! Merging a folder into a folder of the same name (DESIGN §6.24).
//!
//! The whole plan is made before a byte is written. A merge never
//! overwrites: what is missing goes in, folders on both sides are entered,
//! identical files are left alone, and anything else with a taken name lands
//! beside it under a free "(copy)" name.

use crate::engine::{OpError, OpResult};
use crate::recursive::MAX_DEPTH;
use gust_core::lister::CancelToken;
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};

/// One thing the merge will place: `src` copied or moved to `dst`, a path
/// that is free now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeItem {
    pub src: PathBuf,
    pub dst: PathBuf,
    /// Placed under a "(copy)" name because the name was taken.
    pub renamed: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MergePlan {
    pub items: Vec<MergeItem>,
    /// Source entries already present, byte for byte, at the destination.
    pub already_there: Vec<PathBuf>,
    /// Source folders the merge walks into (and a move empties), parents
    /// before children. The root itself is not included.
    pub entered: Vec<PathBuf>,
}

impl MergePlan {
    pub fn renamed(&self) -> usize {
        self.items.iter().filter(|i| i.renamed).count()
    }
}

/// A real directory, never a symlink to one.
pub fn real_dir(p: &Path) -> bool {
    p.symlink_metadata()
        .is_ok_and(|m| m.file_type().is_dir() && !m.file_type().is_symlink())
}

/// Both sides are real folders: the only shape Merge is offered for.
pub fn can_merge(src: &Path, dst: &Path) -> bool {
    real_dir(src) && real_dir(dst)
}

/// Decide everything a merge of `src` into `dst` will do.
///
/// `fat`: the destination is FAT/exFAT, so names land under their FAT-safe
/// spelling (as an ordinary copy there does).
pub fn plan_merge(src: &Path, dst: &Path, fat: bool, cancel: &CancelToken) -> OpResult<MergePlan> {
    if !can_merge(src, dst) {
        return Err(OpError::Refused(
            "only a folder can be merged into a folder".into(),
        ));
    }
    if dst.starts_with(src) || src.starts_with(dst) {
        return Err(OpError::Refused(
            "a folder cannot be merged with one inside it".into(),
        ));
    }
    let mut plan = MergePlan::default();
    walk(src, dst, 0, fat, cancel, &mut plan)?;
    Ok(plan)
}

fn walk(
    src: &Path,
    dst: &Path,
    depth: usize,
    fat: bool,
    cancel: &CancelToken,
    plan: &mut MergePlan,
) -> OpResult<()> {
    if depth > MAX_DEPTH {
        return Err(OpError::Refused("folder too deep to merge".into()));
    }
    let mut names: Vec<OsString> = std::fs::read_dir(src)?
        .filter_map(|e| e.ok().map(|e| e.file_name()))
        .collect();
    // The order the user sees, not the disk's.
    names.sort();
    // Names this merge will create in `dst`, so two of its own items can
    // never pick the same "(copy)" name.
    let mut reserved: BTreeSet<OsString> = BTreeSet::new();
    for name in names {
        if cancel.checkpoint() {
            return Err(OpError::Cancelled);
        }
        let s = src.join(&name);
        let dname = if fat {
            crate::fat::safe_name(&name).unwrap_or_else(|| name.clone())
        } else {
            name.clone()
        };
        let d = dst.join(&dname);
        let Ok(smd) = s.symlink_metadata() else {
            continue; // vanished since the listing
        };
        let dmd = match d.symlink_metadata() {
            Err(_) => {
                plan.items.push(MergeItem {
                    src: s,
                    dst: d,
                    renamed: false,
                });
                continue;
            }
            Ok(m) => m,
        };
        let s_dir = smd.file_type().is_dir();
        let d_dir = dmd.file_type().is_dir();
        if s_dir && d_dir {
            plan.entered.push(s.clone());
            walk(&s, &d, depth + 1, fat, cancel, plan)?;
            continue;
        }
        if same_content(&s, &smd, &d, &dmd, cancel)? {
            plan.already_there.push(s);
            continue;
        }
        let free = free_name(dst, &dname, &mut reserved)?;
        plan.items.push(MergeItem {
            src: s,
            dst: dst.join(free),
            renamed: true,
        });
    }
    Ok(())
}

/// Two entries hold the same thing: both regular files with equal bytes, or
/// both symlinks with the same target. Never follows a link.
fn same_content(
    a: &Path,
    am: &std::fs::Metadata,
    b: &Path,
    bm: &std::fs::Metadata,
    cancel: &CancelToken,
) -> OpResult<bool> {
    let (at, bt) = (am.file_type(), bm.file_type());
    if at.is_symlink() && bt.is_symlink() {
        return Ok(std::fs::read_link(a)? == std::fs::read_link(b)?);
    }
    if !(at.is_file() && bt.is_file()) || am.len() != bm.len() {
        return Ok(false);
    }
    use std::os::unix::fs::OpenOptionsExt;
    let open = |p: &Path| {
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc_flags())
            .open(p)
    };
    let (mut fa, mut fb) = (open(a)?, open(b)?);
    let mut ba = vec![0u8; 64 * 1024];
    let mut bb = vec![0u8; 64 * 1024];
    loop {
        if cancel.checkpoint() {
            return Err(OpError::Cancelled);
        }
        let n = fa.read(&mut ba)?;
        if n == 0 {
            return Ok(fb.read(&mut bb[..1])? == 0);
        }
        let mut got = 0;
        while got < n {
            let m = fb.read(&mut bb[got..n])?;
            if m == 0 {
                return Ok(false);
            }
            got += m;
        }
        if ba[..n] != bb[..n] {
            return Ok(false);
        }
    }
}

/// O_NOFOLLOW | O_NONBLOCK: a symlink or FIFO swapped in after the stat is
/// refused instead of followed or waited on.
fn libc_flags() -> i32 {
    (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32
}

/// "name (copy).ext", "name (copy 2).ext", … free on disk and not already
/// promised to another item of this merge.
fn free_name(dir: &Path, base: &OsStr, reserved: &mut BTreeSet<OsString>) -> OpResult<OsString> {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let b = base.as_bytes();
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
        if !reserved.contains(&name) && dir.join(&name).symlink_metadata().is_err() {
            reserved.insert(name.clone());
            return Ok(name);
        }
    }
    Err(OpError::Refused("cannot find a free name".into()))
}

/// After a merging move: remove the source folders it emptied, deepest
/// first, root last. `remove_dir` only ever removes an EMPTY folder, so an
/// "already there" file or anything put in since keeps its folder.
/// Returns the folders actually removed.
pub fn remove_emptied(root: &Path, entered: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs: Vec<&PathBuf> = entered.iter().collect();
    dirs.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    let mut removed = Vec::new();
    for d in dirs.into_iter().chain(std::iter::once(&root.to_path_buf())) {
        if real_dir(d) && std::fs::remove_dir(d).is_ok() {
            removed.push(d.clone());
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gust-merge-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn rel(root: &Path, p: &Path) -> String {
        p.strip_prefix(root).unwrap().display().to_string()
    }

    #[test]
    fn a_merge_plan_never_overwrites() {
        let d = tmp("plan");
        let (s, t) = (d.join("src/Invoices"), d.join("dst/Invoices"));
        fs::create_dir_all(s.join("2026/q1")).unwrap();
        fs::create_dir_all(t.join("2026")).unwrap();
        fs::write(s.join("jan.txt"), b"new jan").unwrap();
        fs::write(t.join("jan.txt"), b"OLD JAN").unwrap();
        fs::write(t.join("jan (copy).txt"), b"already a copy").unwrap();
        fs::write(s.join("feb.txt"), b"same").unwrap();
        fs::write(t.join("feb.txt"), b"same").unwrap();
        fs::write(s.join("mar.txt"), b"only in src").unwrap();
        fs::write(s.join("2026/q1/a.txt"), b"a").unwrap();
        fs::write(t.join("may.txt"), b"only in dst").unwrap();
        let plan = plan_merge(&s, &t, false, &CancelToken::new()).unwrap();
        let items: Vec<(String, String, bool)> = plan
            .items
            .iter()
            .map(|i| (rel(&s, &i.src), rel(&t, &i.dst), i.renamed))
            .collect();
        assert_eq!(
            items,
            [
                ("2026/q1".into(), "2026/q1".into(), false),
                ("jan.txt".into(), "jan (copy 2).txt".into(), true),
                ("mar.txt".into(), "mar.txt".into(), false),
            ]
        );
        assert_eq!(plan.already_there, [s.join("feb.txt")]);
        assert_eq!(plan.entered, [s.join("2026")]);
        // Planning wrote nothing.
        assert_eq!(fs::read(t.join("jan.txt")).unwrap(), b"OLD JAN");
        assert!(!t.join("mar.txt").exists());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn two_items_of_one_merge_never_share_a_copy_name() {
        // "a.txt" (a file) and "a.txt" can't both exist, but a file and a
        // folder clashing with different destination kinds can each need
        // "(copy)" names in the same folder; they must differ.
        let d = tmp("reserve");
        let (s, t) = (d.join("s"), d.join("t"));
        fs::create_dir_all(s.join("x")).unwrap();
        fs::create_dir_all(&t).unwrap();
        fs::write(s.join("x.txt"), b"1").unwrap();
        fs::write(t.join("x.txt"), b"2").unwrap();
        fs::write(t.join("x"), b"a file where a folder comes").unwrap();
        let plan = plan_merge(&s, &t, false, &CancelToken::new()).unwrap();
        let dsts: BTreeSet<&PathBuf> = plan.items.iter().map(|i| &i.dst).collect();
        assert_eq!(dsts.len(), plan.items.len(), "{:?}", plan.items);
        assert!(plan.items.iter().all(|i| i.dst.symlink_metadata().is_err()));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_symlinked_folder_is_never_merged_into() {
        let d = tmp("symlink");
        fs::create_dir_all(d.join("s/inner")).unwrap();
        fs::create_dir_all(d.join("elsewhere")).unwrap();
        fs::create_dir_all(d.join("t")).unwrap();
        std::os::unix::fs::symlink(d.join("elsewhere"), d.join("t/inner")).unwrap();
        fs::write(d.join("s/inner/f"), b"x").unwrap();
        // The link at the destination is a name clash, not a folder to
        // enter: the incoming folder lands beside it.
        let plan = plan_merge(&d.join("s"), &d.join("t"), false, &CancelToken::new()).unwrap();
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].dst, d.join("t/inner (copy)"));
        assert!(fs::read_dir(d.join("elsewhere")).unwrap().next().is_none());
        // And a symlink is never the thing merged.
        std::os::unix::fs::symlink(d.join("s"), d.join("link-to-s")).unwrap();
        assert!(!can_merge(&d.join("link-to-s"), &d.join("t")));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_folder_cannot_merge_with_its_own_child() {
        let d = tmp("nested");
        fs::create_dir_all(d.join("a/a")).unwrap();
        assert!(plan_merge(&d.join("a"), &d.join("a/a"), false, &CancelToken::new()).is_err());
        assert!(plan_merge(&d.join("a/a"), &d.join("a"), false, &CancelToken::new()).is_err());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn only_emptied_source_folders_are_removed() {
        let d = tmp("emptied");
        fs::create_dir_all(d.join("s/empty/deeper")).unwrap();
        fs::create_dir_all(d.join("s/kept")).unwrap();
        fs::write(d.join("s/kept/already-there.txt"), b"x").unwrap();
        let removed = remove_emptied(
            &d.join("s"),
            &[
                d.join("s/empty"),
                d.join("s/empty/deeper"),
                d.join("s/kept"),
            ],
        );
        assert_eq!(removed, [d.join("s/empty/deeper"), d.join("s/empty")]);
        assert!(d.join("s/kept/already-there.txt").exists());
        fs::remove_dir_all(&d).ok();
    }
}
