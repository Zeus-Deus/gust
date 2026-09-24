//! Quick Look — what a file *is*, without running it (DESIGN §6.9, plan §4).
//!
//! Everything here is a fact Gust can state by looking at bytes and metadata.
//! Nothing is executed, nothing is interpreted as markup or code, and the
//! peek is bounded so a 4 GB log opens instantly.

use crate::mime::{category_for_name, Category};
use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;

/// At most this much of a file is read for a text peek.
pub const MAX_PEEK_BYTES: u64 = 64 * 1024;
/// At most this many lines are shown.
pub const MAX_PEEK_LINES: usize = 40;

/// What kind of thing this is, in words a person uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeekKind {
    Folder,
    /// A symlink, and where it points. Shown as a fact; never followed to
    /// decide what to display.
    Symlink(String),
    Image,
    Text,
    /// Known type, nothing safe to show: "PDF document", "ZIP archive".
    Opaque,
}

impl PeekKind {
    pub fn label(&self) -> &'static str {
        match self {
            PeekKind::Folder => "Folder",
            PeekKind::Symlink(_) => "Link",
            PeekKind::Image => "Image",
            PeekKind::Text => "Text",
            PeekKind::Opaque => "File",
        }
    }
}

/// Everything Quick Look knows about one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peek {
    pub name: String,
    pub kind: PeekKind,
    /// A human phrase: "PNG image", "Rust source", "PDF document".
    pub kind_words: String,
    pub size: u64,
    pub mode_line: String,
    /// First lines of a text file, already made safe to display.
    pub text: Vec<String>,
    /// True when `text` was cut short by the bounds.
    pub truncated: bool,
    /// Why there is nothing to show, when there is nothing to show.
    pub no_preview_reason: Option<String>,
}

/// The mode line as `ls -l` writes it, so a user can see at a glance that a
/// file is not writable or is a link.
pub fn mode_line(mode: u32, is_dir: bool, is_link: bool) -> String {
    let kind = if is_link {
        'l'
    } else if is_dir {
        'd'
    } else {
        '-'
    };
    let bit = |shift: u32, ch: char| {
        if mode >> shift & 1 == 1 {
            ch
        } else {
            '-'
        }
    };
    format!(
        "{kind}{}{}{}{}{}{}{}{}{}",
        bit(8, 'r'),
        bit(7, 'w'),
        bit(6, 'x'),
        bit(5, 'r'),
        bit(4, 'w'),
        bit(3, 'x'),
        bit(2, 'r'),
        bit(1, 'w'),
        bit(0, 'x'),
    )
}

/// Make bytes safe to put on screen.
///
/// Control characters become a visible dot rather than escaping into the
/// terminal, the theme, or a QML string. A file is *shown*, never obeyed.
pub fn sanitise_line(raw: &[u8]) -> String {
    let mut out = String::with_capacity(raw.len());
    for chunk in raw.utf8_chunks() {
        for c in chunk.valid().chars() {
            let bidi = matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
            match c {
                '\t' => out.push_str("    "),
                // Control characters and bidi overrides become a visible dot
                // rather than escaping into the terminal, the theme, or a
                // QML string, or reversing the rest of the line.
                c if c.is_control() || bidi => out.push('·'),
                c => out.push(c),
            }
        }
        if !chunk.invalid().is_empty() {
            out.push('·');
        }
    }
    out
}

/// Human words for what this file is, from its name only.
///
/// The name is not proof of contents — that is exactly why nothing is run
/// on the strength of it. It is enough to say "PDF document" honestly.
pub fn kind_words(name: &OsStr) -> String {
    let lower = name.to_string_lossy().to_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e.to_string());
    let by_ext = match ext.as_deref() {
        Some("png") => "PNG image",
        Some("jpg") | Some("jpeg") => "JPEG image",
        Some("gif") => "GIF image",
        Some("webp") => "WebP image",
        Some("svg") => "SVG image",
        Some("pdf") => "PDF document",
        Some("zip") => "ZIP archive",
        Some("gz") | Some("tgz") => "gzip archive",
        Some("zst") => "zstd archive",
        Some("tar") => "tar archive",
        Some("mp4") | Some("mkv") | Some("webm") => "Video",
        Some("mp3") | Some("flac") | Some("ogg") | Some("wav") => "Audio",
        Some("rs") => "Rust source",
        Some("py") => "Python source",
        Some("qml") => "QML source",
        Some("sh") => "Shell script",
        Some("md") => "Markdown text",
        Some("toml") => "TOML configuration",
        Some("json") => "JSON data",
        Some("txt") | Some("log") => "Plain text",
        _ => "",
    };
    if !by_ext.is_empty() {
        return by_ext.to_string();
    }
    match category_for_name(name) {
        Category::Image => "Image".into(),
        Category::Text => "Text".into(),
        Category::Archive => "Archive".into(),
        Category::Audio => "Audio".into(),
        Category::Video => "Video".into(),
        _ => "File".into(),
    }
}

