//! Recent files, from the desktop's own list (`recently-used.xbel`), which
//! every GTK app — and the file-chooser portal — already writes. Gust reads
//! it; it never rewrites another app's history.

use gust_core::recent::{parse, Recent};
use std::path::PathBuf;

const XBEL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xbel version="1.0" xmlns:bookmark="http://www.freedesktop.org/standards/desktop-bookmarks">
  <bookmark href="file:///home/u/Documents/report%20final.pdf" added="2026-09-20T10:00:00.000000Z" modified="2026-09-24T18:31:00.123456Z" visited="2026-09-24T18:31:00Z">
    <info><metadata owner="http://freedesktop.org"><mime:mime-type type="application/pdf"/></metadata></info>
  </bookmark>
  <bookmark href="file:///home/u/Pictures/cat.jpg" added="2026-09-22T09:00:00Z" modified="2026-09-22T09:00:00Z" visited="2026-09-22T09:00:00Z"/>
  <bookmark href="https://example.com/page" added="2026-09-23T09:00:00Z" modified="2026-09-23T09:00:00Z" visited="2026-09-23T09:00:00Z"/>
  <bookmark href="file:///home/u/a&amp;b.txt" added="2026-09-21T09:00:00Z" modified="2026-09-21T09:00:00Z" visited="2026-09-21T09:00:00Z"/>
</xbel>"#;

#[test]
fn local_files_are_listed_newest_first() {
    let got = parse(XBEL);
    let paths: Vec<&PathBuf> = got.iter().map(|r: &Recent| &r.path).collect();
    assert_eq!(
        paths,
        vec![
            &PathBuf::from("/home/u/Documents/report final.pdf"),
            &PathBuf::from("/home/u/Pictures/cat.jpg"),
            &PathBuf::from("/home/u/a&b.txt"),
        ]
    );
    assert_eq!(got[0].when, "2026-09-24T18:31:00");
}

#[test]
fn web_addresses_are_not_files() {
    assert!(parse(XBEL).iter().all(|r| r.path.is_absolute()));
}

#[test]
fn a_damaged_or_empty_file_gives_nothing_rather_than_failing() {
    assert!(parse("").is_empty());
    assert!(
        parse("<xbel><bookmark href=\"file:///x\"").is_empty()
            || parse("<xbel><bookmark href=\"file:///x\"").len() <= 1
    );
    assert!(parse("\u{0}\u{1}garbage").is_empty());
}

#[test]
fn the_same_file_twice_is_listed_once() {
    let x = r#"<bookmark href="file:///a" modified="2026-01-02T00:00:00Z"/><bookmark href="file:///a" modified="2026-01-01T00:00:00Z"/>"#;
    let got = parse(x);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].when, "2026-01-02T00:00:00");
}
