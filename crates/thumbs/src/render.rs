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

use crate::cache::{cache_base, file_uri, thumbnail_path, thumbnail_path_in, ThumbSize};
use crate::sandbox::{sandbox_available, validate_output, SandboxSpec, ValidationError};
use gust_core::lister::CancelToken;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
        let mut image_args = Vec::new();
        if mime == "image/jpeg" {
            // JPEG can discard DCT coefficients while loading. Decode at
            // twice the final size, keeping enough samples for orientation
            // and the final filter without expanding a multi-megapixel image.
            image_args.extend(args(&[
                "-define",
                &format!("jpeg:size={}x{}", px * 2, px * 2),
            ]));
        }
        image_args.push("/in/input[0]".into());
        if mime == "image/jpeg" {
            // libjpeg also permits a 2x decode for tiny JPEGs. %G is the
            // original file geometry, so cap that before orientation and
            // final scaling to preserve the never-enlarge contract.
            image_args.extend(args(&["-resize", "%G>"]));
        }
        image_args.extend(args(&[
            "-auto-orient",
            "-thumbnail",
            &geom,
            "-strip",
            "-define",
            "png:compression-level=1",
            "png:/out/thumb.png",
        ]));
        v.push(Helper {
            binary: "magick",
            args: image_args,
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
    mime.starts_with("image/") || mime.starts_with("video/") || mime == "application/pdf"
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SourceVersion {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: u64,
    mtime_nsec: i64,
    ctime: i64,
    ctime_nsec: i64,
}

impl SourceVersion {
    fn tag(self) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            self.dev, self.ino, self.len, self.mtime, self.mtime_nsec, self.ctime, self.ctime_nsec
        )
    }
}

fn source_version(path: &Path) -> Result<SourceVersion, ThumbError> {
    let md = path
        .symlink_metadata()
        .map_err(|e| ThumbError::Source(e.to_string()))?;
    if !md.is_file() {
        return Err(ThumbError::Source("not a regular file".into()));
    }
    Ok(SourceVersion {
        dev: md.dev(),
        ino: md.ino(),
        len: md.len(),
        mtime: md
            .mtime()
            .try_into()
            .map_err(|_| ThumbError::Source("no modification time".into()))?,
        mtime_nsec: md.mtime_nsec(),
        ctime: md.ctime(),
        ctime_nsec: md.ctime_nsec(),
    })
}

/// The freedesktop `Thumb::MTime` of the source, as the spec requires.
fn source_mtime(path: &Path) -> Result<u64, ThumbError> {
    source_version(path).map(|v| v.mtime)
}

/// Is there already a valid cached thumbnail for this exact file version?
///
/// Read only a bounded metadata prefix, never the compressed image body.
/// Gust's source-version tag supplements the spec's seconds-only mtime:
/// same-second edits and replacements must not reuse another file's preview.
/// Older thumbnails without that tag are regenerated once.
pub fn cached(path: &Path, size: ThumbSize) -> Option<PathBuf> {
    let target = thumbnail_path(path, size)?;
    let want = source_version(path).ok()?;
    cached_version(&target, want).then_some(target)
}

/// A cache entry checked against a source version on a worker.
#[derive(Clone, Debug)]
pub struct Thumbnail {
    pub path: PathBuf,
    version: SourceVersion,
}

impl Thumbnail {
    /// Changing the source changes Qt's decoded-image cache identity.
    pub fn url(&self) -> String {
        format!("{}?gust={}", file_uri(&self.path), self.version.tag())
    }
}

pub fn cached_thumbnail(path: &Path, size: ThumbSize) -> Option<Thumbnail> {
    let base = cache_base()?;
    let version = source_version(path).ok()?;
    immutable_cached(&base, path, size, version)
}

/// Grid rendering has bounded admission and can stop an already running helper.
pub fn thumbnail_cancellable(
    path: &Path,
    mime: &str,
    size: ThumbSize,
    cancel: &CancelToken,
) -> Result<Thumbnail, ThumbError> {
    let version = source_version(path)?;
    let base = cache_base().ok_or_else(|| ThumbError::Cache("no cache dir".into()))?;
    let target = render_thumbnail(&base, path, mime, size, cancel, true)?;
    verify_source(path, version)?;
    Ok(Thumbnail {
        path: target,
        version,
    })
}

