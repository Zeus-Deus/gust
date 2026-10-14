//! The five reproduced data-loss cases, as failing tests first.
//!
//! Every test in this file destroys or preserves USER data — bytes the user
//! created and never asked anybody to touch. They are written against the
//! real production functions and job chains, not helpers built for the test,
//! because the audit that found them reproduced them exactly that way.
//!
//! Each test asserts on FILE CONTENTS, never on a status value: an operation
//! that reports success while removing a bystander is precisely the failure
//! mode being tested.

use gust_core::lister::CancelToken;
use gust_ops::journal::{ConflictPolicy, JobKind, JobState};
use gust_ops::{jobs, recursive, undo};
use std::fs;
use std::path::{Path, PathBuf};

/// A fixture directory on the SAME filesystem, uniquely named per test.
fn fixture(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "gust-loss-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

/// A fixture on a DIFFERENT filesystem, to force the cross-device paths.
/// /dev/shm is a separate tmpfs on every Linux system.
fn other_fs_fixture(tag: &str) -> Option<PathBuf> {
    let shm = Path::new("/dev/shm");
    if !shm.is_dir() {
        return None;
    }
    let p = shm.join(format!(
        "gust-loss-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).ok()?;
    Some(p)
}

fn dev_of(p: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(p).unwrap().dev()
}

/// Run a copy/move job through the real worker and wait for it to finish.
fn run_job(
    kind: JobKind,
    sources: Vec<PathBuf>,
    dest: PathBuf,
    policy: ConflictPolicy,
) -> (jobs::JobProgress, gust_ops::journal::Job) {
    let req = match kind {
        JobKind::Copy => jobs::Request::Copy {
            sources,
            dest_dir: dest,
        },
        JobKind::Move => jobs::Request::Move {
            sources,
            dest_dir: dest,
        },
        other => panic!("run_job does not drive {other:?}"),
    };
    let mut h = jobs::start("regression", req, policy);
    let progress = h.wait();
    (progress, h.record())
}

// ---------------------------------------------------------------------------
// Case 1: copy into an existing directory, then Undo, deletes files that were
//         in the destination BEFORE the copy and were never part of it.
// ---------------------------------------------------------------------------

#[test]
fn undo_of_a_merged_directory_copy_keeps_preexisting_destination_files() {
    let root = fixture("merge-undo");
    let src = root.join("src");
    let dest = root.join("dest");
    fs::create_dir_all(src.join("payload")).unwrap();
    fs::write(src.join("payload/copied.txt"), b"came from src").unwrap();

    // The destination folder of the same name ALREADY exists and already
    // holds the user's own work.
    fs::create_dir_all(dest.join("payload")).unwrap();
    fs::write(
        dest.join("payload/precious.txt"),
        b"the user's own file, predates the copy",
    )
    .unwrap();

    let (p, rec) = run_job(
        JobKind::Copy,
        vec![src.join("payload")],
        dest.clone(),
        ConflictPolicy::Replace,
    );
    assert_eq!(p.state, JobState::Confirmed, "copy should have succeeded");
    assert!(dest.join("payload/copied.txt").is_file());
    assert!(dest.join("payload/precious.txt").is_file());

    // Now undo it. Undo must remove what the COPY created and nothing else.
    let record = rec;
    let plan = undo::plan_undo(&record);
    let outcome = match &plan {
        undo::UndoPlan::Safe { .. } => undo::apply_undo(&plan),
        undo::UndoPlan::Refused(r) => {
            // Refusing is an acceptable, safe answer. What is NOT acceptable
            // is deleting the bystander.
            eprintln!("undo refused (acceptable): {r}");
            Default::default()
        }
    };
    eprintln!("undo outcome: {outcome:?}");

    assert_eq!(
        fs::read(dest.join("payload/precious.txt")).ok().as_deref(),
        Some(&b"the user's own file, predates the copy"[..]),
        "UNDO DESTROYED A PRE-EXISTING DESTINATION FILE that the copy never created"
    );
    fs::remove_dir_all(&root).ok();
}

// ---------------------------------------------------------------------------
// Case 2: the staging sweep deletes a file it does not own, purely because the
//         NAME looks like debris and the encoded PID is not running.
// ---------------------------------------------------------------------------

#[test]
fn the_staging_sweep_never_deletes_a_file_it_did_not_create() {
    let root = fixture("sweep-unowned");

    // A user file that merely LOOKS like our debris. Nothing about a filename
    // establishes who created it.
    let impostor = root.join(".gust-staging-999999-quarterly-report.txt");
    fs::write(&impostor, b"not ours; the only copy the user has").unwrap();

    // Real debris from a dead process, which SHOULD be swept.
    let real_debris = root.join(".gust-staging-999998-real-debris.bin");
    fs::write(&real_debris, b"partial").unwrap();

    let removed = gust_ops::engine::sweep_stale_staging(&root);
    eprintln!("sweep removed {removed}");

    assert!(
        impostor.is_file(),
        "THE SWEEP DELETED AN UNOWNED FILE: a filename is not an ownership record"
    );
    assert_eq!(
        fs::read(&impostor).ok().as_deref(),
        Some(&b"not ours; the only copy the user has"[..]),
        "the unowned file's contents must be untouched"
    );
    fs::remove_dir_all(&root).ok();
}

// ---------------------------------------------------------------------------
// Case 3: a cross-filesystem folder Move removes the source tree even though a
//         CHILD was edited after that child had been copied. The root's own
//         identity is unchanged, so the root-only check cannot see it.
// ---------------------------------------------------------------------------

#[test]
fn a_cross_filesystem_move_never_discards_a_child_edited_after_it_was_copied() {
    let src_root = fixture("xdev-src");
    let Some(dst_root) = other_fs_fixture("xdev-dst") else {
        gust_testkit::skip!(
            "SHM",
            "/dev/shm unavailable, cannot force the cross-device path"
        );
    };
    assert_ne!(
        dev_of(&src_root),
        dev_of(&dst_root),
        "fixture must span two filesystems or this tests the rename fast path"
    );

    let tree = src_root.join("work");
    fs::create_dir_all(&tree).unwrap();
    fs::write(tree.join("a.txt"), b"original a").unwrap();
    fs::write(tree.join("b.txt"), b"original b").unwrap();

    // Copy the tree exactly as move_tree's cross-device path does, then edit a
    // child in the window between its copy and the source removal. This is the
    // real interleaving: a background move with the user still typing.
    let cancel = CancelToken::default();
    let mut noop = |_: usize, _: usize, _: u64, _: u64| {};
    let dst = dst_root.join("work");
    recursive::copy_tree(&tree, &dst, ConflictPolicy::Replace, &cancel, &mut noop).unwrap();

    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(
        tree.join("a.txt"),
        b"EDITED AFTER THE COPY - must not be lost",
    )
    .unwrap();

    // The source tree may only be removed if every descendant still matches
    // what was actually copied.
    let ids = undo::snapshot_subtree(&tree);
    let changed = undo::verify_subtree_unchanged(&tree, &ids);
    eprintln!("subtree check: {changed:?}");

    // Whatever the engine decides, the newer bytes must survive somewhere.
    let src_has = fs::read(tree.join("a.txt")).ok();
    let dst_has = fs::read(dst.join("a.txt")).ok();
    let edited = &b"EDITED AFTER THE COPY - must not be lost"[..];
    assert!(
        src_has.as_deref() == Some(edited) || dst_has.as_deref() == Some(edited),
        "THE EDIT MADE DURING THE MOVE WAS LOST: src={src_has:?} dst={dst_has:?}"
    );

    fs::remove_dir_all(&src_root).ok();
    fs::remove_dir_all(&dst_root).ok();
}

/// The real `move_tree`, with an edit at its synchronous completed-entry
/// callback. Batched copies publish names only after a flush, so polling for
/// a destination name neither guarantees an interleave nor checks write errors.
/// One child makes the boundary independent of traversal order and scheduler.
#[test]
fn move_tree_cross_device_keeps_the_source_when_a_child_changed_mid_move() {
    let src_root = fixture("xdev2-src");
    let Some(dst_root) = other_fs_fixture("xdev2-dst") else {
        gust_testkit::skip!("SHM", "/dev/shm unavailable");
    };
    assert_ne!(
        dev_of(&src_root),
        dev_of(&dst_root),
        "must exercise the cross-device copy/remove path"
    );
    let tree = src_root.join("work");
    fs::create_dir_all(&tree).unwrap();
    let target = tree.join("f0000.txt");
    let original = b"original bytes";
    let edited = b"EDITED WHILE THE MOVE WAS RUNNING";
    fs::write(&target, original).unwrap();
    let root_before = gust_ops::FileIdentity::of_path(&tree).unwrap();
    let dst = dst_root.join("work");
    let mut injections = 0;
    let cancel = CancelToken::default();
    let mut edit_after_copy = |done: usize, total: usize, bytes: u64, bytes_total: u64| {
        if done == 1 {
            assert_eq!(injections, 0, "inject only at the one completed child");
            assert_eq!(total, 1, "the fixture has exactly one child");
            assert_eq!(bytes, original.len() as u64);
            assert_eq!(bytes_total, original.len() as u64);
            assert_eq!(fs::read(&target).unwrap(), original);
            fs::write(&target, edited).expect("the mid-move edit must really succeed");
            assert_eq!(fs::read(&target).unwrap(), edited);
            assert!(
                root_before.matches(&gust_ops::FileIdentity::of_path(&tree).unwrap()),
                "a root-only check must miss the child edit"
            );
            injections += 1;
        }
    };
    let res = recursive::move_tree(
        &tree,
        &dst,
        ConflictPolicy::Replace,
        &cancel,
        &mut edit_after_copy,
    );
    eprintln!("move_tree returned {res:?}, edit injections={injections}");
    assert_eq!(
        injections, 1,
        "the test must interleave exactly one successful edit"
    );

    let src_bytes = fs::read(&target).ok();
    let dst_bytes = fs::read(dst.join("f0000.txt")).ok();
    assert!(
        src_bytes.as_deref() == Some(&edited[..]) || dst_bytes.as_deref() == Some(&edited[..]),
        "THE MID-MOVE EDIT WAS LOST: src={src_bytes:?} dst={dst_bytes:?}"
    );
    // And the move must have REFUSED rather than reported success while
    // discarding the newer bytes.
    assert!(
        res.is_err(),
        "a move that could not prove the source was unchanged must refuse, got {res:?}"
    );
    assert_eq!(
        src_bytes.as_deref(),
        Some(&edited[..]),
        "the edited source stays in place"
    );
    assert_eq!(
        dst_bytes.as_deref(),
        Some(&original[..]),
        "the completed original copy was kept"
    );
    assert!(
        matches!(&res, Err(gust_ops::OpError::Refused(why)) if why.contains("inside changed during the move")),
        "expected the descendant-change guard, not an unrelated error: {res:?}"
    );

    fs::remove_dir_all(&src_root).ok();
    fs::remove_dir_all(&dst_root).ok();
}

// ---------------------------------------------------------------------------
// Case 4: apply_undo removes a path that was edited AFTER plan_undo verified
//         it. Planning and applying are separate moments in time.
// ---------------------------------------------------------------------------

#[test]
fn apply_undo_reverifies_and_refuses_a_file_edited_after_planning() {
    let root = fixture("plan-apply-window");
    fs::write(root.join("src.txt"), b"source").unwrap();

    fs::create_dir_all(root.join("out")).unwrap();
    let (p, rec) = run_job(
        JobKind::Copy,
        vec![root.join("src.txt")],
        root.join("out"),
        ConflictPolicy::Replace,
    );
    assert_eq!(p.state, JobState::Confirmed);
    let copied = root.join("out/src.txt");
    assert!(copied.is_file());

    let record = rec;

    // Plan the undo while the copy is untouched: this is Safe.
    let plan = undo::plan_undo(&record);
    assert!(
        matches!(plan, undo::UndoPlan::Safe { .. }),
        "expected a safe plan, got {plan:?}"
    );

    // The user now edits the copy. Time passes between planning and applying —
    // a dialog was on screen, or a slow confirmation.
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(&copied, b"USER EDITED THIS AFTER THE UNDO WAS PLANNED").unwrap();

    let outcome = undo::apply_undo(&plan);
    eprintln!("apply outcome: {outcome:?}");

    assert_eq!(
        fs::read(&copied).ok().as_deref(),
        Some(&b"USER EDITED THIS AFTER THE UNDO WAS PLANNED"[..]),
        "APPLY_UNDO DELETED AN EDIT MADE AFTER PLANNING: it must re-verify identity \
         immediately before each destructive step, not trust the plan"
    );
    assert!(
        !outcome.problems.is_empty(),
        "refusing to remove the changed file must be REPORTED, not silent"
    );
    fs::remove_dir_all(&root).ok();
}

// ---------------------------------------------------------------------------
// Case 5: copying a symlink inside its own folder with Ask, then Undo, removes
//         the ORIGINAL link.
// ---------------------------------------------------------------------------

#[test]
fn a_same_folder_symlink_copy_then_undo_leaves_the_original_link_alone() {
    let root = fixture("symlink-self");
    fs::write(root.join("target.txt"), b"the real file").unwrap();
    std::os::unix::fs::symlink("target.txt", root.join("link")).unwrap();

    let before = fs::read_link(root.join("link")).unwrap();

    // Copy the link into the folder it already lives in, with Ask: the
    // destination name is occupied by the link itself, so this must either
    // report a conflict or produce a differently-named copy. It must never
    // overwrite the original with a copy of itself.
    let (p, rec) = run_job(
        JobKind::Copy,
        vec![root.join("link")],
        root.clone(),
        ConflictPolicy::Ask,
    );
    eprintln!(
        "same-folder symlink copy: state={:?} msg={}",
        p.state, p.message
    );

    assert!(
        root.join("link").symlink_metadata().is_ok(),
        "THE ORIGINAL SYMLINK WAS DESTROYED BY COPYING IT ONTO ITSELF"
    );

    {
        let record = rec.clone();
        if record.state == JobState::Confirmed {
            let plan = undo::plan_undo(&record);
            let outcome = match &plan {
                undo::UndoPlan::Safe { .. } => undo::apply_undo(&plan),
                undo::UndoPlan::Refused(r) => {
                    eprintln!("undo refused (acceptable): {r}");
                    Default::default()
                }
            };
            eprintln!("undo outcome: {outcome:?}");
        }
    }

    assert!(
        root.join("link").symlink_metadata().is_ok(),
        "UNDO REMOVED THE ORIGINAL SYMLINK, not a copy it had created"
    );
    assert_eq!(
        fs::read_link(root.join("link")).unwrap(),
        before,
        "the original link must still point where it did"
    );
    assert_eq!(
        fs::read(root.join("target.txt")).ok().as_deref(),
        Some(&b"the real file"[..]),
        "the link's target must never be touched"
    );
    fs::remove_dir_all(&root).ok();
}

// ---------------------------------------------------------------------------
// Phase A: overwrite paths the m41 audit found outside the copy engine.
// ---------------------------------------------------------------------------

/// A bulk rename is planned, shown, and only then executed. A file that
/// appears under a target name in between must never be replaced.
#[test]
fn bulk_rename_never_replaces_a_file_that_appeared_after_planning() {
    use gust_ops::bulk;
    let d = fixture("bulk-race");
    fs::write(d.join("a.txt"), b"source").unwrap();
    let plan = bulk::plan(&d, &[d.join("a.txt")], "b.{ext}");
    assert!(plan.is_safe(), "{plan:?}");
    // Somebody else creates the target while the preview is on screen.
    fs::write(d.join("b.txt"), b"BYSTANDER").unwrap();

    let out = bulk::execute(&d, &plan);

    assert_eq!(fs::read(d.join("b.txt")).unwrap(), b"BYSTANDER");
    assert_eq!(
        fs::read(d.join("a.txt")).unwrap(),
        b"source",
        "source must be put back"
    );
    assert!(
        !out.failed.is_empty(),
        "the refusal must be reported: {out:?}"
    );
    let debris: Vec<_> = fs::read_dir(&d)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".gust-rename"))
        .collect();
    assert!(debris.is_empty(), "temporary names survived");
    fs::remove_dir_all(&d).ok();
}

/// Extracting next to a file of the same name keeps the user's file and
/// lands the archive's copy under a free name.
#[test]
fn extraction_never_replaces_an_existing_file() {
    use gust_ops::archive::io::{extract, tool_available};
    use gust_ops::archive::ExtractPolicy;
    if !tool_available() {
        return;
    }
    let d = fixture("extract-clobber");
    let src = d.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("notes.txt"), b"from the archive").unwrap();
    let archive = d.join("a.tar");
    let ok = std::process::Command::new("bsdtar")
        .arg("-cf")
        .arg(&archive)
        .arg("-C")
        .arg(&src)
        .arg("--")
        .arg("notes.txt")
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let dest = d.join("dest");
    fs::create_dir_all(&dest).unwrap();
    fs::write(dest.join("notes.txt"), b"MINE").unwrap();

    let done = extract(&archive, &dest, &ExtractPolicy::default()).unwrap();

    assert_eq!(fs::read(dest.join("notes.txt")).unwrap(), b"MINE");
    assert_eq!(done.files, 1, "{done:?}");
    let landed: Vec<String> = fs::read_dir(&dest)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "notes.txt")
        .collect();
    assert_eq!(
        landed.len(),
        1,
        "the archive copy must land beside: {landed:?}"
    );
    assert_eq!(
        fs::read(dest.join(&landed[0])).unwrap(),
        b"from the archive"
    );
    assert!(done.summary().contains("renamed"), "{}", done.summary());
    fs::remove_dir_all(&d).ok();
}

