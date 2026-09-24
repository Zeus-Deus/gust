//! Folder sizes.
//!
//! A folder's size is the apparent size of every regular file under it —
//! what copying it would move — with each hard-linked file counted once.
//! The walk is the kind that must never surprise:
//! - symlinks are never followed (a link to `/` or to a parent);
//! - it stays on the folder's own filesystem (a mounted share or `/proc`
//!   inside is not the folder's size, and walking it can hang);
//! - it is bounded, cancellable, and reports when it could not see
//!   everything, so a partial number is never shown as exact.

use crate::lister::CancelToken;
use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeLimits {
    /// Stop after this many entries; the answer is then a lower bound.
    pub max_entries: u64,
}

impl Default for SizeLimits {
    fn default() -> Self {
        SizeLimits { max_entries: 2_000_000 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sized {
    pub bytes: u64,
    pub files: u64,
    /// False when anything was skipped (unreadable, limit, cancel): the
    /// size is then "at least" this much.
    pub complete: bool,
}

pub fn dir_size(root: &Path, cancel: &CancelToken, limits: SizeLimits) -> Sized {
    let mut out = Sized { complete: true, ..Sized::default() };
    let Ok(root_md) = std::fs::symlink_metadata(root) else {
        out.complete = false;
        return out;
    };
    if !root_md.is_dir() {
        out.complete = false;
        return out;
    }
    let dev = root_md.dev();
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    let mut entries = 0u64;

    while let Some(dir) = stack.pop() {
        if cancel.is_cancelled() {
            out.complete = false;
            return out;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            out.complete = false;
            continue;
        };
        for e in rd {
            let Ok(e) = e else {
                out.complete = false;
                continue;
            };
            entries += 1;
            if entries > limits.max_entries {
                out.complete = false;
                return out;
            }
            if entries % 1024 == 0 && cancel.is_cancelled() {
                out.complete = false;
                return out;
            }
            // lstat: a symlink is measured as a link, never followed.
            let Ok(md) = e.metadata() else {
                out.complete = false;
                continue;
            };
            let ft = md.file_type();
            if ft.is_dir() {
                if md.dev() == dev {
                    stack.push(e.path());
                }
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            if md.nlink() > 1 && !seen.insert((md.dev(), md.ino())) {
                continue;
            }
            out.bytes = out.bytes.saturating_add(md.len());
            out.files += 1;
        }
    }
    out
}
