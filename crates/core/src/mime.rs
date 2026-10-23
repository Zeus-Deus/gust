//! MIME type detection and the "what opens this" question.
//!
//! Extension-first with a bounded content sniff. Gust never executes a file to
//! learn what it is, and never auto-runs anything during browsing.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Image,
    Video,
    Audio,
    Text,
    Pdf,
    Archive,
    Executable,
    Other,
}

pub fn category_for_name(name: &OsStr) -> Category {
    let b = name.as_bytes();
    let ext = match b.iter().rposition(|c| *c == b'.') {
        Some(p) if p > 0 => String::from_utf8_lossy(&b[p + 1..]).to_ascii_lowercase(),
        _ => String::new(),
    };
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "avif" | "heic" | "tiff"
        | "heif" | "tif" | "ico" => Category::Image,
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "wmv" | "flv" => Category::Video,
        "mp3" | "flac" | "ogg" | "wav" | "m4a" | "opus" | "aac" => Category::Audio,
        "pdf" => Category::Pdf,
        "zip" | "tar" | "gz" | "zst" | "xz" | "bz2" | "7z" | "rar" | "tgz" => Category::Archive,
        "txt" | "md" | "rs" | "py" | "js" | "ts" | "json" | "toml" | "yaml" | "yml" | "sh"
        | "c" | "h" | "cpp" | "qml" | "css" | "html" | "xml" | "log" | "conf" | "ini" => {
            Category::Text
        }
        "appimage" | "run" | "bin" | "exe" => Category::Executable,
        _ => Category::Other,
    }
}

/// Is this file executable by its mode bits?
pub fn is_executable_mode(mode: u32) -> bool {
    mode & 0o111 != 0
}

/// Should activating this entry require a deliberate trusted action?
///
/// Phone and non-Unix mounts can supply executable bits for ordinary content.
/// Known content types therefore beat mode bits, but script/program extensions
/// and executable files of unknown type still require deliberate Run.
pub fn needs_trusted_launch(name: &OsStr, mode: Option<u32>) -> bool {
    let ext = Path::new(name)
        .extension()
        .map(|e| e.as_bytes().to_ascii_lowercase())
        .unwrap_or_default();
    if matches!(
        ext.as_slice(),
        b"sh" | b"bash" | b"zsh" | b"fish" | b"py" | b"pl" | b"desktop"
    ) {
        return true;
    }
    // JS/TS sources are text, but with an executable bit they are shebang
    // scripts, not content.
    if matches!(ext.as_slice(), b"js" | b"ts") {
        return mode.is_some_and(is_executable_mode);
    }
    match category_for_name(name) {
        Category::Executable => true,
        Category::Other => mode.is_some_and(is_executable_mode),
        _ => false,
    }
}

