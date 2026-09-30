//! Live folder updates: the watcher tells Gust when the folder on screen
//! changed underneath it (another program saved, deleted or renamed a file).
//!
//! Written against the production watcher before it existed.

use gust_core::watch::{row_edits, Change, DirWatch, RowEdit};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("gust-watch-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn start(dir: &Path) -> (DirWatch, mpsc::Receiver<Change>) {
    let (tx, rx) = mpsc::channel();
    let w = DirWatch::start(dir, move |c| {
        let _ = tx.send(c);
    })
    .expect("a local folder can be watched");
    (w, rx)
}

/// Every notification that arrives within `window`.
fn drain(rx: &mpsc::Receiver<Change>, window: Duration) -> Vec<Change> {
    let end = Instant::now() + window;
    let mut out = Vec::new();
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(c) => out.push(c),
            Err(_) => break,
        }
    }
    out
}

#[test]
fn a_file_created_by_another_program_is_noticed_quickly() {
    let d = tmp("create");
    let (_w, rx) = start(&d);
    let t = Instant::now();
    fs::write(d.join("report.pdf"), b"x").unwrap();
    let c = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("change reported");
    assert_eq!(c, Change::Contents);
    assert!(
        t.elapsed() < Duration::from_millis(800),
        "took {:?}",
        t.elapsed()
    );
    fs::remove_dir_all(&d).ok();
}

#[test]
fn delete_and_rename_are_noticed() {
    let d = tmp("delren");
    fs::write(d.join("a"), b"1").unwrap();
    fs::write(d.join("b"), b"2").unwrap();
    let (_w, rx) = start(&d);
    fs::remove_file(d.join("a")).unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        Change::Contents
    );
    drain(&rx, Duration::from_millis(400));
    fs::rename(d.join("b"), d.join("c")).unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        Change::Contents
    );
    fs::remove_dir_all(&d).ok();
}

#[test]
fn a_burst_of_changes_is_one_or_two_refreshes_not_thousands() {
    // Unpacking an archive or a build writing into the folder must not make
    // Gust relist it once per file.
    let d = tmp("burst");
    let (_w, rx) = start(&d);
    for i in 0..2000 {
        fs::write(d.join(format!("f{i:04}")), b"x").unwrap();
    }
    let got = drain(&rx, Duration::from_millis(1500));
    assert!(!got.is_empty(), "the burst was noticed");
    assert!(
        got.len() <= 4,
        "burst coalesced, got {} notifications",
        got.len()
    );
    fs::remove_dir_all(&d).ok();
}

