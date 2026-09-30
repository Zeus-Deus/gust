//! What the transfer card shows while a copy runs: bytes moving, a speed and
//! a time left. Written against the production engine, not a helper.

use gust_core::lister::CancelToken;
use gust_ops::journal::ConflictPolicy;
use gust_ops::rate::{format_eta, format_rate, RateMeter};
use gust_ops::recursive;
use std::fs;
use std::path::PathBuf;

fn fixture(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "gust-progress-{tag}-{}-{}",
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

/// A single large file must report bytes WHILE it copies. m41 reported
/// only between files, so one 4 GB video sat at 0 % and then jumped to
/// 100 % — no speed, no time left, and no way to tell it from a hang.
#[test]
fn a_large_file_reports_bytes_while_it_copies() {
    let d = fixture("big");
    let size = 8 * 1024 * 1024u64;
    fs::write(d.join("big.bin"), vec![7u8; size as usize]).unwrap();
    fs::create_dir_all(d.join("dest")).unwrap();

    let mut seen: Vec<u64> = Vec::new();
    let mut cb = |_: usize, _: usize, bytes: u64, _: u64| seen.push(bytes);
    let cancel = CancelToken::new();
    recursive::copy_tree(
        &d.join("big.bin"),
        &d.join("dest/big.bin"),
        ConflictPolicy::Ask,
        &cancel,
        &mut cb,
    )
    .unwrap();

    let mid: Vec<u64> = seen
        .iter()
        .copied()
        .filter(|b| *b > 0 && *b < size)
        .collect();
    assert!(
        mid.len() >= 4,
        "expected progress inside the file, got {seen:?}"
    );
    assert!(
        seen.windows(2).all(|w| w[0] <= w[1]),
        "bytes must never go backwards: {seen:?}"
    );
    assert_eq!(*seen.last().unwrap(), size);
    fs::remove_dir_all(&d).ok();
}

/// The same inside a folder: a big file among small ones still moves the bar.
#[test]
fn a_folder_copy_reports_bytes_inside_each_file() {
    let d = fixture("tree");
    fs::create_dir_all(d.join("src")).unwrap();
    fs::write(d.join("src/a.txt"), b"small").unwrap();
    fs::write(d.join("src/big.bin"), vec![1u8; 4 * 1024 * 1024]).unwrap();
    let total = 5 + 4 * 1024 * 1024u64;

    let mut seen: Vec<u64> = Vec::new();
    let mut cb = |_: usize, _: usize, bytes: u64, _: u64| seen.push(bytes);
    recursive::copy_tree(
        &d.join("src"),
        &d.join("dst"),
        ConflictPolicy::Ask,
        &CancelToken::new(),
        &mut cb,
    )
    .unwrap();
    let distinct: std::collections::BTreeSet<u64> = seen.iter().copied().collect();
    assert!(
        distinct.len() >= 6,
        "expected several byte counts, got {distinct:?}"
    );
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
    assert_eq!(*seen.last().unwrap(), total);
    fs::remove_dir_all(&d).ok();
}

#[test]
fn the_speed_is_bytes_over_the_recent_window() {
    let mut m = RateMeter::new();
    assert_eq!(m.bytes_per_sec(), None, "no speed before two samples");
    m.observe(0.0, 0);
    m.observe(1.0, 10_000_000);
    m.observe(2.0, 20_000_000);
    let r = m.bytes_per_sec().unwrap();
    assert!((r - 10_000_000.0).abs() < 1.0, "{r}");
}

#[test]
fn an_old_burst_does_not_hold_the_speed_up() {
    // Fast for one second, then a stall: the window must follow the stall.
    let mut m = RateMeter::new();
    m.observe(0.0, 0);
    m.observe(1.0, 100_000_000);
    for t in 2..=8 {
        m.observe(t as f64, 100_000_000);
    }
    assert_eq!(m.bytes_per_sec(), Some(0.0));
    assert_eq!(
        m.eta_secs(1_000),
        None,
        "a stalled transfer has no honest ETA"
    );
}

#[test]
fn the_time_left_follows_the_speed() {
    let mut m = RateMeter::new();
    m.observe(0.0, 0);
    m.observe(2.0, 20_000_000);
    assert_eq!(m.eta_secs(50_000_000), Some(5));
}

#[test]
fn a_brand_new_transfer_waits_before_guessing() {
    // Two samples 50 ms apart are noise, not a speed.
    let mut m = RateMeter::new();
    m.observe(0.0, 0);
    m.observe(0.05, 3_000_000);
    assert_eq!(m.bytes_per_sec(), None);
}

#[test]
fn speeds_and_times_read_like_a_person_wrote_them() {
    assert_eq!(format_rate(512.0), "512 B/s");
    assert_eq!(format_rate(45.0 * 1024.0 * 1024.0), "45 MB/s");
    assert_eq!(format_rate(1.5 * 1024.0 * 1024.0), "1.5 MB/s");
    assert_eq!(format_eta(4), "a few seconds left");
    assert_eq!(format_eta(42), "42 s left");
    assert_eq!(format_eta(150), "about 3 min left");
    assert_eq!(format_eta(2 * 3600 + 600), "about 2 h 10 min left");
}