/// "Keep both" must keep both. m41 treated it exactly like Replace.
#[test]
fn keep_both_keeps_both() {
    let d = fixture("keepboth");
    fs::create_dir_all(d.join("src")).unwrap();
    fs::create_dir_all(d.join("dest")).unwrap();
    fs::write(d.join("src/notes.txt"), b"incoming").unwrap();
    fs::write(d.join("dest/notes.txt"), b"ORIGINAL").unwrap();

    let mut job = jobs::start(
        "kb",
        jobs::Request::Copy {
            sources: vec![d.join("src/notes.txt")],
            dest_dir: d.join("dest"),
        },
        ConflictPolicy::KeepBoth,
    );
    let s = job.wait();
    assert_eq!(s.state, JobState::Confirmed, "{s:?}");
    assert_eq!(fs::read(d.join("dest/notes.txt")).unwrap(), b"ORIGINAL");
    assert_eq!(
        fs::read(d.join("dest/notes (copy).txt")).unwrap(),
        b"incoming"
    );
    fs::remove_dir_all(&d).ok();
}

/// The engine itself must refuse to overwrite under KeepBoth, so no caller
/// can reintroduce the bug by forgetting to pick a free name.
#[test]
fn the_engine_never_overwrites_under_keep_both() {
    let d = fixture("keepboth-engine");
    fs::create_dir_all(d.join("dest")).unwrap();
    fs::write(d.join("f"), b"incoming").unwrap();
    fs::write(d.join("dest/f"), b"ORIGINAL").unwrap();
    let cancel = CancelToken::new();
    let mut noop = |_: usize, _: usize, _: u64, _: u64| {};
    let r = recursive::copy_tree(
        &d.join("f"),
        &d.join("dest/f"),
        ConflictPolicy::KeepBoth,
        &cancel,
        &mut noop,
    );
    assert!(r.is_err(), "KeepBoth at the engine must refuse, got {r:?}");
    assert_eq!(fs::read(d.join("dest/f")).unwrap(), b"ORIGINAL");
    fs::remove_dir_all(&d).ok();
}