fn immutable_path(base: &Path, source: &Path, size: ThumbSize, version: SourceVersion) -> PathBuf {
    let name = crate::cache::md5_hex(file_uri(source).as_bytes());
    let revision = crate::cache::md5_hex(version.tag().as_bytes());
    base.join("gust-grid")
        .join(size.dir_name())
        .join(format!("{name}-{revision}.png"))
}

fn immutable_cached(
    base: &Path,
    source: &Path,
    size: ThumbSize,
    version: SourceVersion,
) -> Option<Thumbnail> {
    let target = immutable_path(base, source, size, version);
    if !cached_version(&target, version) {
        // Pin the shared inode while checking and promoting its bytes. Checking
        // one pathname and then copying it would race another renderer's rename.
        let bytes = read_cached_png(&thumbnail_path_in(base, source, size), version)?;
        publish_cache(&target, &bytes, false).ok()?;
        if !cached_version(&target, version) {
            return None;
        }
    }
    Some(Thumbnail {
        path: target,
        version,
    })
}

fn read_cached_png(target: &Path, version: SourceVersion) -> Option<Vec<u8>> {
    use rustix::fs::{Mode, OFlags};
    let fd = rustix::fs::open(
        target,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .ok()?;
    read_cached_file(std::fs::File::from(fd), version)
}

fn read_cached_file(file: std::fs::File, version: SourceVersion) -> Option<Vec<u8>> {
    const MAX: u64 = 8 * 1024 * 1024 + 512;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX || png_source_matches(&mut bytes.as_slice(), version) != Some(true)
    {
        return None;
    }
    Some(bytes)
}

/// Stage complete bytes before publication. Immutable destinations are linked
/// without replacement; another process can only win with the same version.
fn publish_cache(target: &Path, bytes: &[u8], replace: bool) -> Result<(), ThumbError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let parent = target
        .parent()
        .ok_or_else(|| ThumbError::Cache("no cache parent".into()))?;
    std::fs::create_dir_all(parent).map_err(|e| ThumbError::Cache(e.to_string()))?;
    let stage = parent.join(format!(".gust-publish-{}-{n}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&stage)
        .map_err(|e| ThumbError::Cache(e.to_string()))?;
    let result = (|| {
        file.write_all(bytes)?;
        drop(file);
        if replace {
            std::fs::rename(&stage, target)
        } else {
            match std::fs::hard_link(&stage, target) {
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                result => result,
            }
        }
    })();
    let _ = std::fs::remove_file(stage);
    result.map_err(|e| ThumbError::Cache(e.to_string()))
}

fn cached_version(target: &Path, want: SourceVersion) -> bool {
    use rustix::fs::{Mode, OFlags};
    let Ok(fd) = rustix::fs::open(
        target,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) else {
        return false;
    };
    let mut file = std::fs::File::from(fd);
    let Ok(md) = file.metadata() else {
        return false;
    };
    // The validated helper output may use its entire 8 MiB budget; allow
    // the bounded source tags Gust adds without rejecting that valid hit.
    if !md.is_file() || md.len() > 8 * 1024 * 1024 + 512 {
        return false;
    }
    // Our two source tags are inserted directly after IHDR and fit in this
    // fixed prefix even at the largest metadata values. No image-sized heap
    // allocation or read is needed for a hit.
    let mut prefix = [0; 512];
    let len = md.len().min(prefix.len() as u64) as usize;
    if file.read_exact(&mut prefix[..len]).is_err() {
        return false;
    }
    png_source_matches(&mut &prefix[..len], want).unwrap_or(false)
}

/// Walk bounded PNG metadata without decoding or allocating for chunk lengths.
fn png_source_matches(reader: &mut impl Read, want: SourceVersion) -> Option<bool> {
    let mut magic = [0; 8];
    reader.read_exact(&mut magic).ok()?;
    if magic != [0x89, b'P', b'N', b'G', 13, 10, 26, 10] {
        return None;
    }
    let want_tag = want.tag();
    let mut mtime = false;
    let mut version = false;
    let mut data = [0; 256];
    for index in 0..64 {
        let mut header = [0; 8];
        reader.read_exact(&mut header).ok()?;
        let len = u32::from_be_bytes(header[..4].try_into().ok()?) as usize;
        let kind = &header[4..];
        if index == 0 && (kind != b"IHDR" || len != 13) {
            return None;
        }
        if kind == b"IDAT" || kind == b"IEND" {
            return Some(false);
        }
        if kind == b"tEXt" && len <= data.len() {
            reader.read_exact(&mut data[..len]).ok()?;
            let text = &data[..len];
            if let Some(value) = text.strip_prefix(b"Thumb::MTime\0") {
                mtime = std::str::from_utf8(value).ok()?.parse::<u64>().ok()? == want.mtime;
                if !mtime {
                    return Some(false);
                }
            } else if let Some(value) = text.strip_prefix(b"Gust::SourceVersion\0") {
                version = value == want_tag.as_bytes();
                if !version {
                    return Some(false);
                }
            }
        } else {
            let mut remaining = len;
            while remaining > 0 {
                let n = remaining.min(data.len());
                reader.read_exact(&mut data[..n]).ok()?;
                remaining -= n;
            }
        }
        let mut crc = [0; 4];
        reader.read_exact(&mut crc).ok()?;
        if mtime && version {
            return Some(true);
        }
    }
    None
}

/// Produce a thumbnail for `path`, returning the cached PNG's location.
///
/// Runs the helper under the sandbox, validates its output through a held
/// descriptor, and writes the freedesktop cache atomically. Returns quickly
/// when a valid cached thumbnail already exists.
pub fn thumbnail(path: &Path, mime: &str, size: ThumbSize) -> Result<PathBuf, ThumbError> {
    let base = cache_base().ok_or_else(|| ThumbError::Cache("no cache dir".into()))?;
    thumbnail_in(&base, path, mime, size)
}

fn thumbnail_in(
    base: &Path,
    path: &Path,
    mime: &str,
    size: ThumbSize,
) -> Result<PathBuf, ThumbError> {
    thumbnail_in_cancel(base, path, mime, size, &CancelToken::new())
}

fn thumbnail_in_cancel(
    base: &Path,
    path: &Path,
    mime: &str,
    size: ThumbSize,
    cancel: &CancelToken,
) -> Result<PathBuf, ThumbError> {
    render_thumbnail(base, path, mime, size, cancel, false)
}

fn render_thumbnail(
    base: &Path,
    path: &Path,
    mime: &str,
    size: ThumbSize,
    cancel: &CancelToken,
    immutable: bool,
) -> Result<PathBuf, ThumbError> {
    check_cancel(cancel)?;
    let version = source_version(path)?;
    let target = thumbnail_path_in(base, path, size);
    if immutable {
        if let Some(thumbnail) = immutable_cached(base, path, size, version) {
            return Ok(thumbnail.path);
        }
    } else if cached_version(&target, version) {
        return Ok(target);
    }
    if !sandbox_available() {
        return Err(ThumbError::SandboxUnavailable);
    }
    let helpers = helpers_for(mime, size.pixels());
    if helpers.is_empty() {
        return Err(ThumbError::NoHelper);
    }
    let mtime = version.mtime;

    // Private scratch for the helper's output, inside our own cache. Unique
    // per render: files copied in the same second share an mtime, and two
    // workers (a listing that refreshed while the last pass still ran) then
    // deleted each other's output, leaving a copied photo without a preview.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let scratch = base.join(format!(".gust-thumb-{}-{}-{n}", std::process::id(), mtime));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).map_err(|e| ThumbError::Cache(e.to_string()))?;

    let result = run_helpers_cancel(path, &helpers, &scratch, cancel);

    let out = match result {
        Ok(bytes) => bytes,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&scratch);
            return Err(e);
        }
    };

    // A helper may have read while the source was edited or replaced. Never
    // publish those bytes as a verified preview of either version.
    if let Err(e) = check_cancel(cancel).and_then(|_| verify_source(path, version)) {
        let _ = std::fs::remove_dir_all(&scratch);
        return Err(e);
    }
    let tagged = with_source_text(&out, version);

    let write = publish_rendered(base, path, size, version, &tagged, immutable);
    let _ = std::fs::remove_dir_all(&scratch);
    write
}

