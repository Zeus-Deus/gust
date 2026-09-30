//! The facts the Properties view shows about a path, gathered without
//! following links and without running anything.

use gust_core::properties::{describe, full_time, owner_names, relative_time, Fact};
use std::fs;
use std::os::unix::fs::symlink;

fn get<'a>(facts: &'a [Fact], label: &str) -> Option<&'a str> {
    facts
        .iter()
        .find(|f| f.label == label)
        .map(|f| f.value.as_str())
}

#[test]
fn a_file_shows_where_it_is_when_it_changed_and_who_owns_it() {
    let d = std::env::temp_dir().join(format!("gust-props-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("a.txt"), b"hello").unwrap();
    let f = describe(&d.join("a.txt"));
    assert_eq!(get(&f, "Location"), Some(d.to_string_lossy().as_ref()));
    assert!(get(&f, "Modified").is_some_and(|m| m.len() == 16), "{f:?}"); // 2026-09-24 16:40
    assert!(get(&f, "Owner").is_some());
    assert!(get(&f, "Permissions").is_some_and(|p| p.starts_with("-rw")));
    assert_eq!(get(&f, "Link to"), None);
    fs::remove_dir_all(&d).ok();
}

#[test]
fn a_symlink_says_where_it_points_and_is_not_followed() {
    let d = std::env::temp_dir().join(format!("gust-props-link-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    symlink("/etc/shadow", d.join("sneaky")).unwrap();
    let f = describe(&d.join("sneaky"));
    assert_eq!(get(&f, "Link to"), Some("/etc/shadow"));
    assert!(get(&f, "Permissions").is_some_and(|p| p.starts_with('l')));
    fs::remove_dir_all(&d).ok();
}

#[test]
fn a_hostile_link_target_is_shown_not_obeyed() {
    let d = std::env::temp_dir().join(format!("gust-props-esc-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    symlink("x\u{1b}[31mred\u{202e}", d.join("esc")).unwrap();
    let f = describe(&d.join("esc"));
    let t = get(&f, "Link to").unwrap();
    assert!(!t.contains('\u{1b}') && !t.contains('\u{202e}'), "{t:?}");
    fs::remove_dir_all(&d).ok();
}

#[test]
fn times_are_absolute_and_local() {
    // 2021-03-04 05:06 UTC; the value depends on TZ, so only the shape and
    // the date-only prefix under UTC are pinned.
    let s = full_time(1_614_834_360, 0);
    assert_eq!(s, "2021-03-04 05:06");
    assert_eq!(full_time(0, 3600), "1970-01-01 01:00");
}

#[test]
fn owners_are_names_from_the_local_databases() {
    let passwd = "root:x:0:0::/root:/bin/bash\nuser:x:1000:1000::/home/user:/bin/zsh\n";
    let group = "root:x:0:\nwheel:x:998:user\nuser:x:1000:\n";
    assert_eq!(owner_names(1000, 998, passwd, group), "user : wheel");
    assert_eq!(owner_names(4242, 4243, passwd, group), "4242 : 4243");
}

#[test]
fn non_english_names_are_shown_as_written() {
    let d = std::env::temp_dir().join(format!("gust-props-uni-{}/Música 写真", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("x"), b"1").unwrap();
    let f = describe(&d.join("x"));
    assert!(
        get(&f, "Location").unwrap().ends_with("/Música 写真"),
        "{f:?}"
    );
}

#[test]
fn the_list_shows_local_time_not_utc() {
    // Found live in CEST (UTC+2): a file saved at 16:49 was listed as
    // "today 14:49" — the clock said one thing, the list another.
    let now = 1_790_261_400; // 2026-09-24 14:50 UTC = 16:50 CEST
    let saved = now - 60;
    assert_eq!(relative_time(saved, now, 7200), "today 16:49");
    assert_eq!(relative_time(saved, now, 0), "today 14:49");
}

#[test]
fn today_and_yesterday_follow_the_local_calendar() {
    // 00:30 local on the 24th; a file from 23:50 local on the 23rd is
    // "yesterday", even though it is only 40 minutes old.
    let offset = 7200;
    let now = 1_790_202_600; // 2026-09-23 22:30 UTC = 2026-09-24 00:30 CEST
    assert_eq!(relative_time(now - 40 * 60, now, offset), "yesterday 23:50");
    assert_eq!(relative_time(now - 10 * 60, now, offset), "today 00:20");
    // Older dates are written out, so two files are comparable at a glance.
    assert_eq!(relative_time(now - 10 * 86_400, now, offset), "2026-09-14");
}
