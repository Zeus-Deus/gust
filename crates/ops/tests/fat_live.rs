//! Copies onto a real FAT32 mount (tests/fat-fixture.sh). FAT32 cannot hold
//! a file of 4 GiB or more and rejects some characters in names; other file
//! managers fail at 99 % or drop the file. Gust refuses up front, and
//! renames instead of dropping.

use gust_ops::jobs::{start, Request};
use gust_ops::journal::{ConflictPolicy, JobState};
use std::fs;
use std::path::PathBuf;

fn fat() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os("GUST_FAT_FIXTURE")?);
    p.is_dir().then_some(p)
}

fn src(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("gust-fat-src-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_file_over_4_gib_is_refused_before_anything_is_written() {
    let Some(mnt) = fat() else {
        gust_testkit::skip!("FAT", "no FAT32 fixture (tests/fat-fixture.sh start)");
    };
    let s = src("big");
    // Sparse: 4 GiB + 1 byte of size, no real data written.
    let f = fs::File::create(s.join("film.mkv")).unwrap();
    f.set_len(0xFFFF_FFFF + 1).unwrap();
    fs::write(s.join("small.txt"), b"x").unwrap();
    let dest = mnt.join("big-test");
    fs::create_dir_all(&dest).unwrap();
    let mut h = start(
        "fat-big",
        Request::Copy {
            sources: vec![s.join("small.txt"), s.join("film.mkv")],
            dest_dir: dest.clone(),
        },
        ConflictPolicy::Ask,
    );
    let p = h.wait();
    assert_eq!(p.state, JobState::Failed, "{}", p.message);
    assert!(
        p.message.contains("film.mkv") && p.message.contains("Nothing was copied"),
        "{}",
        p.message
    );
    assert_eq!(
        fs::read_dir(&dest).unwrap().count(),
        0,
        "not even the small file was written"
    );
    fs::remove_dir_all(&dest).ok();
    fs::remove_dir_all(&s).ok();
}

#[test]
fn names_fat_cannot_store_are_renamed_not_dropped() {
    let Some(mnt) = fat() else {
        gust_testkit::skip!("FAT", "no FAT32 fixture (tests/fat-fixture.sh start)");
    };
    let s = src("names");
    fs::create_dir_all(s.join("Album: 2024")).unwrap();
    fs::write(s.join("Album: 2024/track?.mp3"), b"music").unwrap();
    fs::write(s.join("Album: 2024/ok.mp3"), b"fine").unwrap();
    let dest = mnt.join("names-test");
    fs::create_dir_all(&dest).unwrap();
    let mut h = start(
        "fat-names",
        Request::Copy {
            sources: vec![s.join("Album: 2024")],
            dest_dir: dest.clone(),
        },
        ConflictPolicy::Ask,
    );
    let p = h.wait();
    assert_eq!(
        p.state,
        JobState::Confirmed,
        "{} {:?}",
        p.message,
        p.refused
    );
    assert_eq!(
        fs::read(dest.join("Album_ 2024/track_.mp3")).unwrap(),
        b"music"
    );
    assert_eq!(fs::read(dest.join("Album_ 2024/ok.mp3")).unwrap(), b"fine");
    assert!(
        p.notes.iter().any(|n| n.contains("Renamed")),
        "the rename is reported: {:?}",
        p.notes
    );
    assert!(
        p.message.contains("Renamed"),
        "and shown on the transfer card: {}",
        p.message
    );
    fs::remove_dir_all(&dest).ok();
    fs::remove_dir_all(&s).ok();
}