fn publish_rendered(
    base: &Path,
    source: &Path,
    size: ThumbSize,
    version: SourceVersion,
    tagged: &[u8],
    immutable: bool,
) -> Result<PathBuf, ThumbError> {
    let target = thumbnail_path_in(base, source, size);
    let immutable_target = immutable_path(base, source, size, version);
    if immutable {
        publish_cache(&immutable_target, tagged, false)?;
    }
    publish_cache(&target, tagged, true)?;
    if immutable {
        if !cached_version(&immutable_target, version) {
            return Err(ThumbError::Cache("immutable preview is invalid".into()));
        }
        Ok(immutable_target)
    } else {
        Ok(target)
    }
}

fn verify_source(path: &Path, version: SourceVersion) -> Result<(), ThumbError> {
    if source_version(path)? == version {
        Ok(())
    } else {
        Err(ThumbError::Source("file changed while rendering".into()))
    }
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
    run_helpers_cancel(path, helpers, scratch, &CancelToken::new())
}

fn check_cancel(cancel: &CancelToken) -> Result<(), ThumbError> {
    if cancel.is_cancelled() {
        Err(ThumbError::HelperFailed("cancelled".into()))
    } else {
        Ok(())
    }
}

// Shared across tabs and windows in this process, including cancelled passes
// that have not yet retired their helper. Hits never wait for admission.
static HELPERS: std::sync::Mutex<usize> = std::sync::Mutex::new(0);
struct Permit;
impl Permit {
    fn acquire(cancel: &CancelToken) -> Result<Self, ThumbError> {
        loop {
            check_cancel(cancel)?;
            let mut active = HELPERS
                .lock()
                .map_err(|_| ThumbError::HelperFailed("admission unavailable".into()))?;
            if *active < 2 {
                *active += 1;
                return Ok(Self);
            }
            drop(active);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        if let Ok(mut active) = HELPERS.lock() {
            *active -= 1;
        }
    }
}

fn run_helpers_cancel(
    path: &Path,
    helpers: &[Helper],
    scratch: &Path,
    cancel: &CancelToken,
) -> Result<Vec<u8>, ThumbError> {
    let input = crate::helper::Input::open(path, u64::MAX)
        .map_err(|_| ThumbError::Source("cannot open preview input".into()))?;
    let mut last = ThumbError::NoHelper;
    for h in helpers {
        check_cancel(cancel)?;
        let mut spec = SandboxSpec::new(path, scratch);
        if h.binary == "ffmpegthumbnailer" {
            // Decoder/scaler thread stacks can exhaust 512 MiB of virtual
            // address space even for 1080p video, leaving every tile blank.
            // Match the audio decoder's budget; keep all other limits.
            spec.address_space_bytes = 1024 * 1024 * 1024;
        }
        let hargs: Vec<&str> = h.args.iter().map(String::as_str).collect();
        let argv = crate::helper::argv(&spec, h.binary, &hargs);
        let out_file = scratch.join("thumb.png");
        let _ = std::fs::remove_file(&out_file);

        let permit = Permit::acquire(cancel)?;
        let status = crate::helper::OwnedChild::spawn_input(&argv, &input, Stdio::null()).and_then(
            |mut child| {
                child.retain_until_exit(permit);
                crate::helper::wait(
                    &mut child,
                    cancel,
                    std::time::Duration::from_secs(u64::from(spec.wall_timeout_seconds)),
                    scratch,
                )
            },
        );

        match status {
            Ok(s) if s.success() => match validate_output(&out_file, spec.max_output_bytes) {
                Ok(bytes) => return Ok(bytes),
                Err(e) => last = ThumbError::BadOutput(e),
            },
            Ok(s) => last = ThumbError::HelperFailed(format!("{} exited with {s}", h.binary)),
            Err(e) => last = ThumbError::HelperFailed(format!("{}: {e:?}", h.binary)),
        }
    }
    Err(last)
}

/// Encode a PNG tEXt chunk, including the CRC required by PNG readers.
fn text_chunk(name: &[u8], value: &[u8]) -> Vec<u8> {
    let mut key = name.to_vec();
    key.push(0);
    key.extend_from_slice(value);

    let mut chunk = Vec::new();
    chunk.extend_from_slice(&(key.len() as u32).to_be_bytes());
    chunk.extend_from_slice(b"tEXt");
    chunk.extend_from_slice(&key);
    let mut crc_input = b"tEXt".to_vec();
    crc_input.extend_from_slice(&key);
    chunk.extend_from_slice(&crc32(&crc_input).to_be_bytes());

    chunk
}

fn with_source_text(png: &[u8], version: SourceVersion) -> Vec<u8> {
    let mut chunk = text_chunk(b"Thumb::MTime", version.mtime.to_string().as_bytes());
    chunk.extend_from_slice(&text_chunk(
        b"Gust::SourceVersion",
        version.tag().as_bytes(),
    ));
    let mut out = Vec::with_capacity(png.len() + chunk.len());
    // Header, then IHDR (which must stay first), then our text chunk.
    let ihdr_end = if png.len() >= 12 {
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
        let cache = d.join("cache/thumbnails");
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
            let cache = cache.clone();
            std::thread::spawn(move || {
                list.iter()
                    .map(|p| {
                        (
                            p.clone(),
                            thumbnail_in(&cache, p, "image/png", ThumbSize::Normal),
                        )
                    })
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
    fn jpeg_decode_hints_keep_aspect_ratio_and_never_enlarge_small_images() {
        gust_testkit::skip_unless!(
            sandbox_available() && gust_testkit::have("magick"),
            "thumbnails",
            "bwrap or ImageMagick is not installed"
        );
        let d = tmp("jpeg-hint");
        let cache = d.join("cache");
        for (name, geometry, expected) in [
            ("photo.jpg", "2400x1600", [(128, 85), (1024, 683)]),
            ("rotated.jpg", "2400x1600", [(85, 128), (683, 1024)]),
            ("small.jpg", "61x47", [(61, 47), (61, 47)]),
        ] {
            let source = d.join(name);
            assert!(Command::new("magick")
                .args(["-size", geometry, "gradient:"])
                .arg(&source)
                .status()
                .unwrap()
                .success());
            if name == "rotated.jpg" {
                // Minimal little-endian EXIF IFD with Orientation=6.
                let jpeg = fs::read(&source).unwrap();
                let exif =
                    b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
                let mut tagged = jpeg[..2].to_vec();
                tagged.extend_from_slice(&[0xff, 0xe1]);
                tagged.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
                tagged.extend_from_slice(exif);
                tagged.extend_from_slice(&jpeg[2..]);
                fs::write(&source, tagged).unwrap();
            }
            for (size, (width, height)) in [ThumbSize::Normal, ThumbSize::XxLarge]
                .into_iter()
                .zip(expected)
            {
                let target = thumbnail_in(&cache, &source, "image/jpeg", size).unwrap();
                let png = validate_output(&target, 8 * 1024 * 1024).unwrap();
                assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), width);
                assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), height);
                assert!(cached_version(&target, source_version(&source).unwrap()));
                // A valid hit needs neither the original helper nor a
                // supported MIME, so this exercises the actual hit branch.
                assert_eq!(
                    thumbnail_in(&cache, &source, "text/plain", size).unwrap(),
                    target
                );
            }
        }
        fs::remove_dir_all(d).unwrap();
    }