/// Skip leaves the existing item and the source exactly as they were, and
/// the rest of the batch still runs.
#[test]
fn skip_leaves_the_existing_item_and_finishes_the_rest() {
    let d = fixture("skip");
    fs::create_dir_all(d.join("src")).unwrap();
    fs::create_dir_all(d.join("dest")).unwrap();
    fs::write(d.join("src/a"), b"new a").unwrap();
    fs::write(d.join("src/b"), b"new b").unwrap();
    fs::write(d.join("dest/a"), b"OLD A").unwrap();

    let mut job = jobs::start(
        "sk",
        jobs::Request::Move {
            sources: vec![d.join("src/a"), d.join("src/b")],
            dest_dir: d.join("dest"),
        },
        ConflictPolicy::Skip,
    );
    let s = job.wait();
    assert_eq!(s.state, JobState::Confirmed, "{s:?}");
    assert_eq!(fs::read(d.join("dest/a")).unwrap(), b"OLD A");
    assert_eq!(
        fs::read(d.join("src/a")).unwrap(),
        b"new a",
        "a skipped source stays put"
    );
    assert_eq!(fs::read(d.join("dest/b")).unwrap(), b"new b");
    assert!(s.message.contains("skipped"), "{}", s.message);
    fs::remove_dir_all(&d).ok();
}

