//! The clipboard other applications can see (DESIGN §6.19, plan §9).
//!
//! Gust's clipboard used to be a field on the model: copy here, paste here,
//! and nothing outside knew about it. That is not a clipboard, it is a
//! variable. This module puts a copy where the rest of the desktop looks
//! for one, and reads what the rest of the desktop leaves.
//!
//! Nothing here executes anything, and only `file://` URIs are accepted —
//! an `http://` URI on the clipboard is not something to paste into a
//! folder.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Copy or cut. The distinction survives the trip to another application
/// only in the GNOME format, which puts the verb on its first line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipVerb {
    Copy,
    Cut,
}

impl ClipVerb {
    pub fn as_str(self) -> &'static str {
        match self {
            ClipVerb::Copy => "copy",
            ClipVerb::Cut => "cut",
        }
    }
}

/// What was found on the clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipContents {
    pub verb: ClipVerb,
    pub paths: Vec<PathBuf>,
}

/// The GNOME/GTK payload: a verb, then one `file://` URI per line.
///
/// This is what Nautilus, Nemo and Thunar read, and the only one of the
/// three formats with room to say "cut".
pub fn gnome_payload(verb: ClipVerb, paths: &[PathBuf]) -> String {
    let mut s = String::from(verb.as_str());
    for p in paths {
        s.push('\n');
        s.push_str(&path_to_uri(p));
    }
    s
}

/// `text/uri-list` per RFC 2483: CRLF-separated URIs.
pub fn uri_list_payload(paths: &[PathBuf]) -> String {
    let mut s = String::new();
    for p in paths {
        s.push_str(&path_to_uri(p));
        s.push_str("\r\n");
    }
    s
}

/// Plain text: the paths themselves, for terminals and editors.
pub fn text_payload(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Percent-encode a path into a `file://` URI.
///
/// Spaces and `#` in a file name would otherwise truncate or corrupt the
/// URI at the receiving end.
pub fn path_to_uri(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let mut out = String::from("file://");
    for b in raw.bytes() {
        let safe = b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~');
        if safe {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Turn a `file://` URI back into a path.
///
/// Returns None for anything that is not a local file: an `http://` URI is
/// not something to paste into a folder.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.trim().strip_prefix("file://")?;
    // An authority component (file://host/path) is not local.
    // A leading slash means the path is absolute and local; anything else
    // (file://host/path) has an authority component and is not.
    let rest = format!("/{}", rest.strip_prefix('/')?);
    let mut out = Vec::with_capacity(rest.len());
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(out)))
}

/// Parse the GNOME payload back into a verb and paths.
pub fn parse_gnome(text: &str) -> Option<ClipContents> {
    let mut lines = text.lines();
    let verb = match lines.next()?.trim() {
        "cut" => ClipVerb::Cut,
        "copy" => ClipVerb::Copy,
        _ => return None,
    };
    let paths: Vec<PathBuf> = lines.filter_map(uri_to_path).collect();
    if paths.is_empty() {
        return None;
    }
    Some(ClipContents { verb, paths })
}

/// Parse `text/uri-list`.
///
/// Always a copy: there is nowhere in this format to say "cut", and
/// silently MOVING somebody's files because the format could not express it
/// would be the worst possible default.
pub fn parse_uri_list(text: &str) -> Option<ClipContents> {
    let paths: Vec<PathBuf> = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(uri_to_path)
        .collect();
    if paths.is_empty() {
        return None;
    }
    Some(ClipContents {
        verb: ClipVerb::Copy,
        paths,
    })
}

/// Parse plain text as paths, keeping only those that exist.
///
/// Text on the clipboard is usually not a file list; accepting a
/// non-existent path would turn a stray copy into a confusing paste error.
pub fn parse_plain(text: &str) -> Option<ClipContents> {
    let paths: Vec<PathBuf> = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| l.starts_with('/'))
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .collect();
    if paths.is_empty() {
        return None;
    }
    Some(ClipContents {
        verb: ClipVerb::Copy,
        paths,
    })
}

