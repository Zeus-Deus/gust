//! Producing a thumbnail: run a helper under the sandbox, validate, cache.
//!
//! Nothing here decodes an untrusted byte. The helper runs under
//! `bwrap`+`prlimit` (see [`crate::sandbox`]), writes into a private output
//! directory, and the parent only ever sees bytes that passed
//! [`crate::sandbox::validate_output`].
//!
//! The freedesktop cache is written atomically: a thumbnail is staged under a
//! private name and renamed into place, so a reader never sees half a PNG and
//! a crash cannot publish a truncated image.

use crate::cache::{cache_base, file_uri, thumbnail_path, ThumbSize};
use crate::sandbox::{sandbox_available, validate_output, SandboxSpec, ValidationError};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::UNIX_EPOCH;

/// Why a thumbnail is not available.
///
/// Every variant is a state the UI can show honestly. There is deliberately
/// no "fell back to decoding it ourselves" case.
#[derive(Debug, Clone, PartialEq)]
pub enum ThumbError {
    /// bwrap/prlimit missing: thumbnailing of untrusted input is unavailable.
    SandboxUnavailable,
    /// No helper on this system can handle this kind of file.
    NoHelper,
    /// The helper failed, timed out, or was killed by its resource budget.
    HelperFailed(String),
    /// The helper produced something that is not a bounded, regular PNG.
    BadOutput(ValidationError),
    /// The source file could not be read or is not a regular file.
    Source(String),
    /// The cache could not be written.
    Cache(String),
}

impl std::fmt::Display for ThumbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThumbError::SandboxUnavailable => {
                write!(f, "previews are unavailable: bwrap or prlimit is missing")
            }
            ThumbError::NoHelper => write!(f, "no preview helper for this kind of file"),
            ThumbError::HelperFailed(e) => write!(f, "the preview helper failed: {e}"),
            ThumbError::BadOutput(e) => write!(f, "the preview helper output was rejected: {e}"),
            ThumbError::Source(e) => write!(f, "cannot read this file: {e}"),
            ThumbError::Cache(e) => write!(f, "cannot write the preview cache: {e}"),
        }
    }
}

/// A helper that can turn one kind of file into a PNG.
#[derive(Debug, Clone)]
pub struct Helper {
    /// Binary name, looked up on PATH inside the sandbox.
    pub binary: &'static str,
    /// Arguments, with `{in}` and `{out}` replaced by the in-sandbox paths.
    pub args: &'static [&'static str],
}

/// Helpers we are willing to run, in preference order.
///
/// The in-sandbox paths are fixed: the input is bound read-only at
/// `/in/input` and the private output directory at `/out`.
fn helpers_for(mime: &str) -> Vec<Helper> {
    let mut v = Vec::new();
    if mime.starts_with("image/") {
        // ImageMagick: `[0]` takes the first frame so an animation or a
        // multi-page document cannot expand into hundreds of renders.
        v.push(Helper {
            binary: "magick",
            args: &[
                "/in/input[0]",
                "-auto-orient",
                "-thumbnail",
                "256x256>",
                "-strip",
                "png:/out/thumb.png",
            ],
        });
    }
    if mime.starts_with("video/") {
        v.push(Helper {
            binary: "ffmpegthumbnailer",
            args: &[
                "-i",
                "/in/input",
                "-o",
                "/out/thumb.png",
                "-s",
                "256",
                "-q",
                "6",
            ],
        });
    }
    v
}

/// Can anything on this system thumbnail this MIME type?
pub fn can_thumbnail(mime: &str) -> bool {
    !helpers_for(mime).is_empty()
}

/// The freedesktop `Thumb::MTime` of the source, as the spec requires.
fn source_mtime(path: &Path) -> Result<u64, ThumbError> {
    let md = path
        .symlink_metadata()
        .map_err(|e| ThumbError::Source(e.to_string()))?;
    if !md.is_file() {
        return Err(ThumbError::Source("not a regular file".into()));
    }
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .ok_or_else(|| ThumbError::Source("no modification time".into()))
}

/// Is there already a valid cached thumbnail for this exact file version?
///
/// A cached PNG whose recorded source mtime differs from the file's current
/// mtime is stale and must be ignored, or an edited image would keep showing
/// its old preview forever.
pub fn cached(path: &Path, size: ThumbSize) -> Option<PathBuf> {
    let target = thumbnail_path(path, size)?;
    let want = source_mtime(path).ok()?;
    let bytes = std::fs::read(&target).ok()?;
    if png_text_mtime(&bytes)? == want {
        Some(target)
    } else {
        None
    }
}

