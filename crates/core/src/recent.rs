//! Recent files from `$XDG_DATA_HOME/recently-used.xbel`.
//!
//! A deliberately small reader: it pulls `href` and `modified` out of each
//! `<bookmark>` element and ignores everything else. No XML entity beyond
//! the five standard ones is expanded, so a hostile file cannot make it
//! fetch or expand anything. Read-only: other apps own this file.

use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recent {
    pub path: PathBuf,
    /// `YYYY-MM-DDThh:mm:ss` (UTC, as the file stores it).
    pub when: String,
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let start = tag.find(&key)? + key.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Newest first, one entry per file, local files only.
pub fn parse(xbel: &str) -> Vec<Recent> {
    let mut out: Vec<Recent> = Vec::new();
    for chunk in xbel.split("<bookmark").skip(1) {
        let Some(end) = chunk.find('>') else {
            continue;
        };
        let tag = &chunk[..end];
        let Some(href) = attr(tag, "href") else {
            continue;
        };
        let Some(path) = crate::clipboard::uri_to_path(&unescape(href)) else {
            continue;
        };
        let when = attr(tag, "modified")
            .or_else(|| attr(tag, "visited"))
            .or_else(|| attr(tag, "added"))
            .map(|w| w.get(..19).unwrap_or(w).to_string())
            .unwrap_or_default();
        out.push(Recent { path, when });
    }
    out.sort_by(|a, b| b.when.cmp(&a.when));
    let mut seen = HashSet::new();
    out.retain(|r| seen.insert(r.path.clone()));
    out
}

pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("recently-used.xbel"))
}

/// The recent list, bounded: the file can hold thousands of entries.
pub fn load(max: usize) -> Vec<Recent> {
    let Some(p) = default_path() else {
        return Vec::new();
    };
    // 16 MiB cap: a pathological file is not read in full.
    let Ok(md) = std::fs::metadata(&p) else {
        return Vec::new();
    };
    if md.len() > 16 * 1024 * 1024 {
        return Vec::new();
    }
    let text = std::fs::read_to_string(&p).unwrap_or_default();
    let mut v = parse(&text);
    v.truncate(max);
    v
}