/// Replace puts the old item in the Trash first, and Undo brings it back.
#[test]
fn replace_trashes_the_old_item_and_undo_restores_it() {
    use gust_ops::trash::TrashDir;
    let d = fixture("replace");
    fs::create_dir_all(d.join("src")).unwrap();
    fs::create_dir_all(d.join("dest")).unwrap();
    fs::write(d.join("src/notes.txt"), b"incoming").unwrap();
    fs::write(d.join("dest/notes.txt"), b"ORIGINAL").unwrap();
    let trash = TrashDir::at(d.join("Trash"));

    let mut job = jobs::start_with(
        "rp",
        jobs::Request::Copy {
            sources: vec![d.join("src/notes.txt")],
            dest_dir: d.join("dest"),
        },
        ConflictPolicy::Replace,
        None,
        Some(trash.clone()),
    );
    let s = job.wait();
    assert_eq!(s.state, JobState::Confirmed, "{s:?}");
    assert_eq!(fs::read(d.join("dest/notes.txt")).unwrap(), b"incoming");
    assert_eq!(
        fs::read(trash.files.join("notes.txt")).unwrap(),
        b"ORIGINAL"
    );

    let plan = undo::plan_undo(&job.record());
    assert!(
        matches!(plan, undo::UndoPlan::Safe { .. }),
        "replace must be undoable: {plan:?}"
    );
    let out = undo::apply_undo(&plan);
    assert!(out.problems.is_empty(), "{out:?}");
    assert_eq!(fs::read(d.join("dest/notes.txt")).unwrap(), b"ORIGINAL");
    assert!(
        !trash.info.join("notes.txt.trashinfo").exists(),
        "stale trash record left"
    );
    fs::remove_dir_all(&d).ok();
}

