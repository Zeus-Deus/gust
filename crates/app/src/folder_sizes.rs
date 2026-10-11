//! Targeted invalidation of cached visible-folder totals after transfers.
use gust_ops::journal::{Job, JobKind};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Component, Path};

/// Remove only visible ancestors of paths the transfer could have changed.
/// The same record describes the paths affected by its inverse. This is
/// display-cache invalidation, never authorization to touch those paths.
pub(crate) fn invalidate_job<T>(sizes: &mut HashMap<OsString, T>, here: &Path, job: &Job) -> bool {
    let mut changed = false;
    for entry in &job.entries {
        let paths = entry
            .destination
            .as_deref()
            .into_iter()
            .chain((job.kind == JobKind::Move).then_some(entry.source.as_path()));
        for path in paths {
            let Ok(relative) = path.strip_prefix(here) else {
                continue;
            };
            if relative.components().any(|c| c == Component::ParentDir) {
                continue;
            }
            match relative.components().next() {
                Some(Component::Normal(name)) => changed |= sizes.remove(name).is_some(),
                None => {
                    changed |= !sizes.is_empty();
                    sizes.clear();
                }
                _ => {}
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use gust_ops::journal::JournalEntry;
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    fn job(kind: JobKind, from: impl Into<PathBuf>, to: impl Into<PathBuf>) -> Job {
        let mut job = Job::new("folder-total", kind);
        job.entries.push(JournalEntry {
            source: from.into(),
            destination: Some(to.into()),
            source_identity: None,
            preserved: None,
            done: true,
            error: None,
        });
        job
    }

    fn sizes() -> HashMap<OsString, u64> {
        [("Backups", 0), ("Untouched", 37), ("From", 61)]
            .into_iter()
            .map(|(name, bytes)| (OsString::from(name), bytes))
            .collect()
    }

    #[test]
    fn copying_refreshes_only_the_destination_folder() {
        let mut cached = sizes();
        let rec = job(JobKind::Copy, "/desk/From/a", "/desk/Backups/a");
        assert!(invalidate_job(&mut cached, Path::new("/desk"), &rec));
        assert!(!cached.contains_key(std::ffi::OsStr::new("Backups")));
        assert_eq!(cached.get(std::ffi::OsStr::new("From")), Some(&61));
        assert_eq!(cached.get(std::ffi::OsStr::new("Untouched")), Some(&37));
    }

    #[test]
    fn moving_refreshes_both_source_and_destination_folders() {
        let mut cached = sizes();
        let rec = job(JobKind::Move, "/desk/From/a", "/desk/Backups/a");
        assert!(invalidate_job(&mut cached, Path::new("/desk"), &rec));
        assert!(!cached.contains_key(std::ffi::OsStr::new("From")));
        assert!(!cached.contains_key(std::ffi::OsStr::new("Backups")));
        assert_eq!(cached.get(std::ffi::OsStr::new("Untouched")), Some(&37));
    }

    #[test]
    fn a_nested_change_invalidates_its_visible_ancestor() {
        let mut cached = sizes();
        let rec = job(JobKind::Copy, "/source/a", "/desk/Backups/deep/a");
        assert!(invalidate_job(&mut cached, Path::new("/desk"), &rec));
        assert!(!cached.contains_key(std::ffi::OsStr::new("Backups")));
        assert_eq!(cached.len(), 2);
    }

    #[test]
    fn a_partial_or_conflicted_entry_may_have_changed_its_destination() {
        let mut cached = sizes();
        let mut rec = job(JobKind::Copy, "/source/a", "/desk/Backups/a");
        rec.entries[0].done = false;
        assert!(invalidate_job(&mut cached, Path::new("/desk"), &rec));
        assert!(!cached.contains_key(std::ffi::OsStr::new("Backups")));
    }

    #[test]
    fn byte_names_are_not_lossily_converted() {
        let name = OsString::from_vec(b"Backups-\xff".to_vec());
        let mut cached = HashMap::from([(name.clone(), 61)]);
        let to = Path::new("/desk").join(&name).join("a");
        let rec = job(JobKind::Copy, "/source/a", to);
        assert!(invalidate_job(&mut cached, Path::new("/desk"), &rec));
        assert!(cached.is_empty());
    }

    #[test]
    fn adjacent_prefixes_and_parent_components_do_not_claim_a_folder() {
        let mut cached = sizes();
        for to in ["/desk-other/Backups/a", "/desk/Backups/../Untouched/a"] {
            let rec = job(JobKind::Copy, "/source/a", to);
            assert!(!invalidate_job(&mut cached, Path::new("/desk"), &rec));
        }
        assert_eq!(cached, sizes());
    }

    #[test]
    fn copying_onto_the_open_root_refreshes_all_its_children() {
        let mut cached = sizes();
        let rec = job(JobKind::Copy, "/source/desk", "/desk");
        assert!(invalidate_job(&mut cached, Path::new("/desk"), &rec));
        assert!(cached.is_empty());
    }

    #[test]
    fn a_job_without_destinations_does_not_clear_unrelated_copy_totals() {
        let mut cached = sizes();
        let mut rec = job(JobKind::Copy, "/desk/From/a", "/desk/Backups/a");
        rec.entries[0].destination = None;
        assert!(!invalidate_job(&mut cached, Path::new("/desk"), &rec));
        assert_eq!(cached, sizes());
    }
}
