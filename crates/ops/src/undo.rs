//! Subtree-safe undo.
//!
//! The rule that makes this different from a naive implementation
//! (plan §9, Flea #111): root inode+mtime is NOT a subtree fingerprint.
//! An undo that would recursively delete a tree must first prove every
//! descendant is exactly what it was. If anything changed, a new file
//! appeared, or identity cannot be established — preserve the work and ask.

use crate::anchor::FileIdentity;
use crate::journal::{Job, JobKind, JobState, RecordedIdentity};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Why an undo cannot proceed automatically.
#[derive(Debug, Clone, PartialEq)]
pub enum UndoRefusal {
    /// The operation has no safe inverse at all.
    NotUndoable(JobKind),
    /// A file we would remove changed since the operation.
    DescendantChanged(PathBuf),
    /// A file that did not exist at operation time is now inside the subtree.
    NewWorkPresent(PathBuf),
    /// We cannot read identity to prove safety.
    IdentityUnknown(PathBuf),
    /// The remote backend cannot conditionally validate the inverse.
    RemoteCannotValidate,
    Incomplete,
}

impl std::fmt::Display for UndoRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UndoRefusal::NotUndoable(k) => write!(f, "{k:?} cannot be undone"),
            UndoRefusal::DescendantChanged(p) => {
                write!(f, "{} changed since the operation", p.display())
            }
            UndoRefusal::NewWorkPresent(p) => {
                write!(f, "{} is new work created after the operation", p.display())
            }
            UndoRefusal::IdentityUnknown(p) => {
                write!(f, "cannot verify identity of {}", p.display())
            }
            UndoRefusal::RemoteCannotValidate => {
                write!(f, "remote backend cannot validate the inverse")
            }
            UndoRefusal::Incomplete => write!(f, "operation did not complete"),
        }
    }
}

/// What the UI should do with an undo request.
#[derive(Debug, Clone, PartialEq)]
pub enum UndoPlan {
    /// Safe: every touched path verified unchanged.
    Safe { steps: Vec<UndoStep> },
    /// Refused. The UI asks the user instead of destroying anything.
    Refused(UndoRefusal),
}

#[derive(Debug, Clone, PartialEq)]
pub enum UndoStep {
    /// Move something back where it came from.
    MoveBack { from: PathBuf, to: PathBuf },
    /// Remove a file the operation created — only after identity verification.
    ///
    /// `expected` is the identity recorded when the operation created it.
    /// `apply_undo` re-checks it immediately before removing, because time
    /// passes between planning and applying and a file edited in that window
    /// must survive.
    RemoveCreated {
        path: PathBuf,
        expected: Option<RecordedIdentity>,
        /// Identities of everything inside, when `path` is a directory. An
        /// empty map means "nothing recorded", which is refused rather than
        /// assumed safe.
        subtree: BTreeMap<PathBuf, RecordedIdentity>,
    },
    /// Restore preserved overwritten bytes.
    RestorePreserved { preserved: PathBuf, to: PathBuf },
}

/// Snapshot every descendant identity under `root`.
///
/// This is what makes the subtree check real. Symlinks are recorded but never
/// followed.
pub fn snapshot_subtree(root: &Path) -> BTreeMap<PathBuf, RecordedIdentity> {
    let mut out = BTreeMap::new();

    // Record the root itself. When a single FILE is copied, the root is that
    // file and there is nothing to walk — omitting it left undo unable to
    // verify the very thing it was asked to remove.
    if let Ok(id) = FileIdentity::of_path(root) {
        out.insert(root.to_path_buf(), RecordedIdentity::from(id));
    }

    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if let Ok(id) = FileIdentity::of_path(&p) {
                out.insert(p.clone(), RecordedIdentity::from(id));
            }
            let is_real_dir = e
                .file_type()
                .map(|t| t.is_dir() && !t.is_symlink())
                .unwrap_or(false);
            if is_real_dir {
                stack.push(p);
            }
        }
    }
    out
}