/// Read the `Thumb::MTime` value out of a PNG's tEXt chunks.
///
/// Deliberately a bounded scan of chunk headers, not a decoder: it never
/// interprets image data, only walks the length-prefixed chunk structure.
fn png_text_mtime(bytes: &[u8]) -> Option<u64> {
    const HEADER: usize = 8;
    let mut i = HEADER;
    // Bounded: stop after a sane number of chunks or at the image data.
    for _ in 0..64 {
        if i + 8 > bytes.len() {
            return None;
        }
        let len = u32::from_be_bytes(bytes[i..i + 4].try_into().ok()?) as usize;
        let kind = &bytes[i + 4..i + 8];
        if kind == b"IDAT" || kind == b"IEND" {
            return None;
        }
        let data_start = i + 8;
        let data_end = data_start.checked_add(len)?;
        if data_end > bytes.len() {
            return None;
        }
        if kind == b"tEXt" {
            let data = &bytes[data_start..data_end];
            if let Some(nul) = data.iter().position(|b| *b == 0) {
                if &data[..nul] == b"Thumb::MTime" {
                    return std::str::from_utf8(&data[nul + 1..])
                        .ok()?
                        .trim()
                        .parse()
                        .ok();
                }
            }
        }
        i = data_end + 4; // skip the CRC
    }
    None
}

/// Produce a thumbnail for `path`, returning the cached PNG's location.
///
/// Runs the helper under the sandbox, validates its output through a held
/// descriptor, and writes the freedesktop cache atomically. Returns quickly
/// when a valid cached thumbnail already exists.
pub fn thumbnail(path: &Path, mime: &str, size: ThumbSize) -> Result<PathBuf, ThumbError> {
    if let Some(hit) = cached(path, size) {
        return Ok(hit);
    }
    if !sandbox_available() {
        return Err(ThumbError::SandboxUnavailable);
    }
    let helpers = helpers_for(mime);
    if helpers.is_empty() {
        return Err(ThumbError::NoHelper);
    }
    let mtime = source_mtime(path)?;
    let target =
        thumbnail_path(path, size).ok_or_else(|| ThumbError::Cache("no cache dir".into()))?;
    let base = cache_base().ok_or_else(|| ThumbError::Cache("no cache dir".into()))?;

    // Private scratch for the helper's output, inside our own cache.
    let scratch = base.join(format!(".gust-thumb-{}-{}", std::process::id(), mtime));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).map_err(|e| ThumbError::Cache(e.to_string()))?;

    let result = run_helpers(path, &helpers, &scratch);

    let out = match result {
        Ok(bytes) => bytes,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&scratch);
            return Err(e);
        }
    };

    // Record the source mtime so a later edit invalidates this thumbnail.
    let tagged = with_mtime_text(&out, mtime);

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ThumbError::Cache(e.to_string()))?;
    }
    // Stage then rename: a reader never sees a half-written PNG.
    let stage = target.with_file_name(format!(
        ".gust-staging-{}-{}",
        std::process::id(),
        target
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("thumb")
    ));
    let write = std::fs::write(&stage, &tagged)
        .and_then(|_| std::fs::rename(&stage, &target))
        .map_err(|e| ThumbError::Cache(e.to_string()));
    let _ = std::fs::remove_dir_all(&scratch);
    if let Err(e) = write {
        let _ = std::fs::remove_file(&stage);
        return Err(e);
    }
    Ok(target)
}

/// Try each helper in turn; the first that yields a valid PNG wins.
fn run_helpers(path: &Path, helpers: &[Helper], scratch: &Path) -> Result<Vec<u8>, ThumbError> {
    let mut last = ThumbError::NoHelper;
    for h in helpers {
        let spec = SandboxSpec::new(path, scratch);
        let argv = spec.bwrap_argv(h.binary, h.args);
        let out_file = scratch.join("thumb.png");
        let _ = std::fs::remove_file(&out_file);

        let status = Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        match status {
            Ok(s) if s.success() => match validate_output(&out_file, spec.max_output_bytes) {
                Ok(bytes) => return Ok(bytes),
                Err(e) => last = ThumbError::BadOutput(e),
            },
            Ok(s) => last = ThumbError::HelperFailed(format!("{} exited with {s}", h.binary)),
            Err(e) => last = ThumbError::HelperFailed(format!("{}: {e}", h.binary)),
        }
    }
    Err(last)
}