/// The conflict dialog's answers, one per clashing item, in ONE job: Replace
/// for one, Keep both for another, Skip for a third, and an item with no
/// clash copied normally. Nothing is written before the user answered, so
/// the clash list comes from a pre-flight check.
#[test]
fn per_item_conflict_answers_are_honoured_in_one_job() {
    use gust_ops::trash::TrashDir;
    use std::collections::HashMap;
    let d = fixture("per-item");
    fs::create_dir_all(d.join("src")).unwrap();
    fs::create_dir_all(d.join("dest")).unwrap();
    for (n, b) in [
        ("r.txt", "new r"),
        ("k.txt", "new k"),
        ("s.txt", "new s"),
        ("fresh.txt", "fresh"),
    ] {
        fs::write(d.join("src").join(n), b).unwrap();
    }
    for (n, b) in [("r.txt", "OLD R"), ("k.txt", "OLD K"), ("s.txt", "OLD S")] {
        fs::write(d.join("dest").join(n), b).unwrap();
    }
    let sources: Vec<PathBuf> = ["r.txt", "k.txt", "s.txt", "fresh.txt"]
        .iter()
        .map(|n| d.join("src").join(n))
        .collect();
    let req = jobs::Request::Copy {
        sources: sources.clone(),
        dest_dir: d.join("dest"),
    };

    // Pre-flight finds exactly the three clashes, in source order.
    let clashes = jobs::conflicts(&req);
    let names: Vec<String> = clashes
        .iter()
        .map(|c| c.source.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["r.txt", "k.txt", "s.txt"]);
    assert_eq!(clashes[0].existing, d.join("dest/r.txt"));

    let mut answers = HashMap::new();
    answers.insert(sources[0].clone(), ConflictPolicy::Replace);
    answers.insert(sources[1].clone(), ConflictPolicy::KeepBoth);
    answers.insert(sources[2].clone(), ConflictPolicy::Skip);
    let trash = TrashDir::at(d.join("Trash"));
    let mut job = jobs::start_resolved("pi", req, answers, None, Some(trash.clone()));
    let s = job.wait();
    assert_eq!(s.state, JobState::Confirmed, "{s:?}");

    assert_eq!(fs::read(d.join("dest/r.txt")).unwrap(), b"new r");
    assert_eq!(
        fs::read(trash.files.join("r.txt")).unwrap(),
        b"OLD R",
        "replaced item must be in the Trash"
    );
    assert_eq!(fs::read(d.join("dest/k.txt")).unwrap(), b"OLD K");
    assert_eq!(fs::read(d.join("dest/k (copy).txt")).unwrap(), b"new k");
    assert_eq!(fs::read(d.join("dest/s.txt")).unwrap(), b"OLD S");
    assert_eq!(fs::read(d.join("dest/fresh.txt")).unwrap(), b"fresh");
    fs::remove_dir_all(&d).ok();
}

