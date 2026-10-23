//! Permanent deletion — the one operation with no undo.
//!
//! Reached only where Trash is genuinely unavailable (a different filesystem,
//! a remote place), after a confirmation that says so and focuses Cancel.
//! Gust never substitutes this for Trash on its own (plan §3 rule 2).
//!
//! Because there is no journal entry that can bring anything back, the
//! guards here are stricter than anywhere else in the codebase:
//!
//! - the target must sit inside the folder the user is looking at, so a
//!   stale row index or a crafted name cannot delete somewhere else;
//! - a symlink is unlinked, never followed — deleting the link must not
//!   delete what it points at;
//! - refuses obvious catastrophes (`/`, `$HOME`, the folder itself);
//! - counts what it removed, so the UI can say something true afterwards.

use std::path::{Component, Path, PathBuf};
#[path = "permanent_worker.rs"]
mod worker;
pub(crate) use worker::{admit_tree, delete_admitted, mount_id};
pub use worker::{
    delete_permanently_with_cancel, delete_permanently_with_progress, DeleteError, DeleteFailure,
    DeletePhase, DeleteProgress,
};

/// Why a permanent delete was refused. Each is a sentence a person can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteRefusal {
    /// The path is not inside the folder the user is looking at.
    OutsideFolder,
    /// Deleting this would be a catastrophe (`/`, home, the folder itself).
    TooDangerous(String),
    /// It is already gone.
    Missing,
}

impl DeleteRefusal {
    pub fn message(&self) -> String {
        match self {
            DeleteRefusal::OutsideFolder => {
                "That item is not in this folder — nothing was deleted.".into()
            }
            DeleteRefusal::TooDangerous(what) => {
                format!("Refusing to delete {what} — nothing was deleted.")
            }
            DeleteRefusal::Missing => "That item is already gone.".into(),
        }
    }
}

/// What a permanent delete did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Removed {
    pub files: usize,
    pub dirs: usize,
}

impl Removed {
    pub fn total(self) -> usize {
        self.files + self.dirs
    }

    /// A sentence for the status line. Never "1 items".
    pub fn summary(self) -> String {
        match (self.files, self.dirs) {
            (0, 0) => "Nothing was deleted".into(),
            (1, 0) => "Deleted permanently".into(),
            (f, 0) => format!("Deleted {f} files permanently"),
            (0, 1) => "Deleted a folder permanently".into(),
            (0, d) => format!("Deleted {d} folders permanently"),
            (f, d) => format!("Deleted {f} files and {d} folders permanently"),
        }
    }
}

/// Would deleting this path be a catastrophe regardless of anything else?
fn catastrophic(path: &Path) -> Option<String> {
    if path.parent().is_none() {
        return Some("the root of the filesystem".into());
    }
    // `/` is one component; `/etc` is two (RootDir + "etc"). Anything at
    // that depth is a top-level system directory and never something a
    // file manager should be removing.
    if path.is_absolute() && path.components().count() <= 2 {
        return Some("a top-level directory".into());
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        if !home.as_os_str().is_empty() && path == home {
            return Some("your home folder".into());
        }
    }
    // A path containing `..` has not been normalised and cannot be checked.
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Some("a path that is not fully resolved".into());
    }
    None
}

/// Check a permanent delete without performing it.
///
/// Separated from the act so every refusal is testable without destroying
/// anything, and so the UI can ask before it commits.
pub fn check(folder: &Path, target: &Path) -> Result<(), DeleteRefusal> {
    if let Some(what) = catastrophic(target) {
        return Err(DeleteRefusal::TooDangerous(what));
    }
    if target == folder {
        return Err(DeleteRefusal::TooDangerous(
            "the folder you are looking at".into(),
        ));
    }
    // Must be a DIRECT child of the folder on screen. Deeper paths are not
    // reachable from a row, so anything else is a bug or an attack.
    if target.parent() != Some(folder) {
        return Err(DeleteRefusal::OutsideFolder);
    }
    // symlink_metadata: a broken symlink still exists and is deletable.
    if std::fs::symlink_metadata(target).is_err() {
        return Err(DeleteRefusal::Missing);
    }
    Ok(())
}

/// Delete permanently. There is no undo.
///
/// `folder` is the directory the user is looking at; `target` must be a
/// direct child of it.
pub fn delete_permanently(folder: &Path, target: &Path) -> Result<Removed, DeleteRefusal> {
    check(folder, target)?;

    let meta = match std::fs::symlink_metadata(target) {
        Ok(m) => m,
        Err(_) => return Err(DeleteRefusal::Missing),
    };

    let mut removed = Removed::default();

    // A symlink is unlinked, never followed: deleting a shortcut must not
    // delete what it points at, even when it points at a directory.
    if meta.file_type().is_symlink() {
        if std::fs::remove_file(target).is_ok() {
            removed.files += 1;
        }
        return Ok(removed);
    }

    if meta.is_dir() {
        remove_tree(target, &mut removed);
    } else if std::fs::remove_file(target).is_ok() {
        removed.files += 1;
    }
    Ok(removed)
}