/// Insert a `Thumb::MTime` tEXt chunk right after the PNG header.
fn with_mtime_text(png: &[u8], mtime: u64) -> Vec<u8> {
    let mut key = b"Thumb::MTime".to_vec();
    key.push(0);
    key.extend_from_slice(mtime.to_string().as_bytes());

    let mut chunk = Vec::new();
    chunk.extend_from_slice(&(key.len() as u32).to_be_bytes());
    chunk.extend_from_slice(b"tEXt");
    chunk.extend_from_slice(&key);
    let mut crc_input = b"tEXt".to_vec();
    crc_input.extend_from_slice(&key);
    chunk.extend_from_slice(&crc32(&crc_input).to_be_bytes());

    let mut out = Vec::with_capacity(png.len() + chunk.len());
    // Header, then IHDR (which must stay first), then our text chunk.
    let ihdr_end = if png.len() > 8 {
        let len = u32::from_be_bytes(png[8..12].try_into().unwrap_or([0; 4])) as usize;
        (8 + 8 + len + 4).min(png.len())
    } else {
        png.len()
    };
    out.extend_from_slice(&png[..ihdr_end]);
    out.extend_from_slice(&chunk);
    out.extend_from_slice(&png[ihdr_end..]);
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for b in data {
        crc ^= *b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// The freedesktop URI a thumbnail is keyed by. Re-exported for the bridge.
pub fn uri_for(path: &Path) -> String {
    file_uri(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gust-render-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn only_known_kinds_get_a_helper() {
        assert!(can_thumbnail("image/png"));
        assert!(can_thumbnail("image/jpeg"));
        assert!(can_thumbnail("video/mp4"));
        // Never guess at something we cannot safely render.
        assert!(!can_thumbnail("application/x-executable"));
        assert!(!can_thumbnail("text/plain"));
        assert!(!can_thumbnail("application/octet-stream"));
        assert!(!can_thumbnail("inode/directory"));
    }

    #[test]
    fn a_helper_never_receives_the_real_path_as_an_argument() {
        // The input is bound at a fixed in-sandbox path, so a filename can
        // never be interpreted as an option by the helper.
        for h in helpers_for("image/png") {
            for a in h.args {
                assert!(
                    !a.contains("{in}") && !a.contains("{out}"),
                    "unsubstituted placeholder in {a}"
                );
                assert!(
                    !a.starts_with("/home") && !a.starts_with("/root"),
                    "helper argv leaks a real path: {a}"
                );
            }
        }
    }

    #[test]
    fn an_mtime_tag_round_trips() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        // A minimal IHDR chunk.
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&[0u8; 13]);
        png.extend_from_slice(&0u32.to_be_bytes());

        let tagged = with_mtime_text(&png, 1_726_000_000);
        assert_eq!(png_text_mtime(&tagged), Some(1_726_000_000));
        assert_eq!(&tagged[..8], &png[..8], "the PNG header must stay intact");
    }

    #[test]
    fn a_png_without_our_tag_is_treated_as_stale() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&[0u8; 13]);
        png.extend_from_slice(&0u32.to_be_bytes());
        assert_eq!(png_text_mtime(&png), None);
    }

    #[test]
    fn a_truncated_png_does_not_panic_the_scanner() {
        for n in 0..40usize {
            let bytes = vec![0x89u8; n];
            let _ = png_text_mtime(&bytes);
        }
        // A chunk claiming a huge length must not read out of bounds.
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&u32::MAX.to_be_bytes());
        png.extend_from_slice(b"tEXt");
        assert_eq!(png_text_mtime(&png), None);
    }

    #[test]
    fn a_directory_is_never_thumbnailed() {
        let d = tmp("dir");
        let r = source_mtime(&d);
        assert!(r.is_err(), "a directory must not be treated as a source");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_symlink_is_not_followed_for_its_mtime() {
        let d = tmp("link");
        fs::write(d.join("real.png"), b"x").unwrap();
        std::os::unix::fs::symlink(d.join("real.png"), d.join("link.png")).unwrap();
        // symlink_metadata means we describe the link, not its target.
        assert!(
            source_mtime(&d.join("link.png")).is_err(),
            "a symlink must not be thumbnailed as though it were its target"
        );
        fs::remove_dir_all(&d).ok();
    }
}