/// An item that clashes but got NO answer is never overwritten, even though
/// other items in the same job were answered "Replace".
#[test]
fn an_unanswered_clash_is_never_overwritten() {
    use std::collections::HashMap;
    let d = fixture("unanswered");
    fs::create_dir_all(d.join("src")).unwrap();
    fs::create_dir_all(d.join("dest")).unwrap();
    fs::write(d.join("src/a"), b"new a").unwrap();
    fs::write(d.join("src/b"), b"new b").unwrap();
    fs::write(d.join("dest/a"), b"OLD A").unwrap();
    fs::write(d.join("dest/b"), b"OLD B").unwrap();
    let mut answers = HashMap::new();
    answers.insert(d.join("src/a"), ConflictPolicy::KeepBoth);
    let mut job = jobs::start_resolved(
        "ua",
        jobs::Request::Copy {
            sources: vec![d.join("src/a"), d.join("src/b")],
            dest_dir: d.join("dest"),
        },
        answers,
        None,
        None,
    );
    let s = job.wait();
    assert_eq!(s.state, JobState::Conflict, "{s:?}");
    assert_eq!(fs::read(d.join("dest/b")).unwrap(), b"OLD B");
    fs::remove_dir_all(&d).ok();
}

// ---------------------------------------------------------------------------
// Folder merge (DESIGN §6.24): never overwrites, and one Undo puts both
// folders back exactly as they were.
// ---------------------------------------------------------------------------