/// Depth-first removal that never follows a symlink out of the tree.
///
/// `std::fs::remove_dir_all` is not used: its behaviour with symlinked
/// directories has varied across versions, and this is the one operation
/// where "probably fine" is not good enough.
fn remove_tree(dir: &Path, removed: &mut Removed) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() {
            // Unlink the link itself. Never descend through it.
            if std::fs::remove_file(&p).is_ok() {
                removed.files += 1;
            }
        } else if ft.is_dir() {
            remove_tree(&p, removed);
        } else if std::fs::remove_file(&p).is_ok() {
            removed.files += 1;
        }
    }
    if std::fs::remove_dir(dir).is_ok() {
        removed.dirs += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gust-perm-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_file_is_really_gone() {
        let d = tmp("file");
        let f = d.join("bye.txt");
        std::fs::write(&f, b"x").unwrap();

        let r = delete_permanently(&d, &f).expect("refused");
        assert_eq!(r.files, 1);
        assert!(!f.exists(), "the file survived");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_folder_and_its_contents_go_together() {
        let d = tmp("tree");
        let sub = d.join("nested");
        std::fs::create_dir_all(sub.join("deeper")).unwrap();
        std::fs::write(sub.join("a.txt"), b"a").unwrap();
        std::fs::write(sub.join("deeper/b.txt"), b"b").unwrap();

        let r = delete_permanently(&d, &sub).expect("refused");
        assert_eq!(r.files, 2, "not every file was counted: {r:?}");
        assert_eq!(r.dirs, 2, "not every folder was counted: {r:?}");
        assert!(!sub.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn deleting_a_symlink_never_touches_its_target() {
        let d = tmp("symlink");
        let real = d.join("real-data");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("precious.txt"), b"keep me").unwrap();

        let link = d.join("shortcut");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let r = delete_permanently(&d, &link).expect("refused");
        assert_eq!(r.files, 1);
        assert!(!link.exists(), "the link survived");
        assert!(
            real.join("precious.txt").exists(),
            "deleting a shortcut destroyed what it pointed at"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_symlink_inside_a_deleted_folder_does_not_leak_outside() {
        let d = tmp("leak");
        let outside = d.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("safe.txt"), b"safe").unwrap();

        let doomed = d.join("doomed");
        std::fs::create_dir_all(&doomed).unwrap();
        std::os::unix::fs::symlink(&outside, doomed.join("escape")).unwrap();

        delete_permanently(&d, &doomed).expect("refused");
        assert!(!doomed.exists());
        assert!(
            outside.join("safe.txt").exists(),
            "a symlink inside the tree let the delete escape"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn something_outside_the_folder_is_refused() {
        let d = tmp("outside-check");
        let other = tmp("other-folder");
        let victim = other.join("not-yours.txt");
        std::fs::write(&victim, b"x").unwrap();

        assert_eq!(
            delete_permanently(&d, &victim),
            Err(DeleteRefusal::OutsideFolder)
        );
        assert!(victim.exists(), "a file outside the folder was deleted");
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn a_deeper_path_is_refused_even_inside_the_folder() {
        // Only a direct child is reachable from a row; anything else means
        // a stale index or a crafted name.
        let d = tmp("deep");
        let deep = d.join("a/b");
        std::fs::create_dir_all(&deep).unwrap();
        let f = deep.join("c.txt");
        std::fs::write(&f, b"x").unwrap();

        assert_eq!(
            delete_permanently(&d, &f),
            Err(DeleteRefusal::OutsideFolder)
        );
        assert!(f.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn catastrophes_are_refused() {
        let d = tmp("catastrophe");
        assert!(matches!(
            check(&d, Path::new("/")),
            Err(DeleteRefusal::TooDangerous(_))
        ));
        assert!(matches!(
            check(&d, Path::new("/etc")),
            Err(DeleteRefusal::TooDangerous(_))
        ));
        // The folder on screen is never its own target.
        assert!(matches!(check(&d, &d), Err(DeleteRefusal::TooDangerous(_))));
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            assert!(
                matches!(check(&d, &home), Err(DeleteRefusal::TooDangerous(_))),
                "the home directory was not protected"
            );
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_unresolved_path_is_refused() {
        let d = tmp("unresolved");
        let sneaky = d.join("../../etc/passwd");
        assert!(
            matches!(check(&d, &sneaky), Err(DeleteRefusal::TooDangerous(_))),
            "a path containing .. was accepted"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn deleting_something_already_gone_says_so() {
        let d = tmp("missing");
        assert_eq!(
            delete_permanently(&d, &d.join("ghost.txt")),
            Err(DeleteRefusal::Missing)
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_broken_symlink_can_still_be_deleted() {
        // It "does not exist" by a naive check, but the user can see it.
        let d = tmp("broken");
        let link = d.join("dangling");
        std::os::unix::fs::symlink(d.join("nowhere"), &link).unwrap();

        let r = delete_permanently(&d, &link).expect("a broken link was not deletable");
        assert_eq!(r.files, 1);
        assert!(std::fs::symlink_metadata(&link).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn every_refusal_speaks_human() {
        for r in [
            DeleteRefusal::OutsideFolder,
            DeleteRefusal::TooDangerous("your home folder".into()),
            DeleteRefusal::Missing,
        ] {
            let m = r.message();
            assert!(
                !m.contains("os error") && !m.contains("Err("),
                "raw text: {m}"
            );
            assert!(m.ends_with('.'), "not a sentence: {m}");
        }
    }

    #[test]
    fn the_summary_never_says_one_items() {
        assert_eq!(
            Removed { files: 1, dirs: 0 }.summary(),
            "Deleted permanently"
        );
        assert_eq!(
            Removed { files: 0, dirs: 1 }.summary(),
            "Deleted a folder permanently"
        );
        assert_eq!(
            Removed { files: 3, dirs: 2 }.summary(),
            "Deleted 3 files and 2 folders permanently"
        );
        assert_eq!(Removed::default().summary(), "Nothing was deleted");
    }
}
