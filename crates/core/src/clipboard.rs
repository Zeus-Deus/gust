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
    // The raw bytes, not a lossy string: a name that is not valid UTF-8
    // must still name the same file on the other side.
    let raw = path.as_os_str().as_encoded_bytes();
    let mut out = String::from("file://");
    for &b in raw {
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

/// Every type one copy is offered as, in order of preference.
///
/// A file manager reads the GNOME type (it carries copy-vs-cut), a browser
/// upload box or chat app reads `text/uri-list`, and a terminal or editor
/// reads plain text. All three are offered by ONE owner: `wl-copy` can hold
/// only a single type, and running it once per type left just the last.
pub fn offer_types(verb: ClipVerb, paths: &[PathBuf]) -> Vec<(String, Vec<u8>)> {
    vec![
        (
            "x-special/gnome-copied-files".into(),
            gnome_payload(verb, paths).into_bytes(),
        ),
        ("text/uri-list".into(), uri_list_payload(paths).into_bytes()),
        (
            "text/plain;charset=utf-8".into(),
            text_payload(paths).into_bytes(),
        ),
        ("text/plain".into(), text_payload(paths).into_bytes()),
        ("UTF8_STRING".into(), text_payload(paths).into_bytes()),
    ]
}

/// Who keeps the offer alive. A Wayland clipboard is served by the process
/// that set it, so something must stay running until another app copies.
pub enum Holder {
    /// A helper process (`gust --clip-serve`): the copy outlives the window,
    /// the way `wl-copy` did.
    Helper(PathBuf),
    /// A thread of this process. For tests and for when no helper exists.
    InProcess,
}

/// Offer `paths` to the rest of the desktop in every useful format.
///
/// Payloads never touch argv: a file list can be long and a file name can
/// contain anything.
pub fn offer(verb: ClipVerb, paths: &[PathBuf]) -> Result<(), String> {
    offer_with(verb, paths, &Holder::InProcess)
}

pub fn offer_with(verb: ClipVerb, paths: &[PathBuf], holder: &Holder) -> Result<(), String> {
    if paths.is_empty() {
        return Err("nothing to copy".into());
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return Err("no Wayland session".into());
    }
    let types = offer_types(verb, paths);
    match holder {
        Holder::InProcess => serve_types(types, false),
        Holder::Helper(exe) => {
            let mut child = Command::new(exe)
                .arg("--clip-serve")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| e.to_string())?;
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(&encode_types(&types))
                    .map_err(|e| e.to_string())?;
            }
            // Wait for "ready" so the offer exists when this returns: a
            // paste straight after Ctrl+C must not find the OLD clipboard.
            // The helper is then left running; it exits by itself as soon
            // as another application takes the clipboard.
            use std::io::Read;
            let mut ok = [0u8; 2];
            let ready = child
                .stdout
                .take()
                .map(|mut o| o.read_exact(&mut ok).is_ok() && &ok == b"ok")
                .unwrap_or(false);
            // Reap it whenever it exits, without blocking the caller.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            if ready {
                Ok(())
            } else {
                Err("the clipboard could not be set".into())
            }
        }
    }
}

/// `len\nmime\nbytes` records: binary-safe, no escaping.
fn encode_types(types: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (mime, data) in types {
        out.extend_from_slice(format!("{}\n{}\n", data.len(), mime).as_bytes());
        out.extend_from_slice(data);
    }
    out
}

fn decode_types(mut buf: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    while !buf.is_empty() {
        let nl = buf.iter().position(|b| *b == b'\n')?;
        let len: usize = std::str::from_utf8(&buf[..nl]).ok()?.parse().ok()?;
        buf = &buf[nl + 1..];
        let nl = buf.iter().position(|b| *b == b'\n')?;
        let mime = std::str::from_utf8(&buf[..nl]).ok()?.to_string();
        buf = &buf[nl + 1..];
        if buf.len() < len {
            return None;
        }
        out.push((mime, buf[..len].to_vec()));
        buf = &buf[len..];
    }
    Some(out)
}