/// Verify a subtree is byte-for-byte the tree we recorded.
///
/// Returns the first reason it is not safe to delete.
pub fn verify_subtree_unchanged(
    root: &Path,
    recorded: &BTreeMap<PathBuf, RecordedIdentity>,
) -> Option<UndoRefusal> {
    let current = snapshot_subtree(root);

    // Anything present now that was not recorded is new work.
    for path in current.keys() {
        if !recorded.contains_key(path) {
            return Some(UndoRefusal::NewWorkPresent(path.clone()));
        }
    }
    // Everything recorded must still be identical.
    for (path, rec) in recorded {
        match FileIdentity::of_path(path) {
            Ok(now) => {
                if !rec.matches(&now) {
                    return Some(UndoRefusal::DescendantChanged(path.clone()));
                }
            }
            Err(_) => {
                // A recorded path that has vanished means the tree moved on.
                return Some(UndoRefusal::DescendantChanged(path.clone()));
            }
        }
    }
    None
}

/// Verify a subtree contains exactly what the operation created.
///
/// Unlike `verify_subtree_unchanged`, a path listed in `preexisting` is
/// EXPECTED to be there and is not treated as new work: a merging copy lands
/// beside the user's own files, and those files are not evidence of tampering.
/// They are, however, never removed — `plan_undo` filters them out of the
/// removal set.
pub fn verify_created_subtree(
    root: &Path,
    recorded: &BTreeMap<PathBuf, RecordedIdentity>,
    preexisting: &std::collections::BTreeSet<&PathBuf>,
) -> Option<UndoRefusal> {
    let current = snapshot_subtree(root);

    // Anything present now that was neither recorded nor known to predate the
    // operation is new work created since.
    for path in current.keys() {
        if !recorded.contains_key(path) && !preexisting.contains(path) {
            return Some(UndoRefusal::NewWorkPresent(path.clone()));
        }
    }
    // Everything we created must still be byte-identical. A pre-existing path
    // may legitimately have changed; it is not ours and will not be removed.
    for (path, rec) in recorded {
        if preexisting.contains(path) {
            continue;
        }
        match FileIdentity::of_path(path) {
            Ok(now) => {
                if !rec.matches(&now) {
                    return Some(UndoRefusal::DescendantChanged(path.clone()));
                }
            }
            Err(_) => return Some(UndoRefusal::DescendantChanged(path.clone())),
        }
    }
    None
}