#[test]
fn a_file_being_written_does_not_refresh_on_every_chunk() {
    // A download or a copy writes in many chunks. Only its appearance and
    // its completion matter to the listing.
    let d = tmp("chunks");
    let mut f = fs::File::create(d.join("big.iso")).unwrap();
    let (_w, rx) = start(&d);
    use std::io::Write;
    for _ in 0..200 {
        f.write_all(&[0u8; 4096]).unwrap();
        f.flush().unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    let during = drain(&rx, Duration::from_millis(300));
    assert!(
        during.is_empty(),
        "chunk writes caused {} refreshes",
        during.len()
    );
    drop(f); // close after writing: the size is final now
    let c = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("completion noticed");
    assert_eq!(c, Change::Contents);
    fs::remove_dir_all(&d).ok();
}

#[test]
fn the_folder_itself_disappearing_is_reported_as_gone() {
    let d = tmp("gone");
    let (_w, rx) = start(&d);
    fs::remove_dir_all(&d).unwrap();
    let got = drain(&rx, Duration::from_millis(1500));
    assert!(got.contains(&Change::Gone), "got {got:?}");
}

#[test]
fn the_folder_being_moved_away_is_reported_as_gone() {
    let d = tmp("moved");
    let away = d.with_extension("away");
    let _ = fs::remove_dir_all(&away);
    let (_w, rx) = start(&d);
    fs::rename(&d, &away).unwrap();
    let got = drain(&rx, Duration::from_millis(1500));
    assert!(got.contains(&Change::Gone), "got {got:?}");
    fs::remove_dir_all(&away).ok();
}

#[test]
fn stopping_the_watch_ends_its_thread_and_silences_it() {
    let d = tmp("stop");
    let (w, rx) = start(&d);
    let t = Instant::now();
    drop(w);
    assert!(
        t.elapsed() < Duration::from_millis(500),
        "stop took {:?}",
        t.elapsed()
    );
    fs::write(d.join("after"), b"x").unwrap();
    assert!(drain(&rx, Duration::from_millis(600)).is_empty());
    fs::remove_dir_all(&d).ok();
}

#[test]
fn a_missing_folder_cannot_be_watched() {
    let d = std::env::temp_dir().join(format!("gust-watch-nope-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    assert!(DirWatch::start(&d, |_| {}).is_err());
}

// --- turning a relisted folder into row edits -----------------------------
//
// A live refresh must not reset the view: a reset throws away the scroll
// position and every delegate, so the list would jump to the top whenever
// another program saved a file. The refreshed listing is instead turned into
// row removals and insertions that the view can animate in place.

fn names(v: &[&str]) -> Vec<OsString> {
    v.iter().map(OsString::from).collect()
}

/// Apply edits the way the Qt model will, and return the result.
fn replay(old: &[OsString], new: &[OsString], edits: &[RowEdit]) -> Vec<OsString> {
    let mut rows = old.to_vec();
    for e in edits {
        match *e {
            RowEdit::Remove(i) => {
                rows.remove(i);
            }
            RowEdit::Insert { at, from_new } => rows.insert(at, new[from_new].clone()),
        }
    }
    rows
}

#[test]
fn row_edits_turn_the_old_rows_into_the_new_ones() {
    let cases: &[(&[&str], &[&str])] = &[
        (&[], &[]),
        (&["a", "b", "c"], &["a", "b", "c"]),
        (&["a", "c"], &["a", "b", "c"]),      // created
        (&["a", "b", "c"], &["a", "c"]),      // deleted
        (&["a", "b", "c"], &["a", "c", "d"]), // renamed b -> d
        (&["b", "c", "a"], &["a", "b", "c"]), // re-sorted (mtime changed)
        (&["a", "b", "c", "d"], &["d", "c", "b", "a"]),
        (&[], &["x", "y"]),
        (&["x", "y"], &[]),
        (&["a", "b", "c", "d", "e"], &["b", "x", "d", "a", "y"]),
    ];
    for (old, new) in cases {
        let (old, new) = (names(old), names(new));
        let edits = row_edits(&old, &new);
        assert_eq!(
            replay(&old, &new, &edits),
            new,
            "{old:?} -> {new:?} via {edits:?}"
        );
    }
}

#[test]
fn an_unchanged_folder_needs_no_edits() {
    let v = names(&["a", "b", "c"]);
    assert!(row_edits(&v, &v).is_empty());
}

#[test]
fn one_new_file_is_one_insert_and_nothing_else_moves() {
    let old = names(&["a", "b", "d", "e"]);
    let new = names(&["a", "b", "c", "d", "e"]);
    assert_eq!(
        row_edits(&old, &new),
        vec![RowEdit::Insert { at: 2, from_new: 2 }]
    );
}

#[test]
fn one_deleted_file_is_one_remove() {
    let old = names(&["a", "b", "c"]);
    let new = names(&["a", "c"]);
    assert_eq!(row_edits(&old, &new), vec![RowEdit::Remove(1)]);
}

#[test]
fn row_edits_hold_for_many_pseudo_random_changes() {
    // A small deterministic generator: no extra dependency for a fuzz loop.
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..500 {
        let pool: Vec<String> = (0..12).map(|i| format!("n{i}")).collect();
        let pick = |r: u64| -> Vec<OsString> {
            let mut v: Vec<OsString> = pool
                .iter()
                .enumerate()
                .filter(|(i, _)| r >> i & 1 == 1)
                .map(|(_, s)| OsString::from(s))
                .collect();
            // Shuffle a little, as a re-sort would.
            let k = v.len();
            for i in 0..k {
                let j = (r as usize >> (i % 8)) % k;
                v.swap(i, j);
            }
            v
        };
        let old = pick(next());
        let new = pick(next());
        let edits = row_edits(&old, &new);
        assert_eq!(replay(&old, &new, &edits), new, "{old:?} -> {new:?}");
    }
}
