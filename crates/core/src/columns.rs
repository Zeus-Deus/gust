//! Bounded side listings for the Columns view: the parent folder beside the
//! current one, and a peek into the folder under the cursor.
//!
//! These are glances, not listings: capped in count and time, folders first
//! then natural order, symlinks never followed. Network mounts go through
//! the stall probe first, so a hung share never freezes the view.

use std::ffi::OsString;
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default)]
pub struct Bounded {
    /// (name, is a real folder)
    pub names: Vec<(OsString, bool)>,
    /// Entries beyond the cap.
    pub more: usize,
    pub error: Option<String>,
}

pub fn list_names_bounded(dir: &Path, cap: usize, show_hidden: bool, deadline: Duration) -> Bounded {
    let mut out = Bounded::default();
    if crate::stall::needs_probe(dir) {
        let r = crate::stall::probe_if_risky(dir, deadline);
        if !r.is_responsive() {
            out.error = Some(r.message(&crate::stall::place_name_for(dir)));
            return out;
        }
    }
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            out.error = Some(e.to_string());
            return out;
        }
    };
    let started = Instant::now();
    let mut all: Vec<(OsString, bool)> = Vec::new();
    let mut total = 0usize;
    for e in rd.flatten() {
        let name = e.file_name();
        if !show_hidden && name.as_encoded_bytes().first() == Some(&b'.') {
            continue;
        }
        total += 1;
        // Keep a bounded working set: enough to sort a screenful fairly.
        if all.len() < cap.saturating_mul(8).max(cap) {
            // file_type() is from getdents: no stat, and a symlink is a
            // symlink (not the folder it points to).
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            all.push((name, is_dir));
        }
        if started.elapsed() > deadline {
            break;
        }
    }
    all.sort_by(|a, b| {
        b.1.cmp(&a.1).then_with(|| crate::entry::natural_cmp(&a.0, &b.0))
    });
    all.truncate(cap);
    out.more = total.saturating_sub(all.len());
    out.names = all;
    out
}