/// Decide whether a job can be undone right now.
pub fn plan_undo(job: &Job) -> UndoPlan {
    if !job.kind.is_undoable() {
        return UndoPlan::Refused(UndoRefusal::NotUndoable(job.kind));
    }
    if job.state != JobState::Confirmed {
        return UndoPlan::Refused(UndoRefusal::Incomplete);
    }

    let mut steps = Vec::new();
    for e in &job.entries {
        if !e.done {
            continue;
        }
        let Some(dest) = &e.destination else { continue };

        match job.kind {
            // Undoing a copy removes the created copy — but only after
            // proving it is still exactly the file we created, and only for
            // paths the copy actually CREATED.
            //
            // A copy into an existing directory merges: the destination
            // folder and any file already in it belong to the user, not to
            // this operation. Removing the whole destination tree destroys
            // data the copy never wrote, which is the merged-copy data-loss
            // case. So the recorded set is filtered to what the operation
            // created, and anything pre-existing is deliberately left.
            JobKind::Copy | JobKind::Extract => {
                if dest.is_dir() {
                    let recorded: BTreeMap<PathBuf, RecordedIdentity> = job
                        .subtree_identities
                        .iter()
                        .filter(|(p, _)| p.starts_with(dest))
                        .cloned()
                        .collect();
                    if recorded.is_empty() {
                        return UndoPlan::Refused(UndoRefusal::IdentityUnknown(dest.clone()));
                    }
                    // Paths that existed at the destination BEFORE this
                    // operation are not ours to delete.
                    let preexisting: std::collections::BTreeSet<&PathBuf> =
                        job.preexisting_at_dest.iter().collect();
                    if let Some(reason) = verify_created_subtree(dest, &recorded, &preexisting) {
                        return UndoPlan::Refused(reason);
                    }
                    let owned: BTreeMap<PathBuf, RecordedIdentity> = recorded
                        .into_iter()
                        .filter(|(p, _)| !preexisting.contains(p))
                        .collect();
                    if owned.is_empty() {
                        // Everything under the destination predates the copy:
                        // there is nothing this operation created to remove.
                        continue;
                    }
                    steps.push(UndoStep::RemoveCreated {
                        path: dest.clone(),
                        expected: None,
                        subtree: owned,
                    });
                    continue;
                } else {
                    // A single file that already existed and was replaced is
                    // restored from its preserved copy, never deleted.
                    if job.preexisting_at_dest.contains(dest) && e.preserved.is_none() {
                        return UndoPlan::Refused(UndoRefusal::NewWorkPresent(dest.clone()));
                    }
                    let Some(expected) = job
                        .subtree_identities
                        .iter()
                        .find(|(p, _)| p == dest)
                        .map(|(_, id)| *id)
                    else {
                        return UndoPlan::Refused(UndoRefusal::IdentityUnknown(dest.clone()));
                    };
                    match FileIdentity::of_path(dest) {
                        Ok(now) if expected.matches(&now) => {}
                        Ok(_) => {
                            return UndoPlan::Refused(UndoRefusal::DescendantChanged(dest.clone()))
                        }
                        Err(_) => {
                            return UndoPlan::Refused(UndoRefusal::IdentityUnknown(dest.clone()))
                        }
                    }
                    steps.push(UndoStep::RemoveCreated {
                        path: dest.clone(),
                        expected: Some(expected),
                        subtree: BTreeMap::new(),
                    });
                }
            }
            // Undoing a move/rename puts it back.
            JobKind::Move | JobKind::Rename | JobKind::Trash => {
                steps.push(UndoStep::MoveBack {
                    from: dest.clone(),
                    to: e.source.clone(),
                });
            }
            JobKind::NewFolder => {
                // A folder we created: removable only while still empty, and
                // only when its identity is the one recorded. Anything the
                // user has since put inside keeps it.
                let expected = job
                    .subtree_identities
                    .iter()
                    .find(|(p, _)| p == dest)
                    .map(|(_, id)| *id);
                let mut subtree = BTreeMap::new();
                if let Some(id) = expected {
                    subtree.insert(dest.clone(), id);
                } else {
                    return UndoPlan::Refused(UndoRefusal::IdentityUnknown(dest.clone()));
                }
                steps.push(UndoStep::RemoveCreated {
                    path: dest.clone(),
                    expected: None,
                    subtree,
                });
            }
            JobKind::DeletePermanent => {
                return UndoPlan::Refused(UndoRefusal::NotUndoable(job.kind));
            }
        }
        if let Some(pres) = &e.preserved {
            steps.push(UndoStep::RestorePreserved {
                preserved: pres.clone(),
                to: dest.clone(),
            });
        }
    }
    UndoPlan::Safe { steps }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{ConflictPolicy, JournalEntry};
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("gust-undo-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn confirmed_copy_job(src: &Path, dest: &Path) -> Job {
        let mut job = Job::new("u1", JobKind::Copy);
        job.conflict_policy = ConflictPolicy::Replace;
        job.entries.push(JournalEntry {
            source: src.to_path_buf(),
            destination: Some(dest.to_path_buf()),
            source_identity: None,
            preserved: None,
            done: true,
            error: None,
        });
        job.state = JobState::Confirmed;
        job
    }

    #[test]
    fn undo_of_permanent_delete_is_always_refused() {
        let mut job = Job::new("d", JobKind::DeletePermanent);
        job.state = JobState::Confirmed;
        assert_eq!(
            plan_undo(&job),
            UndoPlan::Refused(UndoRefusal::NotUndoable(JobKind::DeletePermanent))
        );
    }

    #[test]
    fn undo_of_unfinished_job_is_refused() {
        let mut job = Job::new("x", JobKind::Copy);
        job.state = JobState::Running;
        assert_eq!(plan_undo(&job), UndoPlan::Refused(UndoRefusal::Incomplete));
    }

    #[test]
    fn undo_copy_is_safe_when_nothing_changed() {
        let d = tmp("safe");
        fs::write(d.join("src.txt"), b"x").unwrap();
        fs::write(d.join("dst.txt"), b"x").unwrap();
        let mut job = confirmed_copy_job(&d.join("src.txt"), &d.join("dst.txt"));
        job.subtree_identities.push((
            d.join("dst.txt"),
            RecordedIdentity::from(FileIdentity::of_path(&d.join("dst.txt")).unwrap()),
        ));
        match plan_undo(&job) {
            UndoPlan::Safe { steps } => {
                assert_eq!(steps.len(), 1);
                match &steps[0] {
                    UndoStep::RemoveCreated {
                        path,
                        expected,
                        subtree,
                    } => {
                        assert_eq!(path, &d.join("dst.txt"));
                        assert!(
                            expected.is_some(),
                            "a file removal must carry the identity it will re-verify"
                        );
                        assert!(subtree.is_empty(), "a single file has no subtree");
                    }
                    other => panic!("expected RemoveCreated, got {other:?}"),
                }
            }
            other => panic!("expected Safe, got {other:?}"),
        }
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn undo_refuses_when_the_copied_file_was_edited() {
        let d = tmp("edited");
        fs::write(d.join("src.txt"), b"x").unwrap();
        fs::write(d.join("dst.txt"), b"x").unwrap();
        let mut job = confirmed_copy_job(&d.join("src.txt"), &d.join("dst.txt"));
        job.subtree_identities.push((
            d.join("dst.txt"),
            RecordedIdentity::from(FileIdentity::of_path(&d.join("dst.txt")).unwrap()),
        ));
        // The user edited the copy after the operation.
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(d.join("dst.txt"), b"EDITED BY USER").unwrap();

        match plan_undo(&job) {
            UndoPlan::Refused(UndoRefusal::DescendantChanged(p)) => {
                assert_eq!(p, d.join("dst.txt"));
            }
            other => panic!("edited file must not be silently deleted, got {other:?}"),
        }
        assert_eq!(fs::read(d.join("dst.txt")).unwrap(), b"EDITED BY USER");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn undo_refuses_when_new_work_appeared_inside_the_subtree() {
        // Flea #111: nested work lost on undo. This is the regression test.
        let d = tmp("nested");
        let dest = d.join("copied-dir");
        fs::create_dir_all(dest.join("sub")).unwrap();
        fs::write(dest.join("sub/original.txt"), b"copied").unwrap();

        let mut job = Job::new("n", JobKind::Copy);
        job.entries.push(JournalEntry {
            source: d.join("srcdir"),
            destination: Some(dest.clone()),
            source_identity: None,
            preserved: None,
            done: true,
            error: None,
        });
        job.state = JobState::Confirmed;
        job.subtree_identities = snapshot_subtree(&dest).into_iter().collect();

        // The user then saved NEW work inside the copied tree.
        fs::write(dest.join("sub/my-new-work.txt"), b"do not delete me").unwrap();

        match plan_undo(&job) {
            UndoPlan::Refused(UndoRefusal::NewWorkPresent(p)) => {
                assert_eq!(p, dest.join("sub/my-new-work.txt"));
            }
            other => panic!("new nested work must block undo, got {other:?}"),
        }
        assert!(dest.join("sub/my-new-work.txt").exists(), "work preserved");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn root_mtime_alone_would_have_missed_a_deep_change() {
        // Demonstrates WHY the subtree check exists: editing a deep file does
        // not change the root directory's mtime at all.
        let d = tmp("deep");
        let root = d.join("tree");
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("a/b/deep.txt"), b"one").unwrap();

        let root_before = FileIdentity::of_path(&root).unwrap();
        let recorded = snapshot_subtree(&root);

        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(root.join("a/b/deep.txt"), b"two").unwrap();

        let root_after = FileIdentity::of_path(&root).unwrap();
        assert!(
            root_before.matches(&root_after),
            "root identity is unchanged — a naive check would pass here"
        );
        // The subtree check catches it.
        assert!(matches!(
            verify_subtree_unchanged(&root, &recorded),
            Some(UndoRefusal::DescendantChanged(_))
        ));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn undo_move_plans_a_move_back() {
        let d = tmp("moveback");
        let mut job = Job::new("m", JobKind::Move);
        job.entries.push(JournalEntry {
            source: d.join("a.txt"),
            destination: Some(d.join("b.txt")),
            source_identity: None,
            preserved: None,
            done: true,
            error: None,
        });
        job.state = JobState::Confirmed;
        match plan_undo(&job) {
            UndoPlan::Safe { steps } => assert_eq!(
                steps,
                vec![UndoStep::MoveBack {
                    from: d.join("b.txt"),
                    to: d.join("a.txt")
                }]
            ),
            other => panic!("got {other:?}"),
        }
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn missing_identity_record_refuses_rather_than_guessing() {
        let d = tmp("noident");
        fs::write(d.join("dst.txt"), b"x").unwrap();
        let job = confirmed_copy_job(&d.join("src.txt"), &d.join("dst.txt"));
        // No subtree_identities recorded at all.
        assert!(matches!(
            plan_undo(&job),
            UndoPlan::Refused(UndoRefusal::IdentityUnknown(_))
        ));
        fs::remove_dir_all(&d).ok();
    }
}

/// What actually happened when an undo plan ran.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UndoOutcome {
    pub steps_done: usize,
    pub steps_total: usize,
    /// Steps that were refused or failed, with the reason. Never hidden.
    pub problems: Vec<String>,
}

impl UndoOutcome {
    pub fn is_complete(&self) -> bool {
        self.problems.is_empty() && self.steps_done == self.steps_total
    }
}

/// Execute a verified undo plan.
///
/// `plan_undo` has already proved every path is exactly what the operation
/// created. This re-verifies immediately before each destructive step, because
/// time passes between planning and executing, and a file that changed in that
/// window must survive (DESIGN.md §7).
///
/// A step that cannot be completed is recorded and the rest continue: leaving
/// half an undo silently abandoned is worse than reporting it.
pub fn apply_undo(plan: &UndoPlan) -> UndoOutcome {
    let UndoPlan::Safe { steps } = plan else {
        return UndoOutcome {
            problems: vec!["undo was refused".into()],
            ..Default::default()
        };
    };

    let mut out = UndoOutcome {
        steps_total: steps.len(),
        ..Default::default()
    };

    for step in steps {
        match step {
            UndoStep::MoveBack { from, to } => {
                // Never clobber: if something now occupies the original path,
                // the move back is refused and the user keeps both.
                if to.symlink_metadata().is_ok() {
                    out.problems.push(format!(
                        "{} is occupied again — left in place",
                        to.display()
                    ));
                    continue;
                }
                if let Some(parent) = to.parent() {
                    if !parent.is_dir() {
                        out.problems.push(format!(
                            "{} no longer exists — left in place",
                            parent.display()
                        ));
                        continue;
                    }
                }
                match std::fs::rename(from, to) {
                    Ok(()) => out.steps_done += 1,
                    Err(e) => out.problems.push(format!("{}: {e}", from.display())),
                }
            }

            UndoStep::RemoveCreated {
                path,
                expected,
                subtree,
            } => {
                // Re-verify NOW. The plan proved this was our copy, but that
                // was then: a dialog may have been on screen for a minute and
                // anything edited in that window must survive.
                match remove_verified(path, expected.as_ref(), subtree) {
                    Ok(()) => out.steps_done += 1,
                    Err(why) => out.problems.push(why),
                }
            }

            UndoStep::RestorePreserved { preserved, to } => match std::fs::rename(preserved, to) {
                Ok(()) => out.steps_done += 1,
                Err(e) => out.problems.push(format!("{}: {e}", preserved.display())),
            },
        }
    }
    out
}

/// Remove something this operation created, refusing anything surprising.
///
/// `expected` must match the object on disk RIGHT NOW, not at plan time. For a
/// directory, only the exact paths in `owned` are removed, deepest first, and
/// each is re-verified as it goes. Anything not in `owned` — a file the user
/// created since, or one that predates the operation — stops the removal of
/// its parent and is reported.
fn remove_verified(
    path: &Path,
    expected: Option<&RecordedIdentity>,
    owned: &BTreeMap<PathBuf, RecordedIdentity>,
) -> Result<(), String> {
    let Ok(md) = path.symlink_metadata() else {
        // Already gone: the desired state, not a failure.
        return Ok(());
    };

    // A symlink is unlinked, never followed — but only after proving it is
    // still the link the operation created. Copying a link onto itself and
    // undoing that must not remove the user's original.
    if md.file_type().is_symlink() {
        if let Some(exp) = expected {
            match FileIdentity::of_path(path) {
                Ok(now) if exp.matches(&now) => {}
                Ok(_) => {
                    return Err(format!(
                        "{} changed since the operation — left in place",
                        path.display()
                    ))
                }
                Err(_) => return Err(format!("cannot verify {} — left in place", path.display())),
            }
        } else {
            return Err(format!(
                "{} has no recorded identity — left in place",
                path.display()
            ));
        }
        return std::fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()));
    }

    if md.is_file() {
        let Some(exp) = expected else {
            return Err(format!(
                "{} has no recorded identity — left in place",
                path.display()
            ));
        };
        match FileIdentity::of_path(path) {
            Ok(now) if exp.matches(&now) => {}
            Ok(_) => {
                return Err(format!(
                    "{} changed since the operation — left in place",
                    path.display()
                ))
            }
            Err(_) => return Err(format!("cannot verify {} — left in place", path.display())),
        }
        return std::fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()));
    }

    if md.is_dir() {
        // Remove exactly what the operation created, deepest first, each
        // re-verified immediately before it goes. `remove_dir_all` would take
        // new work with it, which DESIGN.md §7 forbids.
        if owned.is_empty() {
            return Err(format!(
                "{} has no recorded contents — left in place",
                path.display()
            ));
        }
        let mut problems: Vec<String> = Vec::new();
        // Deepest paths first so directories are empty when reached.
        let mut paths: Vec<&PathBuf> = owned.keys().collect();
        paths.sort_by_key(|p| std::cmp::Reverse(p.components().count()));

        for p in paths {
            let Ok(pmd) = p.symlink_metadata() else {
                continue; // already gone
            };
            let exp = owned.get(p);
            if pmd.file_type().is_symlink() || pmd.is_file() {
                match (exp, FileIdentity::of_path(p)) {
                    (Some(e), Ok(now)) if e.matches(&now) => {
                        if let Err(err) = std::fs::remove_file(p) {
                            problems.push(format!("{}: {err}", p.display()));
                        }
                    }
                    (Some(_), Ok(_)) => problems.push(format!(
                        "{} changed since the operation — left in place",
                        p.display()
                    )),
                    _ => problems.push(format!("cannot verify {} — left in place", p.display())),
                }
            } else if pmd.is_dir() {
                // Only removable once empty; if the user put something new in
                // it, removal fails and that is the correct outcome.
                if p.as_path() == path {
                    continue; // the root is handled last, below
                }
                if let Err(err) = std::fs::remove_dir(p) {
                    problems.push(format!(
                        "{} was not empty — left in place ({err})",
                        p.display()
                    ));
                }
            }
        }
        // Finally the root, only if the operation created it and it is now
        // empty. A merged copy's destination folder predates the operation and
        // will still hold the user's files, so this legitimately fails.
        if owned.contains_key(&path.to_path_buf()) {
            if let Err(err) = std::fs::remove_dir(path) {
                problems.push(format!(
                    "{} was not empty — left in place ({err})",
                    path.display()
                ));
            }
        }
        if !problems.is_empty() {
            return Err(problems.join("; "));
        }
        return Ok(());
    }
    Err(format!("{} is a special file — left alone", path.display()))
}