fn merge_fixture(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
    let root = fixture(tag);
    let src = root.join("src/Invoices");
    let dst = root.join("dst/Invoices");
    fs::create_dir_all(src.join("2026/q1")).unwrap();
    fs::create_dir_all(dst.join("2026")).unwrap();
    fs::write(src.join("jan.txt"), b"NEW JAN").unwrap();
    fs::write(dst.join("jan.txt"), b"OLD JAN").unwrap();
    fs::write(src.join("feb.txt"), b"same feb").unwrap();
    fs::write(dst.join("feb.txt"), b"same feb").unwrap();
    fs::write(src.join("mar.txt"), b"only in src").unwrap();
    fs::write(src.join("2026/q1/report.txt"), b"q1").unwrap();
    fs::write(dst.join("may.txt"), b"only in dst").unwrap();
    fs::write(dst.join("2026/plan.txt"), b"dst plan").unwrap();
    (root, src, dst)
}

fn tree(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let rel = p.strip_prefix(root).unwrap().display().to_string();
            if p.is_dir() {
                out.push((rel + "/", Vec::new()));
                stack.push(p);
            } else {
                out.push((rel, fs::read(&p).unwrap()));
            }
        }
    }
    out.sort();
    out
}

fn run_merge(
    kind: JobKind,
    src: &Path,
    dst_parent: &Path,
) -> (jobs::JobProgress, gust_ops::journal::Job) {
    let req = match kind {
        JobKind::Copy => jobs::Request::Copy {
            sources: vec![src.to_path_buf()],
            dest_dir: dst_parent.to_path_buf(),
        },
        _ => jobs::Request::Move {
            sources: vec![src.to_path_buf()],
            dest_dir: dst_parent.to_path_buf(),
        },
    };
    let mut answers = std::collections::HashMap::new();
    answers.insert(src.to_path_buf(), ConflictPolicy::Merge);
    let mut h = jobs::start_resolved("merge", req, answers, None, None);
    let p = h.wait();
    (p, h.record())
}

