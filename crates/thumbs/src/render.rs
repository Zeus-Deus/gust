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
    /// Arguments. Paths are the fixed in-sandbox ones; the only variable is
    /// the size, a number Gust chose.
    pub args: Vec<String>,
}

/// Helpers we are willing to run, in preference order.
///
/// The in-sandbox paths are fixed: the input is bound read-only at
/// `/in/input` and the private output directory at `/out`.
fn helpers_for(mime: &str, px: u32) -> Vec<Helper> {
    let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let n = px.to_string();
    let mut v = Vec::new();
    if mime.starts_with("image/") {
        // ImageMagick: `[0]` takes the first frame so an animation or a
        // multi-page document cannot expand into hundreds of renders. `>`
        // only ever shrinks: a small image keeps its real size.
        let geom = format!("{px}x{px}>");
        v.push(Helper {
            binary: "magick",
            args: args(&[
                "/in/input[0]",
                "-auto-orient",
                "-thumbnail",
                &geom,
                "-strip",
                "png:/out/thumb.png",
            ]),
        });
    }
    if mime == "application/pdf" {
        // First page only, rendered by poppler inside the same sandbox as
        // every other helper: no network, no home, read-only input, killed
        // on the time and memory limits. `-singlefile` writes exactly
        // /out/thumb.png, the path the validator reads.
        v.push(Helper {
            binary: "pdftoppm",
            args: args(&[
                "-png",
                "-f",
                "1",
                "-l",
                "1",
                "-singlefile",
                "-scale-to",
                &n,
                "/in/input",
                "/out/thumb",
            ]),
        });
    }
    if mime.starts_with("video/") {
        v.push(Helper {
            binary: "ffmpegthumbnailer",
            args: args(&[
                "-i",
                "/in/input",
                "-o",
                "/out/thumb.png",
                "-s",
                &n,
                "-q",
                "6",
            ]),
        });
    }
    v
}