#[cfg(test)]
mod apply_tests {
    use super::*;
    use std::fs;

    /// A removal step carrying the identity the file has right now, which is
    /// what a real plan records. Using this in a test means the step goes
    /// through the same re-verification a real undo does.
    fn remove_now(path: &Path) -> UndoStep {
        let expected = FileIdentity::of_path(path).ok().map(RecordedIdentity::from);
        let mut subtree = BTreeMap::new();
        if path.is_dir()
            && !path
                .symlink_metadata()
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
        {
            subtree = snapshot_subtree(path);
        }
        UndoStep::RemoveCreated {
            path: path.to_path_buf(),
            expected,
            subtree,
        }
    }

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gust-applyundo-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn move_back_returns_a_file_to_its_original_path() {
        let d = tmp("moveback");
        fs::write(d.join("moved.txt"), b"payload").unwrap();
        let plan = UndoPlan::Safe {
            steps: vec![UndoStep::MoveBack {
                from: d.join("moved.txt"),
                to: d.join("original.txt"),
            }],
        };
        let out = apply_undo(&plan);
        assert!(out.is_complete(), "{out:?}");
        assert_eq!(fs::read(d.join("original.txt")).unwrap(), b"payload");
        assert!(!d.join("moved.txt").exists());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn move_back_refuses_when_the_original_path_is_occupied_again() {
        let d = tmp("occupied");
        fs::write(d.join("moved.txt"), b"the moved one").unwrap();
        fs::write(d.join("original.txt"), b"NEW WORK").unwrap();

        let plan = UndoPlan::Safe {
            steps: vec![UndoStep::MoveBack {
                from: d.join("moved.txt"),
                to: d.join("original.txt"),
            }],
        };
        let out = apply_undo(&plan);
        assert!(!out.is_complete());
        assert_eq!(out.problems.len(), 1);
        assert!(out.problems[0].contains("occupied"));
        // BOTH survive: nothing was overwritten, nothing was lost.
        assert_eq!(fs::read(d.join("original.txt")).unwrap(), b"NEW WORK");
        assert_eq!(fs::read(d.join("moved.txt")).unwrap(), b"the moved one");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn remove_created_deletes_a_copy_we_made() {
        let d = tmp("removecopy");
        fs::write(d.join("copy.txt"), b"a copy").unwrap();
        let plan = UndoPlan::Safe {
            steps: vec![remove_now(&d.join("copy.txt"))],
        };
        assert!(apply_undo(&plan).is_complete());
        assert!(!d.join("copy.txt").exists());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn removing_an_already_deleted_path_is_not_a_failure() {
        let d = tmp("gone");
        let plan = UndoPlan::Safe {
            steps: vec![UndoStep::RemoveCreated {
                path: d.join("never-existed"),
                expected: None,
                subtree: BTreeMap::new(),
            }],
        };
        let out = apply_undo(&plan);
        assert!(
            out.is_complete(),
            "the desired state already holds: {out:?}"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn removing_a_created_directory_takes_its_whole_copied_tree() {
        let d = tmp("removetree");
        fs::create_dir_all(d.join("copied/inner")).unwrap();
        fs::write(d.join("copied/inner/f.txt"), b"x").unwrap();
        let plan = UndoPlan::Safe {
            steps: vec![remove_now(&d.join("copied"))],
        };
        assert!(apply_undo(&plan).is_complete());
        assert!(!d.join("copied").exists());
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_symlink_is_unlinked_and_its_target_survives() {
        let d = tmp("symlink");
        fs::write(d.join("target.txt"), b"important").unwrap();
        std::os::unix::fs::symlink(d.join("target.txt"), d.join("link")).unwrap();
        let plan = UndoPlan::Safe {
            steps: vec![remove_now(&d.join("link"))],
        };
        assert!(apply_undo(&plan).is_complete());
        assert!(!d.join("link").exists());
        assert_eq!(
            fs::read(d.join("target.txt")).unwrap(),
            b"important",
            "undo must never follow a link and delete its target"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_refused_plan_executes_nothing() {
        let plan = UndoPlan::Refused(UndoRefusal::Incomplete);
        let out = apply_undo(&plan);
        assert_eq!(out.steps_done, 0);
        assert!(!out.problems.is_empty());
    }

    #[test]
    fn one_failing_step_does_not_abandon_the_others() {
        let d = tmp("partial");
        fs::write(d.join("a.txt"), b"a").unwrap();
        fs::write(d.join("blocked.txt"), b"moved").unwrap();
        fs::write(d.join("in-the-way.txt"), b"NEW").unwrap();
        fs::write(d.join("c.txt"), b"c").unwrap();

        let plan = UndoPlan::Safe {
            steps: vec![
                remove_now(&d.join("a.txt")),
                UndoStep::MoveBack {
                    from: d.join("blocked.txt"),
                    to: d.join("in-the-way.txt"),
                },
                remove_now(&d.join("c.txt")),
            ],
        };
        let out = apply_undo(&plan);
        assert_eq!(out.steps_done, 2, "the two good steps ran");
        assert_eq!(out.problems.len(), 1, "the blocked one is reported");
        assert!(!d.join("a.txt").exists());
        assert!(!d.join("c.txt").exists());
        assert_eq!(fs::read(d.join("in-the-way.txt")).unwrap(), b"NEW");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn move_back_into_a_vanished_folder_is_reported_not_lost() {
        let d = tmp("noparent");
        fs::write(d.join("moved.txt"), b"data").unwrap();
        let plan = UndoPlan::Safe {
            steps: vec![UndoStep::MoveBack {
                from: d.join("moved.txt"),
                to: d.join("deleted-folder/original.txt"),
            }],
        };
        let out = apply_undo(&plan);
        assert!(!out.is_complete());
        assert_eq!(
            fs::read(d.join("moved.txt")).unwrap(),
            b"data",
            "the file must still exist somewhere"
        );
        fs::remove_dir_all(&d).ok();
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gust-snap-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    /// Regression: a copied single file could not be undone, because the
    /// snapshot only recorded a directory's CONTENTS and never the root, so
    /// the one path undo needed to verify was missing.
    #[test]
    fn a_single_file_root_is_recorded() {
        let d = tmp("fileroot");
        let f = d.join("copied.txt");
        fs::write(&f, b"x").unwrap();
        let ids = snapshot_subtree(&f);
        assert!(
            ids.contains_key(&f),
            "the file itself must be recorded, not just a directory's contents"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_directory_root_and_its_contents_are_both_recorded() {
        let d = tmp("dirroot");
        let root = d.join("tree");
        fs::create_dir_all(root.join("inner")).unwrap();
        fs::write(root.join("inner/f.txt"), b"x").unwrap();
        let ids = snapshot_subtree(&root);
        assert!(ids.contains_key(&root), "the root directory itself");
        assert!(ids.contains_key(&root.join("inner")));
        assert!(ids.contains_key(&root.join("inner/f.txt")));
        fs::remove_dir_all(&d).ok();
    }
}