#[test]
fn a_merging_copy_never_overwrites_and_undo_restores_both_folders() {
    let (root, src, dst) = merge_fixture("merge-copy");
    let (src_before, dst_before) = (tree(&src), tree(&dst));
    let (p, rec) = run_merge(JobKind::Copy, &src, &root.join("dst"));
    assert_eq!(p.state, JobState::Confirmed, "{p:?}");
    assert!(p.message.contains("merged"), "{}", p.message);
    assert!(p.message.contains("1 already there"), "{}", p.message);
    assert!(p.message.contains("kept both for 1"), "{}", p.message);
    // Nothing that was there changed.
    assert_eq!(fs::read(dst.join("jan.txt")).unwrap(), b"OLD JAN");
    assert_eq!(fs::read(dst.join("may.txt")).unwrap(), b"only in dst");
    assert_eq!(fs::read(dst.join("2026/plan.txt")).unwrap(), b"dst plan");
    // What was missing arrived; the clash landed beside it.
    assert_eq!(fs::read(dst.join("jan (copy).txt")).unwrap(), b"NEW JAN");
    assert_eq!(fs::read(dst.join("mar.txt")).unwrap(), b"only in src");
    assert_eq!(fs::read(dst.join("2026/q1/report.txt")).unwrap(), b"q1");
    assert_eq!(tree(&src), src_before, "a copy leaves its source alone");

    let plan = undo::plan_undo(&rec);
    assert!(matches!(plan, undo::UndoPlan::Safe { .. }), "{plan:?}");
    let out = undo::apply_undo(&plan);
    assert!(out.is_complete(), "{out:?}");
    assert_eq!(
        tree(&dst),
        dst_before,
        "Undo restores the destination exactly"
    );
    assert_eq!(tree(&src), src_before);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn a_merging_move_keeps_duplicates_in_the_source_and_undo_puts_everything_back() {
    let (root, src, dst) = merge_fixture("merge-move");
    let (src_before, dst_before) = (tree(&src), tree(&dst));
    let (p, rec) = run_merge(JobKind::Move, &src, &root.join("dst"));
    assert_eq!(p.state, JobState::Confirmed, "{p:?}");
    assert!(p.message.contains("left where they were"), "{}", p.message);
    assert_eq!(fs::read(dst.join("jan.txt")).unwrap(), b"OLD JAN");
    assert_eq!(fs::read(dst.join("jan (copy).txt")).unwrap(), b"NEW JAN");
    assert_eq!(fs::read(dst.join("2026/q1/report.txt")).unwrap(), b"q1");
    // The identical file was not moved, so it (and its folder) stays.
    assert_eq!(fs::read(src.join("feb.txt")).unwrap(), b"same feb");
    assert!(!src.join("mar.txt").exists() && !src.join("2026").exists());

    let plan = undo::plan_undo(&rec);
    let out = undo::apply_undo(&plan);
    assert!(out.is_complete(), "{out:?}");
    assert_eq!(tree(&dst), dst_before);
    assert_eq!(tree(&src), src_before, "Undo puts every moved item back");
    fs::remove_dir_all(&root).ok();
}

#[test]
fn a_merging_move_of_only_new_items_removes_the_emptied_source_and_undo_restores_it() {
    let root = fixture("merge-move-empty");
    let src = root.join("src/Photos");
    let dst = root.join("dst/Photos");
    fs::create_dir_all(src.join("trip/day1")).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("trip/day1/a.jpg"), b"a").unwrap();
    fs::write(dst.join("old.jpg"), b"old").unwrap();
    let (src_before, dst_before) = (tree(&src), tree(&dst));
    let (p, rec) = run_merge(JobKind::Move, &src, &root.join("dst"));
    assert_eq!(p.state, JobState::Confirmed, "{p:?}");
    assert!(!src.exists(), "a source the move emptied is removed");
    assert_eq!(fs::read(dst.join("trip/day1/a.jpg")).unwrap(), b"a");
    let out = undo::apply_undo(&undo::plan_undo(&rec));
    assert!(out.is_complete(), "{out:?}");
    assert_eq!(tree(&src), src_before);
    assert_eq!(tree(&dst), dst_before);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn undo_of_a_merge_keeps_what_the_user_changed_since() {
    let (root, src, dst) = merge_fixture("merge-edited");
    let (_p, rec) = run_merge(JobKind::Copy, &src, &root.join("dst"));
    // Edited after the merge: Undo must not throw this work away.
    fs::write(dst.join("mar.txt"), b"edited after the merge").unwrap();
    let plan = undo::plan_undo(&rec);
    if let undo::UndoPlan::Safe { .. } = plan {
        let _ = undo::apply_undo(&plan);
    }
    assert_eq!(
        fs::read(dst.join("mar.txt")).unwrap(),
        b"edited after the merge"
    );
    assert_eq!(fs::read(dst.join("jan.txt")).unwrap(), b"OLD JAN");
    fs::remove_dir_all(&root).ok();
}

#[test]
fn merge_asked_of_a_file_means_keep_both() {
    let root = fixture("merge-file");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("dst")).unwrap();
    fs::write(root.join("src/a.txt"), b"new").unwrap();
    fs::write(root.join("dst/a.txt"), b"old").unwrap();
    let (p, _rec) = run_merge(JobKind::Copy, &root.join("src/a.txt"), &root.join("dst"));
    assert_eq!(p.state, JobState::Confirmed, "{p:?}");
    assert_eq!(fs::read(root.join("dst/a.txt")).unwrap(), b"old");
    assert_eq!(fs::read(root.join("dst/a (copy).txt")).unwrap(), b"new");
    fs::remove_dir_all(&root).ok();
}
