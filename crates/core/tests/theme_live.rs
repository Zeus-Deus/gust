//! Live theme reload: switching the Omarchy theme repaints Gust without a
//! restart. Written against the real switch sequence in
//! `/usr/bin/omarchy-theme-set` (Omarchy 4.0.3): build `current/next-theme`,
//! then `rm -rf current/theme` and `mv current/next-theme current/theme`.

use gust_core::theme::{theme_watch_dirs, theme_ready};
use gust_core::watch::{Change, DirWatch};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

fn state(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("gust-theme-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(d.join("current/theme")).unwrap();
    fs::write(d.join("current/theme/colors.toml"), "background = \"#111111\"\n").unwrap();
    d
}

fn omarchy_theme_set(state: &Path, bg: &str) {
    let cur = state.join("current");
    let next = cur.join("next-theme");
    fs::create_dir_all(&next).unwrap();
    fs::write(next.join("colors.toml"), format!("background = \"{bg}\"\n")).unwrap();
    fs::remove_dir_all(cur.join("theme")).unwrap();
    fs::rename(&next, cur.join("theme")).unwrap();
}

#[test]
fn the_folder_omarchy_replaces_is_among_those_watched() {
    let s = state("dirs");
    let dirs = theme_watch_dirs(&s);
    assert!(dirs.contains(&s.join("current")), "{dirs:?}");
    // `current` itself may be a symlink on other Omarchy versions and be
    // swapped atomically, which only a watch on its parent sees.
    assert!(dirs.contains(&s), "{dirs:?}");
    fs::remove_dir_all(&s).ok();
}

#[test]
fn a_theme_switch_is_noticed_and_reads_the_new_colours() {
    let s = state("switch");
    let (tx, rx) = mpsc::channel();
    let _watches: Vec<DirWatch> = theme_watch_dirs(&s)
        .into_iter()
        .filter_map(|d| {
            let tx = tx.clone();
            DirWatch::start(&d, move |c| {
                let _ = tx.send(c);
            })
            .ok()
        })
        .collect();

    omarchy_theme_set(&s, "#eeeeee");
    let c = rx.recv_timeout(Duration::from_secs(2)).expect("switch noticed");
    assert_eq!(c, Change::Contents);
    assert!(theme_ready(&s));
    let t = gust_core::theme::load_from_theme_dir(&s.join("current/theme"), "x");
    assert_eq!(t.color("background"), "#eeeeee");
    fs::remove_dir_all(&s).ok();
}

#[test]
fn half_way_through_a_switch_is_not_ready() {
    // Between `rm -rf theme` and `mv next-theme theme` there is no theme. A
    // reload there would paint the fallback palette for a frame: skip it and
    // wait for the move.
    let s = state("half");
    fs::remove_dir_all(s.join("current/theme")).unwrap();
    assert!(!theme_ready(&s));
    fs::remove_dir_all(&s).ok();
}