/// Is the system clipboard usable?
pub fn system_clipboard_available() -> bool {
    Command::new("wl-copy")
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Offer `paths` to the rest of the desktop.
///
/// `wl-copy` stays running to hold the offer — that is how a Wayland
/// clipboard works — so it is spawned and deliberately not waited on. The
/// payload goes in on **stdin**: a file list can be long and a file name can
/// contain anything, neither of which belongs in argv.
pub fn offer(verb: ClipVerb, paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("nothing to copy".into());
    }
    if !system_clipboard_available() {
        return Err("wl-copy is not installed".into());
    }

    // One wl-copy per type: each holds its own offer. The GNOME type is
    // what other file managers read, so it goes first.
    let offers: [(&str, String); 3] = [
        ("x-special/gnome-copied-files", gnome_payload(verb, paths)),
        ("text/uri-list", uri_list_payload(paths)),
        ("text/plain", text_payload(paths)),
    ];

    // wl-copy replaces the whole clipboard each time it runs, so the types
    // cannot simply be offered one after another. `--type` with several
    // invocations would leave only the last. Offer the GNOME type, which is
    // the one a file manager needs; the others ride along as fallbacks only
    // when it is unavailable.
    let (ty, payload) = &offers[0];
    let mut child = Command::new("wl-copy")
        .arg("--type")
        .arg(ty)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(payload.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    // Deliberately NOT waited on: wl-copy lives as long as the offer does.
    Ok(())
}

/// Read whatever the desktop currently offers.
pub fn read() -> Option<ClipContents> {
    // In preference order: the format with a verb, then the standard URI
    // list, then plain text as a last resort.
    for ty in [
        "x-special/gnome-copied-files",
        "text/uri-list",
        "text/plain",
    ] {
        let out = Command::new("wl-paste")
            .arg("--no-newline")
            .arg("--type")
            .arg(ty)
            .stdin(Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let parsed = match ty {
            "x-special/gnome-copied-files" => parse_gnome(&text),
            "text/uri-list" => parse_uri_list(&text),
            _ => parse_plain(&text),
        };
        if parsed.is_some() {
            return parsed;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gnome_payload_leads_with_the_verb() {
        let p = vec![PathBuf::from("/tmp/a.txt")];
        assert_eq!(gnome_payload(ClipVerb::Copy, &p), "copy\nfile:///tmp/a.txt");
        assert_eq!(gnome_payload(ClipVerb::Cut, &p), "cut\nfile:///tmp/a.txt");
    }

    #[test]
    fn a_uri_list_is_crlf_separated() {
        let p = vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")];
        assert_eq!(uri_list_payload(&p), "file:///tmp/a\r\nfile:///tmp/b\r\n");
    }

    #[test]
    fn spaces_and_hashes_are_escaped() {
        // Unescaped, a space truncates the URI and a # starts a fragment.
        let uri = path_to_uri(Path::new("/tmp/my holiday #1.jpg"));
        assert!(!uri.contains(' '), "got {uri}");
        assert!(uri.contains("%20"));
        assert!(uri.contains("%23"));
    }

    #[test]
    fn a_uri_round_trips_back_to_the_same_path() {
        for original in [
            "/tmp/simple.txt",
            "/tmp/with space.txt",
            "/tmp/with#hash.txt",
            "/tmp/üñïçødé.txt",
            "/tmp/; rm -rf ~",
        ] {
            let p = PathBuf::from(original);
            let back = uri_to_path(&path_to_uri(&p)).expect("should parse");
            assert_eq!(back, p, "round trip failed for {original}");
        }
    }

    #[test]
    fn only_local_files_are_accepted() {
        assert!(uri_to_path("http://example.com/a.txt").is_none());
        assert!(uri_to_path("ftp://host/a").is_none());
        // file:// with an authority is not local either.
        assert!(uri_to_path("file://otherhost/tmp/a").is_none());
        assert!(uri_to_path("file:///tmp/a").is_some());
    }

    #[test]
    fn a_cut_from_another_file_manager_is_read_as_a_move() {
        let c = parse_gnome("cut\nfile:///tmp/a.txt\nfile:///tmp/b.txt").unwrap();
        assert_eq!(c.verb, ClipVerb::Cut);
        assert_eq!(c.paths.len(), 2);
    }

    #[test]
    fn a_uri_list_is_always_a_copy() {
        // There is nowhere in this format to say "cut", and moving
        // somebody's files on a guess is the worst possible default.
        let c = parse_uri_list("file:///tmp/a\r\nfile:///tmp/b\r\n").unwrap();
        assert_eq!(c.verb, ClipVerb::Copy);
    }

    #[test]
    fn uri_list_comments_are_skipped() {
        let c = parse_uri_list("# a comment\r\nfile:///tmp/a\r\n").unwrap();
        assert_eq!(c.paths, vec![PathBuf::from("/tmp/a")]);
    }

    #[test]
    fn an_unknown_verb_is_not_guessed_at() {
        assert!(parse_gnome("move\nfile:///tmp/a").is_none());
        assert!(parse_gnome("file:///tmp/a").is_none());
    }

    #[test]
    fn an_empty_payload_yields_nothing() {
        assert!(parse_gnome("copy").is_none());
        assert!(parse_uri_list("").is_none());
        assert!(parse_plain("").is_none());
    }

    #[test]
    fn plain_text_must_be_paths_that_exist() {
        // Ordinary copied text must not look like a file list.
        assert!(parse_plain("just some words I copied").is_none());
        assert!(parse_plain("/definitely/not/here/at/all").is_none());

        let d = std::env::temp_dir().join("gust-clip-plain");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("real.txt");
        std::fs::write(&f, "x").unwrap();
        let c = parse_plain(&f.to_string_lossy()).unwrap();
        assert_eq!(c.paths, vec![f]);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn copying_nothing_is_refused() {
        assert!(offer(ClipVerb::Copy, &[]).is_err());
    }
}