    fn png_header() -> Vec<u8> {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&[0u8; 13]);
        png.extend_from_slice(&0u32.to_be_bytes());
        png
    }

    fn example_version() -> SourceVersion {
        SourceVersion {
            dev: 1,
            ino: 2,
            len: 3,
            mtime: 1_726_000_000,
            mtime_nsec: 4,
            ctime: 5,
            ctime_nsec: 6,
        }
    }

    #[test]
    fn source_tags_round_trip_without_reading_image_data() {
        let png = png_header();
        let version = example_version();
        let tagged = with_source_text(&png, version);
        // A reader ending immediately after the tags proves that a cache
        // hit never needs to read/decode the compressed image body.
        assert_eq!(
            png_source_matches(&mut tagged.as_slice(), version),
            Some(true)
        );
        assert_eq!(&tagged[..8], &png[..8], "the PNG header must stay intact");
        assert_eq!(&tagged[12..16], b"IHDR", "IHDR must stay first");
    }

    #[test]
    fn decoded_cache_urls_encode_paths_and_separate_file_versions() {
        let version = example_version();
        let thumbnail = Thumbnail {
            path: PathBuf::from("/cache/a #?.png"),
            version,
        };
        let url = thumbnail.url();
        assert!(url.starts_with("file:///cache/a%20%23%3F.png?gust="));
        assert_eq!(thumbnail.clone().url(), url);
        for changed in [
            SourceVersion {
                ino: version.ino + 1,
                ..version
            },
            SourceVersion {
                mtime_nsec: version.mtime_nsec + 1,
                ..version
            },
            SourceVersion {
                ctime_nsec: version.ctime_nsec + 1,
                ..version
            },
        ] {
            assert_ne!(
                Thumbnail {
                    path: thumbnail.path.clone(),
                    version: changed
                }
                .url(),
                url
            );
        }
    }

    #[test]
    fn late_old_renderer_cannot_replace_pixels_behind_a_new_version_url() {
        let d = tmp("immutable-race");
        let source = d.join("source.png");
        let base = d.join("cache");
        fs::write(&source, b"old source").unwrap();
        let old = source_version(&source).unwrap();
        let mut old_bytes = with_source_text(&png_header(), old);
        old_bytes.extend_from_slice(b"old pixels");
        let (checked_tx, checked_rx) = std::sync::mpsc::sync_channel(0);
        let (resume_tx, resume_rx) = std::sync::mpsc::sync_channel(0);
        std::thread::scope(|scope| {
            let source = &source;
            let base = &base;
            let old_bytes = &old_bytes;
            let late = scope.spawn(move || {
                verify_source(source, old).unwrap();
                checked_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
                publish_rendered(base, source, ThumbSize::Normal, old, old_bytes, true).unwrap();
                assert!(verify_source(source, old).is_err());
            });
            checked_rx.recv().unwrap();
            let replacement = d.join("replacement.png");
            fs::write(&replacement, b"new source").unwrap();
            fs::rename(replacement, source).unwrap();
            let new = source_version(source).unwrap();
            let mut new_bytes = with_source_text(&png_header(), new);
            new_bytes.extend_from_slice(b"new pixels");
            let published =
                publish_rendered(base, source, ThumbSize::Normal, new, &new_bytes, true).unwrap();
            let url = Thumbnail {
                path: published.clone(),
                version: new,
            }
            .url();
            resume_tx.send(()).unwrap();
            late.join().unwrap();
            assert_eq!(
                fs::read(thumbnail_path_in(base, source, ThumbSize::Normal)).unwrap(),
                *old_bytes
            );
            assert_eq!(fs::read(&published).unwrap(), new_bytes);
            let cached = immutable_cached(base, source, ThumbSize::Normal, new).unwrap();
            assert_eq!(cached.url(), url);
            assert_eq!(fs::read(cached.path).unwrap(), new_bytes);
            // Even a duplicate publication cannot replace an immutable inode.
            publish_cache(&published, old_bytes, false).unwrap();
            assert_eq!(fs::read(published).unwrap(), new_bytes);
        });
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn shared_cache_promotion_reads_the_checked_inode_after_a_path_replacement() {
        let d = tmp("immutable-promotion");
        let target = d.join("shared.png");
        let version = example_version();
        let mut wanted = with_source_text(&png_header(), version);
        wanted.extend_from_slice(b"wanted pixels");
        publish_cache(&target, &wanted, true).unwrap();
        let held = fs::File::open(&target).unwrap();
        let other = with_source_text(
            &png_header(),
            SourceVersion {
                ino: version.ino + 1,
                ..version
            },
        );
        publish_cache(&target, &other, true).unwrap();
        assert_eq!(read_cached_file(held, version), Some(wanted));
        assert!(read_cached_png(&target, version).is_none());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn cancelled_grid_request_does_not_enter_a_helper() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let d = tmp("cancel-grid");
        let source = d.join("source.png");
        fs::write(&source, b"invalid image").unwrap();
        assert!(thumbnail_in_cancel(
            &d.join("cache"),
            &source,
            "image/png",
            ThumbSize::Normal,
            &cancel
        )
        .is_err());
        assert!(!d.join("cache").exists());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn helper_admission_is_process_wide_and_cancellable() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let active = &active;
                let peak = &peak;
                scope.spawn(move || {
                    let _permit = Permit::acquire(&CancelToken::new()).unwrap();
                    let n = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(n, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        assert!((1..=2).contains(&peak.load(Ordering::SeqCst)));
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(Permit::acquire(&cancel).is_err());
    }

    #[test]
    fn legacy_seconds_only_tags_cannot_prove_the_source_version() {
        let version = example_version();
        let mut png = png_header();
        png.extend(text_chunk(
            b"Thumb::MTime",
            version.mtime.to_string().as_bytes(),
        ));
        assert_ne!(png_source_matches(&mut png.as_slice(), version), Some(true));
    }

    #[test]
    fn edits_and_replacements_with_the_same_second_mtime_invalidate_the_cache() {
        let d = tmp("source-version");
        let source = d.join("source.png");
        let target = d.join("thumbnail.png");
        fs::write(&source, b"original").unwrap();
        let epoch = std::time::UNIX_EPOCH + std::time::Duration::new(1_760_000_000, 100);
        fs::File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_modified(epoch)
            .unwrap();
        let original = source_version(&source).unwrap();
        fs::write(&target, with_source_text(&png_header(), original)).unwrap();
        assert!(cached_version(&target, original));

        fs::write(&source, b"modified").unwrap();
        fs::File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_modified(epoch + std::time::Duration::from_nanos(100))
            .unwrap();
        let edited = source_version(&source).unwrap();
        assert_eq!(edited.mtime, original.mtime);
        assert!(!cached_version(&target, edited));
        assert!(verify_source(&source, original).is_err());

        let replacement = d.join("replacement.png");
        fs::write(&replacement, b"replaced").unwrap();
        fs::File::options()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_modified(epoch)
            .unwrap();
        fs::rename(&replacement, &source).unwrap();
        let replaced = source_version(&source).unwrap();
        assert_eq!(replaced.mtime, original.mtime);
        assert_eq!(replaced.mtime_nsec, original.mtime_nsec);
        assert_eq!(replaced.len, original.len);
        assert_ne!(replaced.ino, original.ino);
        assert!(!cached_version(&target, replaced));
        assert!(verify_source(&source, original).is_err());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn cache_metadata_is_bounded_and_rejects_symlinks_fifos_and_oversize_files() {
        let d = tmp("cache-bounds");
        let version = example_version();
        let real = d.join("real.png");
        fs::write(&real, with_source_text(&png_header(), version)).unwrap();
        assert!(cached_version(&real, version));
        let link = d.join("link.png");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(!cached_version(&link, version));
        assert!(!cached_version(&d, version));

        let fifo = d.join("fifo.png");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_bits_truncate(0o600),
            0,
        )
        .unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            tx.send(cached_version(&fifo, version)).unwrap();
        });
        assert!(!rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("FIFO cache must not block"));

        fs::File::options()
            .write(true)
            .open(&real)
            .unwrap()
            .set_len(8 * 1024 * 1024 + 513)
            .unwrap();
        assert!(!cached_version(&real, version));
        // Valid source tags beyond the read budget must not admit a hit.
        let mut reordered = png_header();
        reordered.extend(text_chunk(b"Other", &vec![b'x'; 70 * 1024]));
        reordered.extend(text_chunk(
            b"Thumb::MTime",
            version.mtime.to_string().as_bytes(),
        ));
        reordered.extend(text_chunk(b"Gust::SourceVersion", version.tag().as_bytes()));
        fs::write(&real, reordered).unwrap();
        assert!(!cached_version(&real, version));
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn a_truncated_png_does_not_panic_the_scanner() {
        for n in 0..40usize {
            let bytes = vec![0x89u8; n];
            let _ = png_source_matches(&mut bytes.as_slice(), example_version());
        }
        // A chunk claiming a huge length must not read out of bounds.
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&u32::MAX.to_be_bytes());
        png.extend_from_slice(b"tEXt");
        assert_eq!(
            png_source_matches(&mut png.as_slice(), example_version()),
            None
        );
        for n in 0..12 {
            let _ = with_source_text(&png[..n], example_version());
        }
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