/// Can anything on this system thumbnail this MIME type?
pub fn can_thumbnail(mime: &str) -> bool {
    !helpers_for(mime, 256).is_empty()
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
    let helpers = helpers_for(mime, size.pixels());
    if helpers.is_empty() {
        return Err(ThumbError::NoHelper);
    }
    let mtime = source_mtime(path)?;
    let target =
        thumbnail_path(path, size).ok_or_else(|| ThumbError::Cache("no cache dir".into()))?;
    let base = cache_base().ok_or_else(|| ThumbError::Cache("no cache dir".into()))?;

    // Private scratch for the helper's output, inside our own cache. Unique
    // per render: files copied in the same second share an mtime, and two
    // workers (a listing that refreshed while the last pass still ran) then
    // deleted each other's output, leaving a copied photo without a preview.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let scratch = base.join(format!(".gust-thumb-{}-{}-{n}", std::process::id(), mtime));
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

/// The number of pages in a PDF, from `pdfinfo` run in the sandbox.
/// At most 64 KiB of its output is read.
pub fn pdf_pages(path: &Path) -> Result<u32, ThumbError> {
    let base = cache_base().ok_or_else(|| ThumbError::Cache("no cache dir".into()))?;
    pdf_pages_in(&base, path)
}

fn pdf_pages_in(base: &Path, path: &Path) -> Result<u32, ThumbError> {
    if !sandbox_available() {
        return Err(ThumbError::SandboxUnavailable);
    }
    let md = path
        .symlink_metadata()
        .map_err(|e| ThumbError::Source(e.to_string()))?;
    if !md.is_file() {
        return Err(ThumbError::Source("not a regular file".into()));
    }
    let scratch = base.join(format!(".gust-pdfinfo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).map_err(|e| ThumbError::Cache(e.to_string()))?;
    let argv = SandboxSpec::new(path, &scratch).bwrap_argv("pdfinfo", &["/in/input"]);
    let child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let result = match child {
        Ok(mut c) => {
            let mut text = Vec::new();
            if let Some(out) = c.stdout.take() {
                use std::io::Read;
                let _ = out.take(64 * 1024).read_to_end(&mut text);
            }
            let _ = c.kill();
            let _ = c.wait();
            parse_pdf_pages(&String::from_utf8_lossy(&text))
                .ok_or_else(|| ThumbError::HelperFailed("pdfinfo gave no page count".into()))
        }
        Err(e) => Err(ThumbError::HelperFailed(format!("pdfinfo: {e}"))),
    };
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

/// `Pages:          12` from pdfinfo's output.
pub fn parse_pdf_pages(text: &str) -> Option<u32> {
    text.lines()
        .find_map(|l| l.strip_prefix("Pages:"))
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|n| *n > 0)
}

/// Page `page` (1-based) of a PDF as a PNG at about `px` pixels, rendered
/// by poppler in the sandbox and validated like every thumbnail. Cached in
/// Gust's own cache (`…/thumbnails/gust-pages/`), keyed by the file's URI,
/// its mtime, the page and the size, so an edited PDF never shows old pages.
pub fn pdf_page(path: &Path, page: u32, px: u32) -> Result<PathBuf, ThumbError> {
    let base = cache_base().ok_or_else(|| ThumbError::Cache("no cache dir".into()))?;
    pdf_page_in(&base, path, page, px)
}

fn pdf_page_in(base: &Path, path: &Path, page: u32, px: u32) -> Result<PathBuf, ThumbError> {
    if !sandbox_available() {
        return Err(ThumbError::SandboxUnavailable);
    }
    let mtime = source_mtime(path)?;
    let key = crate::cache::md5_hex(file_uri(path).as_bytes());
    let dir = base.join("gust-pages");
    let target = dir.join(format!("{key}-{mtime}-{page}-{px}.png"));
    if target.is_file() {
        return Ok(target);
    }
    std::fs::create_dir_all(&dir).map_err(|e| ThumbError::Cache(e.to_string()))?;
    let scratch = base.join(format!(".gust-page-{}-{key}-{page}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).map_err(|e| ThumbError::Cache(e.to_string()))?;
    let p = page.to_string();
    let n = px.to_string();
    let helper = Helper {
        binary: "pdftoppm",
        args: [
            "-png",
            "-f",
            &p,
            "-l",
            &p,
            "-singlefile",
            "-scale-to",
            &n,
            "/in/input",
            "/out/thumb",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    };
    let result = run_helpers(path, &[helper], &scratch);
    let out = match result {
        Ok(b) => b,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&scratch);
            return Err(e);
        }
    };
    // Earlier versions of this file's pages are dropped as the new ones land.
    if let Ok(rd) = std::fs::read_dir(&dir) {
        let stale = format!("{key}-");
        let current = format!("{key}-{mtime}-");
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with(&stale) && !name.starts_with(&current) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let stage = dir.join(format!(".staging-{}-{key}-{page}", std::process::id()));
    let write = std::fs::write(&stage, &out)
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
        let mut spec = SandboxSpec::new(path, scratch);
        if h.binary == "ffmpegthumbnailer" {
            // Decoder/scaler thread stacks can exhaust 512 MiB of virtual
            // address space even for 1080p video, leaving every tile blank.
            // Match the audio decoder's budget; keep all other limits.
            spec.address_space_bytes = 1024 * 1024 * 1024;
        }
        let hargs: Vec<&str> = h.args.iter().map(String::as_str).collect();
        let argv = spec.bwrap_argv(h.binary, &hargs);
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
        // A PDF's first page, through poppler, in the sandbox.
        assert!(can_thumbnail("application/pdf"));
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
        for h in helpers_for("image/png", 1024)
            .into_iter()
            .chain(helpers_for("application/pdf", 1024))
            .chain(helpers_for("video/mp4", 1024))
        {
            for a in &h.args {
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
    fn pdf_page_counts_are_read_from_pdfinfo() {
        assert_eq!(
            parse_pdf_pages("Title: x\nPages:          12\nEncrypted: no\n"),
            Some(12)
        );
        assert_eq!(parse_pdf_pages("Pages: 0\n"), None);
        assert_eq!(parse_pdf_pages("garbage"), None);
    }

    /// A real three-page PDF, rendered page by page in the sandbox.
    #[test]
    fn every_page_of_a_pdf_renders_in_the_sandbox() {
        gust_testkit::skip_unless!(
            sandbox_available()
                && gust_testkit::have("pdftoppm")
                && gust_testkit::have("pdfinfo")
                && gust_testkit::have("magick"),
            "PDF",
            "bwrap, poppler or ImageMagick is not installed"
        );
        let d = std::env::temp_dir().join(format!("gust-pdf-pages-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let pdf = d.join("three.pdf");
        let ok = Command::new("magick")
            .args(["-size", "300x400", "xc:white", "xc:gray", "xc:black"])
            .arg(&pdf)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let cache = d.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        assert_eq!(pdf_pages_in(&cache, &pdf).unwrap(), 3);
        let p1 = pdf_page_in(&cache, &pdf, 1, 256).unwrap();
        let p3 = pdf_page_in(&cache, &pdf, 3, 256).unwrap();
        assert_ne!(p1, p3);
        assert!(std::fs::read(&p3)
            .unwrap()
            .starts_with(&[0x89, b'P', b'N', b'G']));
        // Past the end: the helper fails, nothing is cached.
        assert!(pdf_page_in(&cache, &pdf, 9, 256).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn two_passes_over_same_second_images_both_succeed() {
        // Found in the real window: five photos pasted in one second, the
        // listing refreshed while the first thumbnail pass still ran, and
        // one copy kept a generic icon for good. Both passes named their
        // scratch folder after pid + mtime, so each deleted the other's
        // helper output.
        gust_testkit::skip_unless!(
            sandbox_available() && gust_testkit::have("magick"),
            "thumbnails",
            "bwrap or ImageMagick is not installed"
        );
        let d = std::env::temp_dir().join(format!("gust-thumb-race-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::env::set_var("XDG_CACHE_HOME", d.join("cache"));
        let when =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_760_000_000);
        let mut imgs = Vec::new();
        for i in 0..8 {
            let p = d.join(format!("IMG_{i}.png"));
            let ok = Command::new("magick")
                .args([
                    "-size",
                    "64x48",
                    &format!("xc:rgb({},{},90)", i * 30, 255 - i * 30),
                ])
                .arg(&p)
                .status()
                .unwrap()
                .success();
            assert!(ok);
            std::fs::File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_modified(when)
                .unwrap();
            imgs.push(p);
        }
        let run = |list: Vec<PathBuf>| {
            std::thread::spawn(move || {
                list.iter()
                    .map(|p| (p.clone(), thumbnail(p, "image/png", ThumbSize::Normal)))
                    .collect::<Vec<_>>()
            })
        };
        let mut rev = imgs.clone();
        rev.reverse();
        let a = run(imgs.clone());
        let b = run(rev);
        for (p, r) in a.join().unwrap().into_iter().chain(b.join().unwrap()) {
            assert!(r.is_ok(), "{}: {:?}", p.display(), r.err());
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn every_helper_renders_at_the_size_asked_for() {
        // Quick Look asks for xx-large; a 256px render stretched across the
        // panel was the blur users saw.
        for mime in ["image/png", "application/pdf", "video/mp4"] {
            for (size, want) in [(ThumbSize::Large, "256"), (ThumbSize::XxLarge, "1024")] {
                let hs = helpers_for(mime, size.pixels());
                assert!(!hs.is_empty(), "{mime}");
                for h in hs {
                    assert!(
                        h.args.iter().any(|a| a.contains(want)),
                        "{mime} at {want}: {:?}",
                        h.args
                    );
                }
            }
        }
        // An image is only ever shrunk, never blown up past its own size.
        let img = &helpers_for("image/png", 1024)[0];
        assert!(img.args.iter().any(|a| a == "1024x1024>"));
    }

    #[test]
    fn hd_video_frames_render_in_the_sandbox_at_grid_and_quick_look_sizes() {
        gust_testkit::skip_unless!(
            sandbox_available()
                && gust_testkit::have("ffmpeg")
                && gust_testkit::have("ffmpegthumbnailer"),
            "thumbnails",
            "bwrap, ffmpeg or ffmpegthumbnailer is not installed"
        );
        let d = tmp("hd-video");
        let video = d.join("clip.mp4");
        assert!(Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=1920x1080:rate=25:duration=1",
                "-c:v",
                "libx264",
                "-threads",
                "1",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&video)
            .status()
            .unwrap()
            .success());
        let scratch = d.join("output");
        fs::create_dir(&scratch).unwrap();
        for size in [ThumbSize::Normal, ThumbSize::XxLarge] {
            let png = run_helpers(&video, &helpers_for("video/mp4", size.pixels()), &scratch)
                .unwrap_or_else(|e| panic!("{size:?}: {e}"));
            let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
            let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
            assert_eq!(width, size.pixels());
            assert_eq!(height, size.pixels() * 9 / 16);
        }
        fs::remove_dir_all(d).unwrap();
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