/// A `.desktop` file is a script by another name.
pub fn is_desktop_entry(name: &OsStr) -> bool {
    Path::new(name).extension() == Some(OsStr::new("desktop"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn categorises_by_extension_case_insensitively() {
        assert_eq!(category_for_name(OsStr::new("a.PNG")), Category::Image);
        assert_eq!(
            category_for_name(OsStr::new("a.tar.zst")),
            Category::Archive
        );
        assert_eq!(category_for_name(OsStr::new("notes.md")), Category::Text);
        assert_eq!(category_for_name(OsStr::new("noext")), Category::Other);
    }

    #[test]
    fn dotfiles_are_not_extensions() {
        assert_eq!(category_for_name(OsStr::new(".bashrc")), Category::Other);
    }

    #[test]
    fn executable_bit_requires_trusted_launch() {
        assert!(needs_trusted_launch(OsStr::new("script"), Some(0o755)));
        assert!(!needs_trusted_launch(OsStr::new("notes.txt"), Some(0o644)));
    }

    #[test]
    fn appimage_requires_trusted_launch_even_without_mode() {
        assert!(needs_trusted_launch(OsStr::new("Thing.AppImage"), None));
    }

    #[test]
    fn phone_content_is_not_a_program_despite_executable_bits() {
        for name in [
            "IMG_0001.JPG",
            "photo.HEIC",
            "photo.HEIF",
            "scan.TIF",
            "clip.MOV",
            "clip.MP4",
            "memo.M4A",
            "song.MP3",
            "notes.txt",
            "report.PDF",
            "backup.zip",
        ] {
            for mode in [None, Some(0o700), Some(0o755), Some(0o777)] {
                assert!(
                    !needs_trusted_launch(OsStr::new(name), mode),
                    "{name} {mode:?}"
                );
            }
        }
    }

    #[test]
    fn scripts_and_programs_require_deliberate_run_with_any_mode() {
        for name in [
            "install.sh",
            "setup.PY",
            "run.bash",
            "run.zsh",
            "run.fish",
            "run.pl",
            "app.desktop",
            "app.DESKTOP",
            "Thing.AppImage",
            "setup.run",
            "app.exe",
            "app.bin",
        ] {
            for mode in [None, Some(0o644), Some(0o700)] {
                assert!(
                    needs_trusted_launch(OsStr::new(name), mode),
                    "{name} {mode:?}"
                );
            }
        }
        assert!(needs_trusted_launch(
            OsStr::new("unknown.format"),
            Some(0o700)
        ));
        assert!(needs_trusted_launch(OsStr::new("program"), Some(0o700)));
    }

    #[test]
    fn executable_js_and_ts_scripts_require_deliberate_run() {
        for name in ["build.js", "tool.TS"] {
            for mode in [Some(0o700), Some(0o755)] {
                assert!(
                    needs_trusted_launch(OsStr::new(name), mode),
                    "{name} {mode:?}"
                );
            }
            // Without +x they are source files and open in an editor.
            for mode in [None, Some(0o644)] {
                assert!(
                    !needs_trusted_launch(OsStr::new(name), mode),
                    "{name} {mode:?}"
                );
            }
        }
    }

    #[test]
    fn desktop_entry_detected() {
        assert!(is_desktop_entry(&OsString::from("x.desktop")));
        assert!(!is_desktop_entry(&OsString::from("x.txt")));
    }
}

/// A coarse MIME string for the thumbnail layer.
///
/// Deliberately coarse: the thumbnailer only needs to know which family of
/// helper to try, and a wrong-but-plausible specific type would be worse
/// than an honest family. SVG is excluded from `image/` on purpose — it is a
/// scripting-capable format and is not sent to an image helper.
pub fn thumbnail_mime(name: &OsStr) -> Option<&'static str> {
    let b = name.as_bytes();
    let ext = match b.iter().rposition(|c| *c == b'.') {
        Some(p) if p > 0 => String::from_utf8_lossy(&b[p + 1..]).to_ascii_lowercase(),
        _ => return None,
    };
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "tiff" | "tif" => Some("image/tiff"),
        "avif" => Some("image/avif"),
        "heic" | "heif" => Some("image/heif"),
        "ico" => Some("image/x-icon"),
        "mp4" | "m4v" => Some("video/mp4"),
        "mkv" => Some("video/x-matroska"),
        "webm" => Some("video/webm"),
        "mov" => Some("video/quicktime"),
        "avi" => Some("video/x-msvideo"),
        "pdf" => Some("application/pdf"),
        // Not thumbnailed: svg (scriptable),
        // anything executable, and everything unknown.
        _ => None,
    }
}

#[cfg(test)]
mod thumbnail_mime_tests {
    use super::*;

    #[test]
    fn images_and_videos_get_a_family() {
        assert_eq!(thumbnail_mime(OsStr::new("a.png")), Some("image/png"));
        // A PDF's first page is rendered in the same sandbox (poppler).
        assert_eq!(
            thumbnail_mime(OsStr::new("Invoice.PDF")),
            Some("application/pdf")
        );
        assert_eq!(thumbnail_mime(OsStr::new("A.JPG")), Some("image/jpeg"));
        assert_eq!(thumbnail_mime(OsStr::new("clip.mp4")), Some("video/mp4"));
    }

    #[test]
    fn scriptable_and_unknown_formats_are_refused() {
        // SVG can carry script; it never goes to an image helper.
        assert_eq!(thumbnail_mime(OsStr::new("logo.svg")), None);
        assert_eq!(thumbnail_mime(OsStr::new("prog")), None);
        assert_eq!(thumbnail_mime(OsStr::new("script.sh")), None);
        assert_eq!(thumbnail_mime(OsStr::new("data.bin")), None);
    }

    #[test]
    fn a_dotfile_is_not_read_as_an_extension() {
        // ".png" is a hidden file named png, not a PNG image.
        assert_eq!(thumbnail_mime(OsStr::new(".png")), None);
        assert_eq!(thumbnail_mime(OsStr::new(".bashrc")), None);
    }

    #[test]
    fn a_double_extension_uses_the_last_one() {
        // archive.tar.gz is not an image just because of an earlier part.
        assert_eq!(thumbnail_mime(OsStr::new("archive.tar.gz")), None);
        assert_eq!(
            thumbnail_mime(OsStr::new("photo.bak.png")),
            Some("image/png")
        );
    }
}