fn serve_types(types: Vec<(String, Vec<u8>)>, foreground: bool) -> Result<(), String> {
    use wl_clipboard_rs::copy::{MimeSource, MimeType, Options, Source};
    let sources = types
        .into_iter()
        .map(|(mime, data)| MimeSource {
            source: Source::Bytes(data.into_boxed_slice()),
            mime_type: MimeType::Specific(mime),
        })
        .collect();
    let mut opts = Options::new();
    opts.foreground(foreground);
    if foreground {
        // Report readiness only once the offer is live, then serve until
        // another application replaces the clipboard.
        let prepared = opts
            .prepare_copy_multi(sources)
            .map_err(|e| e.to_string())?;
        let mut out = std::io::stdout();
        let _ = out.write_all(b"ok");
        let _ = out.flush();
        prepared.serve().map_err(|e| e.to_string())
    } else {
        opts.copy_multi(sources).map_err(|e| e.to_string())
    }
}

/// Entry point for `gust --clip-serve`: read the offer from stdin, hold the
/// clipboard until another application takes it, then exit.
pub fn serve_from_stdin() -> i32 {
    use std::io::Read;
    let mut buf = Vec::new();
    if std::io::stdin().read_to_end(&mut buf).is_err() {
        return 2;
    }
    let Some(types) = decode_types(&buf) else {
        return 2;
    };
    match serve_types(types, true) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// The text "Copy path" puts on the clipboard: one absolute path per line,
/// exactly as the shell would want them, with no trailing newline.
pub fn paths_as_text(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Put plain text on the desktop clipboard ("Copy path"). Like [`offer`],
/// the text goes in on stdin and `wl-copy` is left holding the offer.
pub fn offer_text(text: &str) -> Result<(), String> {
    if !system_clipboard_available() {
        return Err("wl-copy is not installed".into());
    }
    let mut child = Command::new("wl-copy")
        .arg("--type")
        .arg("text/plain;charset=utf-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Does the desktop clipboard hold files right now? One `wl-paste
/// --list-types` call: cheap enough to ask each time a menu opens, and it
/// reads no payload.
pub fn offers_files() -> bool {
    let Ok(out) = Command::new("wl-paste")
        .arg("--list-types")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return false;
    };
    types_include_files(&String::from_utf8_lossy(&out.stdout))
}

fn types_include_files(list: &str) -> bool {
    list.lines()
        .map(str::trim)
        .any(|t| t == "x-special/gnome-copied-files" || t == "text/uri-list")
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
mod codec_tests {
    use super::*;

    #[test]
    fn the_helper_protocol_carries_any_bytes() {
        let types = vec![
            ("text/uri-list".to_string(), b"file:///a\r\n".to_vec()),
            (
                "text/plain".to_string(),
                "x\n9\ny\u{202e}".as_bytes().to_vec(),
            ),
            ("empty".to_string(), Vec::new()),
        ];
        assert_eq!(decode_types(&encode_types(&types)), Some(types));
        assert_eq!(decode_types(b"99\ntext/plain\nshort"), None);
    }

    #[test]
    fn a_copy_offers_files_uris_and_text() {
        let t = offer_types(ClipVerb::Cut, &[PathBuf::from("/a b/c")]);
        let names: Vec<&str> = t.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(names[0], "x-special/gnome-copied-files");
        assert!(names.contains(&"text/uri-list") && names.contains(&"text/plain"));
        assert!(String::from_utf8_lossy(&t[0].1).starts_with("cut\n"));
    }
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

    #[test]
    fn copy_path_text_is_one_absolute_path_per_line() {
        let t = paths_as_text(&[PathBuf::from("/a/b c.txt"), PathBuf::from("/d")]);
        assert_eq!(t, "/a/b c.txt\n/d");
        assert_eq!(paths_as_text(&[]), "");
    }

    #[test]
    fn only_file_types_count_as_files_on_the_clipboard() {
        assert!(types_include_files("text/plain\ntext/uri-list\n"));
        assert!(types_include_files("x-special/gnome-copied-files"));
        assert!(!types_include_files(
            "text/plain;charset=utf-8\nUTF8_STRING\n"
        ));
        assert!(!types_include_files(""));
    }
}