/// Read a bounded text peek. Returns the lines and whether it was cut short.
pub fn peek_text(path: &Path) -> Option<(Vec<String>, bool)> {
    let f = std::fs::File::open(path).ok()?;
    let mut buf = Vec::new();
    f.take(MAX_PEEK_BYTES).read_to_end(&mut buf).ok()?;
    // A NUL means this is not text after all, whatever the name says.
    if buf.contains(&0) {
        return None;
    }
    let total_lines = buf.split(|b| *b == b'\n').count();
    let lines: Vec<String> = buf
        .split(|b| *b == b'\n')
        .take(MAX_PEEK_LINES)
        .map(sanitise_line)
        .collect();
    let truncated = total_lines > MAX_PEEK_LINES || buf.len() as u64 >= MAX_PEEK_BYTES;
    Some((lines, truncated))
}

/// Look at one path and report what it is.
///
/// Never opens a file to *run* it and never follows a symlink to decide what
/// to show: the link itself is the thing being previewed.
pub fn peek(path: &Path) -> Peek {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let link_meta = std::fs::symlink_metadata(path).ok();
    let is_link = link_meta
        .as_ref()
        .is_some_and(|m| m.file_type().is_symlink());

    if is_link {
        let target = std::fs::read_link(path)
            .map(|t| t.to_string_lossy().to_string())
            .unwrap_or_else(|_| "?".into());
        use std::os::unix::fs::PermissionsExt;
        let mode = link_meta
            .as_ref()
            .map(|m| m.permissions().mode())
            .unwrap_or(0);
        return Peek {
            name,
            kind: PeekKind::Symlink(target.clone()),
            kind_words: format!("Link to {target}"),
            size: link_meta.as_ref().map(|m| m.len()).unwrap_or(0),
            mode_line: mode_line(mode, false, true),
            text: Vec::new(),
            truncated: false,
            // Deliberate: following the link to preview its target would
            // make a link to a 4 GB image look like an image preview.
            no_preview_reason: Some("Links are shown, not followed".into()),
        };
    }

    let Some(meta) = std::fs::metadata(path).ok() else {
        return Peek {
            name,
            kind: PeekKind::Opaque,
            kind_words: "File".into(),
            size: 0,
            mode_line: String::new(),
            text: Vec::new(),
            truncated: false,
            no_preview_reason: Some("That file is no longer there".into()),
        };
    };
    use std::os::unix::fs::PermissionsExt;
    let mode = meta.permissions().mode();

    if meta.is_dir() {
        let n = std::fs::read_dir(path).map(|rd| rd.count()).unwrap_or(0);
        return Peek {
            name,
            kind: PeekKind::Folder,
            kind_words: if n == 1 {
                "Folder · 1 item".into()
            } else {
                format!("Folder · {n} items")
            },
            size: 0,
            mode_line: mode_line(mode, true, false),
            text: Vec::new(),
            truncated: false,
            no_preview_reason: None,
        };
    }

    let os_name = path.file_name().unwrap_or_default();
    let words = kind_words(os_name);
    let cat = category_for_name(os_name);

    // Images are described, not decoded here: untrusted image bytes are only
    // ever decoded inside the sandbox that already guards thumbnails (§5).
    if cat == Category::Image {
        return Peek {
            name,
            kind: PeekKind::Image,
            kind_words: words,
            size: meta.len(),
            mode_line: mode_line(mode, false, false),
            text: Vec::new(),
            truncated: false,
            no_preview_reason: None,
        };
    }

    if cat == Category::Text {
        if let Some((lines, truncated)) = peek_text(path) {
            return Peek {
                name,
                kind: PeekKind::Text,
                kind_words: words,
                size: meta.len(),
                mode_line: mode_line(mode, false, false),
                text: lines,
                truncated,
                no_preview_reason: None,
            };
        }
        // Named like text, but it has NUL bytes in it.
        return Peek {
            name,
            kind: PeekKind::Opaque,
            kind_words: words,
            size: meta.len(),
            mode_line: mode_line(mode, false, false),
            text: Vec::new(),
            truncated: false,
            no_preview_reason: Some("This is not text after all".into()),
        };
    }

    Peek {
        name,
        kind: PeekKind::Opaque,
        kind_words: words,
        size: meta.len(),
        mode_line: mode_line(mode, false, false),
        text: Vec::new(),
        truncated: false,
        no_preview_reason: Some("No preview for this kind of file".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gust-peek-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn mode_lines_read_like_ls() {
        assert_eq!(mode_line(0o644, false, false), "-rw-r--r--");
        assert_eq!(mode_line(0o755, true, false), "drwxr-xr-x");
        assert_eq!(mode_line(0o777, false, true), "lrwxrwxrwx");
        assert_eq!(mode_line(0o400, false, false), "-r--------");
    }

    #[test]
    fn control_characters_cannot_reach_the_screen() {
        // An escape sequence in a file must be shown, never obeyed.
        let raw = b"before\x1b[31mred\x07 after";
        let out = sanitise_line(raw);
        assert!(!out.contains('\x1b'), "escape survived: {out:?}");
        assert!(!out.contains('\x07'));
        assert!(out.contains("before"));
        assert!(out.contains("after"));
    }

    #[test]
    fn text_in_any_language_is_shown_as_written() {
        // The comment always promised this; the code turned every
        // non-ASCII byte into a dot, so a German or Japanese file was
        // unreadable in Quick Look.
        assert_eq!(sanitise_line("Grüße, 写真 ✓".as_bytes()), "Grüße, 写真 ✓");
        // Invalid bytes are still made visible, one mark per bad sequence.
        assert_eq!(sanitise_line(b"ok\xff\xfeend"), "ok··end");
        // Bidi overrides would visually reverse the rest of a line.
        assert!(!sanitise_line("a\u{202e}b".as_bytes()).contains('\u{202e}'));
    }

    #[test]
    fn tabs_become_spaces_rather_than_vanishing() {
        assert_eq!(sanitise_line(b"a\tb"), "a    b");
    }

    #[test]
    fn kinds_are_words_not_mime_types() {
        assert_eq!(kind_words(OsStr::new("a.png")), "PNG image");
        assert_eq!(kind_words(OsStr::new("b.pdf")), "PDF document");
        assert_eq!(kind_words(OsStr::new("c.rs")), "Rust source");
        assert!(!kind_words(OsStr::new("d.png")).contains('/'));
    }

    #[test]
    fn a_text_file_is_peeked_and_bounded() {
        let d = tmp("text");
        let f = d.join("notes.txt");
        let body: String = (0..200).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&f, body).unwrap();
        let p = peek(&f);
        assert_eq!(p.kind, PeekKind::Text);
        assert!(p.text.len() <= MAX_PEEK_LINES);
        assert!(p.truncated, "a 200-line file must report being cut short");
        assert!(p.no_preview_reason.is_none());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_short_file_is_not_reported_as_truncated() {
        let d = tmp("short");
        let f = d.join("a.txt");
        std::fs::write(&f, "one\ntwo\n").unwrap();
        let p = peek(&f);
        assert!(!p.truncated);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_symlink_is_shown_not_followed() {
        let d = tmp("link");
        let target = d.join("real.txt");
        std::fs::write(&target, "secret contents\n").unwrap();
        let link = d.join("pointer");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let p = peek(&link);
        assert!(matches!(p.kind, PeekKind::Symlink(_)));
        assert!(
            p.text.is_empty(),
            "following the link would preview the target's contents"
        );
        assert!(p.kind_words.contains("Link to"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn an_image_is_described_never_decoded_here() {
        let d = tmp("image");
        let f = d.join("photo.png");
        // Not a real PNG: if anything tried to decode it, this would show.
        std::fs::write(&f, b"\x89PNG\r\n\x1a\n garbage").unwrap();
        let p = peek(&f);
        assert_eq!(p.kind, PeekKind::Image);
        assert_eq!(p.kind_words, "PNG image");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_file_named_like_text_but_full_of_nuls_says_so() {
        let d = tmp("fake");
        let f = d.join("thing.txt");
        std::fs::write(&f, b"text\0\0binary").unwrap();
        let p = peek(&f);
        assert_eq!(p.kind, PeekKind::Opaque);
        assert!(p.no_preview_reason.unwrap().contains("not text"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_folder_reports_how_many_things_are_in_it() {
        let d = tmp("folder");
        std::fs::write(d.join("a"), "").unwrap();
        std::fs::write(d.join("b"), "").unwrap();
        let p = peek(&d);
        assert_eq!(p.kind, PeekKind::Folder);
        assert!(p.kind_words.contains('2'), "got {:?}", p.kind_words);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn one_item_is_singular() {
        let d = tmp("one");
        std::fs::write(d.join("only"), "").unwrap();
        let p = peek(&d);
        assert!(p.kind_words.contains("1 item"), "got {:?}", p.kind_words);
        assert!(!p.kind_words.contains("1 items"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_missing_file_says_so_instead_of_pretending() {
        let p = peek(Path::new("/tmp/gust-peek-does-not-exist-at-all"));
        assert!(p.no_preview_reason.is_some());
    }

    #[test]
    fn an_unknown_binary_offers_no_preview_but_still_says_what_it_is() {
        let d = tmp("opaque");
        let f = d.join("thing.bin");
        std::fs::write(&f, vec![1u8, 2, 3]).unwrap();
        let p = peek(&f);
        assert_eq!(p.kind, PeekKind::Opaque);
        assert!(p.no_preview_reason.is_some());
        assert_eq!(p.size, 3);
        std::fs::remove_dir_all(&d).ok();
    }
}
