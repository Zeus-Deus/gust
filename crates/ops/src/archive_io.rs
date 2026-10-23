//! Reading and writing archives (DESIGN §6.15, plan §4 and mockup C).
//!
//! The *policy* — what may be extracted and what must be refused — lives in
//! [`super`] and has been tested since early on. This module is the part
//! that was missing: getting a real archive's entries so the policy has
//! something to judge, and writing one back out.
//!
//! Listing never writes. Extraction vets every entry before a byte lands,
//! stages inside the destination, and commits with a rename.

use super::protocol::{Frame, Member};
use super::{ArchiveEntry, ArchiveEntryKind, ExtractPolicy};
use gust_core::lister::CancelToken;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// At most this many entries are parsed for display. An archive with more is
/// still extractable; the *listing* is bounded so a malicious index cannot
/// make the UI chew through millions of lines.
pub const MAX_LISTED: usize = 2000;

/// Why an archive could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveError {
    NoTool,
    SandboxUnavailable,
    Unsupported(String),
    Failed(String),
    Cancelled,
    Limit(String),
    /// Publication began; this receipt is authoritative even on failure.
    Partial {
        receipt: Extracted,
        cause: Box<ArchiveError>,
    },
}
impl ArchiveError {
    pub fn message(&self) -> String {
        match self {
            Self::NoTool => "Archives need gust-archive-helper (libarchive/bsdtar support).".into(),
            Self::SandboxUnavailable => {
                "Archives need a working bubblewrap and prlimit sandbox.".into()
            }
            Self::Unsupported(name) => format!("Gust cannot read {name} as an archive."),
            Self::Failed(why) | Self::Limit(why) => why.clone(),
            Self::Cancelled => "Archive operation cancelled.".into(),
            Self::Partial { receipt, cause } => {
                format!("{}; stopped: {}", receipt.summary(), cause.message())
            }
        }
    }
}
impl From<std::io::Error> for ArchiveError {
    fn from(error: std::io::Error) -> Self {
        Self::Failed(error.to_string())
    }
}

impl From<rustix::io::Errno> for ArchiveError {
    fn from(error: rustix::io::Errno) -> Self {
        std::io::Error::from(error).into()
    }
}

/// A password is moved to the worker and sent only on the helper's stdin.
/// Debug is redacted, and dropping it wipes this owned buffer.
pub struct ArchivePassword(Vec<u8>);
impl ArchivePassword {
    pub fn new(bytes: Vec<u8>) -> Result<Self, ArchiveError> {
        let secret = Self(bytes);
        if secret.0.is_empty()
            || secret.0.len() > 1024
            || secret.0.contains(&0)
            || secret.0.contains(&b'\n')
            || secret.0.contains(&b'\r')
        {
            return Err(ArchiveError::Failed(
                "Archive password must be 1–1024 bytes without NUL or line breaks.".into(),
            ));
        }
        Ok(secret)
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.0
    }
}
impl std::fmt::Debug for ArchivePassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ArchivePassword([redacted])")
    }
}
impl Drop for ArchivePassword {
    fn drop(&mut self) {
        super::protocol::wipe(&mut self.0);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    TarZstd,
    Zip,
    SevenZip,
}

fn helper_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("GUST_ARCHIVE_HELPER") {
        let path = PathBuf::from(path);
        return path.is_absolute().then_some(path).filter(|p| p.is_file());
    }
    let executable = std::env::current_exe().ok()?;
    let directory = executable.parent()?;
    [
        directory.join("gust-archive-helper"),
        directory.parent()?.join("gust-archive-helper"),
        PathBuf::from("/usr/bin/gust-archive-helper"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

pub fn tool_available() -> bool {
    helper_path().is_some()
        && Path::new("/usr/bin/bwrap").is_file()
        && Path::new("/usr/bin/prlimit").is_file()
}

/// Does this name look like something worth offering to open as an archive?
///
/// The name is a hint, never proof — the listing is what decides. Gust does
/// not run a file to find out what it is.
pub fn looks_like_archive(name: &str) -> bool {
    let lower = name.to_lowercase();
    const ENDINGS: &[&str] = &[
        ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tar.xz", ".txz", ".tar.zst", ".tzst",
        ".zip", ".7z", ".rar", ".jar", ".gz", ".xz", ".zst",
    ];
    ENDINGS.iter().any(|e| lower.ends_with(e))
}

/// Parse one line of `bsdtar -tv` output into an entry.
///
/// The format is `mode links owner group size date... name`, with a link
/// target after " -> " for symlinks. Deliberately tolerant: a line that
/// cannot be parsed is skipped rather than guessed at.
pub fn parse_listing_line(line: &str) -> Option<ArchiveEntry> {
    let line = line.trim_end();
    if line.trim().is_empty() {
        return None;
    }
    let mut fields = line.split_whitespace();
    let mode_str = fields.next()?;
    if mode_str.len() < 10 {
        return None;
    }
    // Skip links, owner, group.
    let _links = fields.next()?;
    let _owner = fields.next()?;
    let _group = fields.next()?;
    let size: u64 = fields.next()?.parse().ok()?;

    // A date is "Mon DD HH:MM" or "Mon DD  YYYY" — three whitespace fields
    // after the size. Walk positionally: matching on " {size} " finds the
    // LINKS column when the size is 0, which swallowed the date into the
    // name.
    let mut cursor = line;
    for _ in 0..8 {
        let t = cursor.trim_start();
        let cut = t.find(char::is_whitespace)?;
        cursor = &t[cut..];
    }
    let name_part = cursor.trim_start().to_string();
    if name_part.is_empty() {
        return None;
    }

    let (name, link_target) = match name_part.split_once(" -> ") {
        Some((n, t)) => (n.to_string(), Some(t.to_string())),
        None => (name_part, None),
    };

    let kind = match mode_str.as_bytes()[0] {
        b'd' => ArchiveEntryKind::Directory,
        b'l' => ArchiveEntryKind::Symlink,
        b'h' => ArchiveEntryKind::Hardlink,
        b'p' | b'c' | b'b' | b's' => ArchiveEntryKind::Special,
        _ => ArchiveEntryKind::Regular,
    };

    Some(ArchiveEntry {
        name: name.trim_end_matches('/').to_string(),
        kind,
        link_target,
        size,
        mode: parse_mode(mode_str),
    })
}

/// Turn `-rwsr-xr-x` into 0o4755, so setuid entries can be refused.
fn parse_mode(s: &str) -> u32 {
    let b = s.as_bytes();
    if b.len() < 10 {
        return 0;
    }
    let bit = |i: usize, ch: u8| if b[i] == ch { 1 } else { 0 };
    let mut mode = 0u32;
    mode |= bit(1, b'r') << 8;
    mode |= bit(2, b'w') << 7;
    mode |= u32::from(b[3] == b'x' || b[3] == b's') << 6;
    mode |= bit(4, b'r') << 5;
    mode |= bit(5, b'w') << 4;
    mode |= u32::from(b[6] == b'x' || b[6] == b's') << 3;
    mode |= bit(7, b'r') << 2;
    mode |= bit(8, b'w') << 1;
    mode |= u32::from(b[9] == b'x' || b[9] == b't');
    // setuid / setgid / sticky, which the policy refuses.
    if b[3] == b's' || b[3] == b'S' {
        mode |= 0o4000;
    }
    if b[6] == b's' || b[6] == b'S' {
        mode |= 0o2000;
    }
    mode
}

/// Compatibility command builder. `false` now refuses the unsandboxed path.
/// Production parsing uses the structured helper, never this human listing.
pub fn list_command(path: &Path, sandbox: bool) -> std::io::Result<Command> {
    if !sandbox {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "archive sandbox is required",
        ));
    }
    let file = std::fs::File::open(path)?;
    let mut c = Command::new("bwrap");
    c.args([
        "--unshare-all",
        "--die-with-parent",
        "--new-session",
        "--clearenv",
        "--setenv",
        "PATH",
        "/usr/bin",
        "--ro-bind",
        "/usr",
        "/usr",
        "--symlink",
        "usr/lib",
        "/lib",
        "--symlink",
        "usr/lib64",
        "/lib64",
        "--symlink",
        "usr/bin",
        "/bin",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--chdir",
        "/",
        "prlimit",
        "--cpu=20",
        "--as=536870912",
        "--nproc=64",
        "timeout",
        "30",
        "bsdtar",
        "-tvf",
        "-",
    ])
    .stdin(Stdio::from(file));
    Ok(c)
}

#[derive(Debug, Clone)]
pub struct ArchivePreview {
    pub entries: Vec<ArchiveEntry>,
    pub truncated: bool,
}

pub fn list(path: &Path) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    Ok(list_preview(path, &gust_core::lister::CancelToken::new())?.entries)
}

pub fn list_preview(
    path: &Path,
    cancel: &gust_core::lister::CancelToken,
) -> Result<ArchivePreview, ArchiveError> {
    let mut reader = Reader::open(path, None, cancel)?;
    let mut entries = Vec::new();
    loop {
        match reader.frame(cancel)? {
            super::protocol::Frame::Header(member) => {
                if entries.len() == MAX_LISTED {
                    return Ok(ArchivePreview {
                        entries,
                        truncated: true,
                    });
                }
                entries.push(display_member(&member));
                reader.decision(false)?;
                if !matches!(reader.frame(cancel)?, super::protocol::Frame::EndMember) {
                    return Err(protocol_error());
                }
            }
            super::protocol::Frame::EndArchive => {
                reader.finish(cancel)?;
                return Ok(ArchivePreview {
                    entries,
                    truncated: false,
                });
            }
            _ => return Err(protocol_error()),
        }
    }
}

/// Byte-preserving member metadata, independent of the display preview.
pub use super::protocol::Member as ArchiveMember;

/// Complete header enumeration, independently bounded by execution policy.
/// These are raw member identities; display strings must never address files.
pub fn enumerate(
    path: &Path,
    policy: &ExtractPolicy,
    cancel: &gust_core::lister::CancelToken,
) -> Result<Vec<ArchiveMember>, ArchiveError> {
    let mut reader = Reader::open(path, None, cancel)?;
    let mut entries = Vec::new();
    let mut metadata = 0usize;
    loop {
        match reader.frame(cancel)? {
            super::protocol::Frame::Header(member) => {
                check_header_limits(&member, entries.len(), &mut metadata, policy)?;
                entries.push(member);
                reader.decision(false)?;
                if !matches!(reader.frame(cancel)?, super::protocol::Frame::EndMember) {
                    return Err(protocol_error());
                }
            }
            super::protocol::Frame::EndArchive => {
                reader.finish(cancel)?;
                return Ok(entries);
            }
            _ => return Err(protocol_error()),
        }
    }
}

/// A name in `dir` that is not taken: `photos.tar.zst`, then
/// `photos-2.tar.zst`, and so on.
///
/// Compressing twice must not silently replace the first archive.
pub fn unique_name(dir: &Path, wanted: &str) -> PathBuf {
    let candidate = dir.join(wanted);
    if !candidate.exists() {
        return candidate;
    }
    // Split on the FULL compound extension, so "a.tar.zst" becomes
    // "a" + ".tar.zst" rather than "a.tar" + ".zst".
    let (stem, ext) = match wanted.find(".tar.") {
        Some(i) => (&wanted[..i], &wanted[i..]),
        None => match wanted.rsplit_once('.') {
            Some((s, e)) => (s, e),
            None => (wanted, ""),
        },
    };
    let dot = if ext.starts_with('.') || ext.is_empty() {
        ""
    } else {
        "."
    };
    for n in 2..10_000 {
        let candidate = dir.join(format!("{stem}-{n}{dot}{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(wanted)
}

/// What an extraction did, for reporting.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extracted {
    pub files: usize,
    pub dirs: usize,
    /// Entries the policy refused. Shown to the user, never swallowed.
    pub refused: Vec<String>,
    /// Entries that clashed with something already there and landed under a
    /// free name instead, as `(archive name, name used)`. The existing file
    /// is the user's and is never replaced.
    pub renamed: Vec<(String, String)>,
    /// Exact created objects, captured after publication for recovery.
    pub published: Vec<(PathBuf, crate::journal::RecordedIdentity)>,
}

impl Extracted {
    pub fn summary(&self) -> String {
        let mut s = match (self.files, self.dirs) {
            (0, 0) => "Nothing was extracted".to_string(),
            (f, 0) => format!("Extracted {f} file{}", plural(f)),
            (0, d) => format!("Extracted {d} folder{}", plural(d)),
            (f, d) => format!(
                "Extracted {f} file{} and {d} folder{}",
                plural(f),
                plural(d)
            ),
        };
        if !self.renamed.is_empty() {
            s.push_str(&format!(
                " · {} renamed to keep existing file{}",
                self.renamed.len(),
                plural(self.renamed.len())
            ));
        }
        if !self.refused.is_empty() {
            s.push_str(&format!(
                " · refused {} unsafe entr{}",
                self.refused.len(),
                if self.refused.len() == 1 { "y" } else { "ies" }
            ));
        }
        s
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// The argv for extracting into a staging directory.
///
/// `--` ends option parsing, so an archive named `--one-file-system` is a
/// file name rather than a flag. `-P` is deliberately NOT passed: without
/// it bsdtar refuses absolute paths itself, which is a second line of
/// defence behind the policy.
pub fn extract_args(archive: &Path, staging: &Path) -> Vec<String> {
    vec![
        "-xf".into(),
        archive.to_string_lossy().into_owned(),
        "-C".into(),
        staging.to_string_lossy().into_owned(),
        "--no-same-owner".into(),
        "--no-same-permissions".into(),
    ]
}

/// The argv for compressing `names` (relative to `dir`) into `archive`.
///
/// Every name is a separate argument after `--`, so a file called
/// `--checkpoint-action=exec=sh` cannot become an option.
pub fn compress_args(dir: &Path, archive: &Path, names: &[String]) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-c".into(),
        "--zstd".into(),
        "-f".into(),
        archive.to_string_lossy().into_owned(),
        "-C".into(),
        dir.to_string_lossy().into_owned(),
        "--".into(),
    ];
    a.extend(names.iter().cloned());
    a
}

/// Extract an archive into `dest`, refusing anything the policy rejects.
///
/// Stages inside the destination so the commit is a same-filesystem rename,
/// and removes the staging directory on failure: a half-extracted tree that
/// looks like success is worse than an error.
pub fn extract(
    archive: &Path,
    dest: &Path,
    policy: &ExtractPolicy,
) -> Result<Extracted, ArchiveError> {
    extract_with(
        archive,
        dest,
        policy,
        None,
        &gust_core::lister::CancelToken::new(),
        &mut |_, _, _| {},
    )
}

fn protocol_error() -> ArchiveError {
    ArchiveError::Failed("Archive helper returned an incomplete or invalid member stream.".into())
}

struct Reader {
    child: Option<std::process::Child>,
    input: std::process::ChildStdin,
    output: std::process::ChildStdout,
    archive: std::fs::File,
    identity: crate::anchor::FileIdentity,
    started: Instant,
}
impl Reader {
    fn open(
        path: &Path,
        password: Option<&ArchivePassword>,
        cancel: &CancelToken,
    ) -> Result<Self, ArchiveError> {
        if cancel.is_cancelled() {
            return Err(ArchiveError::Cancelled);
        }
        let helper = helper_path().ok_or(ArchiveError::NoTool)?;
        if !Path::new("/usr/bin/bwrap").is_file() || !Path::new("/usr/bin/prlimit").is_file() {
            return Err(ArchiveError::SandboxUnavailable);
        }
        let (parent, name) = crate::anchor::resolve_parent(path)?;
        let file = std::fs::File::from(rustix::fs::openat(
            parent.as_fd(),
            &name,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )?);
        if !file.metadata()?.is_file() {
            return Err(ArchiveError::Failed(
                "Archive input is not an ordinary file.".into(),
            ));
        }
        let identity = crate::anchor::FileIdentity::from_stat(&rustix::fs::fstat(&file)?);
        // Pass an owned FD directly: user namespaces can refuse reopening a
        // parent process's /proc/<pid>/fd path even for the same user.
        let inherited = file.try_clone()?;
        let held = inherited.as_raw_fd().to_string();
        let mut command = Command::new("/usr/bin/bwrap");
        command
            .args([
                "--unshare-all",
                "--die-with-parent",
                "--new-session",
                "--clearenv",
                "--setenv",
                "PATH",
                "/usr/bin",
                "--setenv",
                "LC_ALL",
                "C.UTF-8",
                "--ro-bind",
                "/usr",
                "/usr",
                "--symlink",
                "usr/lib",
                "/lib",
                "--symlink",
                "usr/lib",
                "/lib64",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--ro-bind-fd",
            ])
            .arg(held)
            .arg("/input")
            .arg("--ro-bind")
            .arg(helper)
            .arg("/helper")
            .args([
                "--chdir",
                "/",
                "/usr/bin/prlimit",
                "--cpu=60",
                "--as=536870912",
                "--nproc=16",
                "--nofile=64",
                "--fsize=0",
                "--",
                "/helper",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        use std::os::unix::process::CommandExt;
        // SAFETY: after fork this closure only performs async-signal-safe
        // fcntl on its owned descriptor. Parent FD flags remain untouched.
        unsafe {
            command.pre_exec(move || {
                rustix::io::fcntl_setfd(&inherited, rustix::io::FdFlags::empty())
                    .map_err(std::io::Error::from)
            });
        }
        let mut child = command
            .spawn()
            .map_err(|_| ArchiveError::SandboxUnavailable)?;
        let input = child.stdin.take().ok_or_else(protocol_error)?;
        let output = child.stdout.take().ok_or_else(protocol_error)?;
        rustix::fs::fcntl_setfl(
            &output,
            rustix::fs::fcntl_getfl(&output)? | rustix::fs::OFlags::NONBLOCK,
        )?;
        let mut reader = Self {
            child: Some(child),
            input,
            output,
            archive: file,
            identity,
            started: Instant::now(),
        };
        let mut request = super::protocol::ReadRequest {
            password: password.map(|p| p.bytes().to_vec()).unwrap_or_default(),
        };
        let mut bytes = serde_json::to_vec(&request).map_err(|_| protocol_error())?;
        super::protocol::wipe(&mut request.password);
        let result = reader
            .input
            .write_all(&(bytes.len() as u32).to_le_bytes())
            .and_then(|_| reader.input.write_all(&bytes))
            .and_then(|_| reader.input.flush());
        super::protocol::wipe(&mut bytes);
        result?;
        Ok(reader)
    }
    fn check(&self, cancel: &CancelToken) -> Result<(), ArchiveError> {
        if cancel.is_cancelled() {
            return Err(ArchiveError::Cancelled);
        }
        if self.started.elapsed() > Duration::from_secs(120) {
            return Err(ArchiveError::Limit(
                "Archive helper exceeded its two-minute deadline.".into(),
            ));
        }
        Ok(())
    }
    fn read_exact(
        &mut self,
        mut bytes: &mut [u8],
        cancel: &CancelToken,
    ) -> Result<(), ArchiveError> {
        while !bytes.is_empty() {
            self.check(cancel)?;
            match self.output.read(bytes) {
                Ok(0) => return Err(protocol_error()),
                Ok(n) => {
                    bytes = &mut bytes[n..];
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return Err(protocol_error()),
            }
        }
        Ok(())
    }
    fn frame(&mut self, cancel: &CancelToken) -> Result<Frame, ArchiveError> {
        let mut length = [0; 4];
        self.read_exact(&mut length, cancel)?;
        let length = u32::from_le_bytes(length) as usize;
        if length == 0 || length > super::protocol::MAX_FRAME {
            return Err(protocol_error());
        }
        let mut bytes = vec![0; length];
        self.read_exact(&mut bytes, cancel)?;
        let frame: Frame = serde_json::from_slice(&bytes).map_err(|_| protocol_error())?;
        if matches!(frame, Frame::Failed) {
            return Err(ArchiveError::Failed("Archive could not be decoded; it may be damaged, unsupported, or require a different password.".into()));
        }
        Ok(frame)
    }
    fn decision(&mut self, read: bool) -> Result<(), ArchiveError> {
        self.input.write_all(if read { b"R" } else { b"S" })?;
        Ok(())
    }
    fn finish(&mut self, cancel: &CancelToken) -> Result<(), ArchiveError> {
        loop {
            self.check(cancel)?;
            if let Some(status) = self.child.as_mut().ok_or_else(protocol_error)?.try_wait()? {
                if !status.success() {
                    return Err(protocol_error());
                }
                // A successfully terminated stream cannot carry trailing frames.
                let mut byte = [0];
                if self.output.read(&mut byte)? != 0 {
                    return Err(protocol_error());
                }
                let now =
                    crate::anchor::FileIdentity::from_stat(&rustix::fs::fstat(&self.archive)?);
                if !self.identity.matches(&now) {
                    return Err(ArchiveError::Failed(
                        "Archive changed while it was read; nothing was published.".into(),
                    ));
                }
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Reader {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            if child.try_wait().ok().flatten().is_some() {
                return;
            }
            let _ = child.kill();
            // A remote input may be in uninterruptible sleep. A detached
            // reaper owns the child; cancellation never waits for that syscall.
            let _ = std::thread::Builder::new()
                .name("gust-archive-reap".into())
                .spawn(move || {
                    let _ = child.wait();
                });
        }
    }
}

fn display_member(member: &Member) -> ArchiveEntry {
    ArchiveEntry {
        name: gust_core::location::escape_for_display(std::ffi::OsStr::from_bytes(&member.name))
            .trim_end_matches('/')
            .to_string(),
        mode: member.mode,
        size: member.size,
        kind: kind(member),
        link_target: member
            .link
            .as_ref()
            .map(|b| String::from_utf8_lossy(b).into_owned()),
    }
}
fn kind(member: &Member) -> ArchiveEntryKind {
    if member.hardlink {
        return ArchiveEntryKind::Hardlink;
    }
    match member.mode & 0o170000 {
        0o100000 => ArchiveEntryKind::Regular,
        0o040000 => ArchiveEntryKind::Directory,
        0o120000 => ArchiveEntryKind::Symlink,
        _ => ArchiveEntryKind::Special,
    }
}
fn member_path(bytes: &[u8]) -> Option<PathBuf> {
    if bytes.len() > super::protocol::MAX_PATH || bytes.contains(&0) {
        return None;
    }
    let path = Path::new(std::ffi::OsStr::from_bytes(bytes));
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(name) if name.len() <= 255 => out.push(name),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty() && out.components().count() < crate::recursive::MAX_DEPTH)
        .then_some(out)
}
fn link_stays_inside(directory: &Path, target: &[u8]) -> bool {
    if target.is_empty() || target.contains(&0) {
        return false;
    }
    let mut depth = directory.components().count();
    for component in Path::new(std::ffi::OsStr::from_bytes(target)).components() {
        match component {
            std::path::Component::Normal(_) => depth += 1,
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir if depth > 0 => depth -= 1,
            _ => return false,
        }
    }
    true
}
fn normalized_link(directory: &Path, bytes: &[u8]) -> Option<PathBuf> {
    let mut out = directory.to_path_buf();
    for part in Path::new(std::ffi::OsStr::from_bytes(bytes)).components() {
        match part {
            std::path::Component::Normal(name) => out.push(name),
            std::path::Component::ParentDir if out.pop() => {}
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    Some(out)
}
fn relative_link(from: &Path, target: &Path) -> PathBuf {
    let source: Vec<_> = from.components().collect();
    let target: Vec<_> = target.components().collect();
    let common = source
        .iter()
        .zip(&target)
        .take_while(|(a, b)| a == b)
        .count();
    let mut out = PathBuf::new();
    for _ in common..source.len() {
        out.push("..");
    }
    for part in &target[common..] {
        out.push(part.as_os_str());
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

fn check_header_limits(
    member: &Member,
    seen: usize,
    metadata: &mut usize,
    policy: &ExtractPolicy,
) -> Result<(), ArchiveError> {
    if seen >= policy.max_entries.min(super::protocol::MAX_HEADERS) {
        return Err(ArchiveError::Limit(
            "Archive has too many members; nothing was published.".into(),
        ));
    }
    *metadata = metadata
        .saturating_add(member.name.len())
        .saturating_add(member.link.as_ref().map_or(0, Vec::len))
        .saturating_add(128);
    if *metadata > super::protocol::MAX_METADATA
        || member.name.len() > super::protocol::MAX_PATH
        || member
            .link
            .as_ref()
            .is_some_and(|b| b.len() > super::protocol::MAX_PATH)
    {
        return Err(ArchiveError::Limit(
            "Archive member metadata exceeds the resource limit.".into(),
        ));
    }
    Ok(())
}

fn process_umask() -> u32 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix("Umask:")
                    .and_then(|value| u32::from_str_radix(value.trim(), 8).ok())
            })
        })
        .unwrap_or(0o077)
        & 0o777
}

/// Worker-side extraction. All archive data is decoded in a sandbox; the
/// parent creates only vetted names and checks actual bytes before each write.
pub fn extract_with(
    archive: &Path,
    dest: &Path,
    policy: &ExtractPolicy,
    password: Option<&ArchivePassword>,
    cancel: &CancelToken,
    progress: &mut dyn FnMut(super::jobs::ArchivePhase, usize, u64),
) -> Result<Extracted, ArchiveError> {
    use super::{jobs::ArchivePhase, stage};
    use std::collections::{BTreeMap, BTreeSet};
    let mask = process_umask();
    let mut reader = Reader::open(archive, password, cancel)?;
    let staging = stage::Stage::create(dest)?;
    let mut done = Extracted::default();
    let mut manifest = BTreeMap::new();
    let mut seen_paths = BTreeSet::new();
    let mut seen = 0;
    let mut metadata = 0;
    let mut declared = 0u64;
    let mut actual = 0u64;
    loop {
        let member = match reader.frame(cancel)? {
            Frame::EndArchive => {
                reader.finish(cancel)?;
                break;
            }
            Frame::Header(member) => member,
            _ => return Err(protocol_error()),
        };
        check_header_limits(&member, seen, &mut metadata, policy)?;
        seen += 1;
        progress(ArchivePhase::Reading, seen, actual);
        // TAR commonly includes its structural './' root. It does not name
        // an extractable child and is not an unsafe member refusal.
        if kind(&member) == ArchiveEntryKind::Directory
            && matches!(member.name.as_slice(), b"." | b"./")
        {
            reader.decision(false)?;
            if !matches!(reader.frame(cancel)?, Frame::EndMember) {
                return Err(protocol_error());
            }
            continue;
        }
        let relative = member_path(&member.name);
        let entry_kind = kind(&member);
        let reason = if relative.is_none() {
            Some("unsafe member path")
        } else if entry_kind == ArchiveEntryKind::Special {
            Some("special files are not extracted")
        } else if member.mode & 0o6000 != 0 {
            Some("privileged modes are not extracted")
        } else if matches!(
            entry_kind,
            ArchiveEntryKind::Symlink | ArchiveEntryKind::Hardlink
        ) && (!policy.allow_symlinks
            || !member.link.as_ref().is_some_and(|target| {
                if member.hardlink {
                    member_path(target).is_some()
                } else {
                    link_stays_inside(
                        relative.as_ref().unwrap().parent().unwrap_or(Path::new("")),
                        target,
                    )
                }
            }))
        {
            Some("link is disallowed or escapes the destination")
        } else {
            None
        };
        if let Some(reason) = reason {
            done.refused.push(format!(
                "{} ({reason})",
                String::from_utf8_lossy(&member.name)
            ));
            reader.decision(false)?;
            if !matches!(reader.frame(cancel)?, Frame::EndMember) {
                return Err(protocol_error());
            }
            continue;
        }
        let relative = relative.ok_or_else(protocol_error)?;
        if !seen_paths.insert(relative.clone()) {
            return Err(ArchiveError::Failed(
                "Archive contains duplicate normalized member paths; nothing was published.".into(),
            ));
        }
        if entry_kind == ArchiveEntryKind::Regular {
            declared = declared
                .checked_add(member.size)
                .ok_or_else(|| ArchiveError::Limit("Archive expansion size overflow.".into()))?;
            if declared > policy.max_total_bytes {
                return Err(ArchiveError::Limit(
                    "Archive exceeds the expanded-byte budget; nothing was published.".into(),
                ));
            }
        }
        let parent_path = relative.parent().unwrap_or(Path::new(""));
        let (parent, _) = stage::parents(&staging.dir, parent_path, true)?;
        let name = relative.file_name().ok_or_else(protocol_error)?;
        let mut file = match entry_kind {
            ArchiveEntryKind::Regular => Some(stage::create_file(&parent, name)?),
            ArchiveEntryKind::Directory => {
                stage::parents(&staging.dir, &relative, true)?;
                None
            }
            _ => None,
        };
        reader.decision(file.is_some())?;
        let mut member_bytes = 0u64;
        loop {
            match reader.frame(cancel)? {
                Frame::EndMember => break,
                Frame::Data(data)
                    if file.is_some()
                        && !data.is_empty()
                        && data.len() <= super::protocol::DATA_CHUNK =>
                {
                    member_bytes = member_bytes
                        .checked_add(data.len() as u64)
                        .ok_or_else(protocol_error)?;
                    actual = actual
                        .checked_add(data.len() as u64)
                        .ok_or_else(protocol_error)?;
                    if member_bytes > member.size || actual > policy.max_total_bytes {
                        return Err(ArchiveError::Limit(
                            "Archive emitted more bytes than authorized; nothing was published."
                                .into(),
                        ));
                    }
                    file.as_mut().unwrap().write_all(&data)?;
                    progress(ArchivePhase::Extracting, seen, actual);
                }
                _ => return Err(protocol_error()),
            }
        }
        if let Some(file) = file {
            if member_bytes != member.size {
                return Err(protocol_error());
            }
            // Keep staging private while writing, then restore ordinary
            // executable/rwx bits through the held fd before publication.
            rustix::fs::fchmod(
                &file,
                rustix::fs::Mode::from_bits_truncate(member.mode & 0o777 & !mask),
            )?;
            file.sync_all()?;
        }
        manifest.insert(relative, member);
    }
    // Links are created only after all file data, so archive link chains can
    // never redirect a staging write. Hardlinks address the archive root.
    let mut refused_links = BTreeSet::new();
    for (relative, member) in &manifest {
        if kind(member) == ArchiveEntryKind::Symlink {
            let target = normalized_link(
                relative.parent().unwrap_or(Path::new("")),
                member.link.as_ref().ok_or_else(protocol_error)?,
            )
            .ok_or_else(protocol_error)?;
            if !manifest.get(&target).is_some_and(|m| {
                matches!(
                    kind(m),
                    ArchiveEntryKind::Regular
                        | ArchiveEntryKind::Directory
                        | ArchiveEntryKind::Hardlink
                )
            }) {
                done.refused.push(format!(
                    "{} (link target is not an approved archive file or folder)",
                    relative.to_string_lossy()
                ));
                refused_links.insert(relative.clone());
                continue;
            }
        }
        let (parent, _) = stage::parents(
            &staging.dir,
            relative.parent().unwrap_or(Path::new("")),
            false,
        )?;
        let name = relative.file_name().ok_or_else(protocol_error)?;
        match kind(member) {
            ArchiveEntryKind::Symlink => {}
            ArchiveEntryKind::Hardlink => {
                let target = member_path(member.link.as_ref().ok_or_else(protocol_error)?)
                    .ok_or_else(protocol_error)?;
                // Do not link to an implicit or refused target, a symlink, or
                // another hardlink: dependency cycles cannot be guessed at.
                if !manifest
                    .get(&target)
                    .is_some_and(|m| kind(m) == ArchiveEntryKind::Regular)
                {
                    return Err(ArchiveError::Failed(
                        "Archive hardlink target is not an approved regular file.".into(),
                    ));
                }
                let (source, _) = stage::parents(
                    &staging.dir,
                    target.parent().unwrap_or(Path::new("")),
                    false,
                )?;
                let source_name = target.file_name().ok_or_else(protocol_error)?;
                let st = source.stat_child(source_name)?;
                if st.st_mode & 0o170000 != 0o100000 {
                    return Err(protocol_error());
                }
                rustix::fs::linkat(
                    source.as_fd(),
                    source_name,
                    parent.as_fd(),
                    name,
                    rustix::fs::AtFlags::empty(),
                )?;
            }
            _ => {}
        }
    }
    let held_destination = staging.parent.stat_self()?;
    let path_destination = stage::open_dir(dest)?.stat_self()?;
    if held_destination.st_dev != path_destination.st_dev
        || held_destination.st_ino != path_destination.st_ino
    {
        return Err(ArchiveError::Failed(
            "Archive destination changed; nothing was published.".into(),
        ));
    }
    progress(ArchivePhase::Publishing, seen, actual);
    let mut published = BTreeMap::new();
    let mut published_dirs = BTreeSet::new();
    let mut placed_names = BTreeMap::new();
    let mut publication = (|| -> Result<(), ArchiveError> {
        // Explicit and implicit folders are created through the same anchored
        // walk. Existing folders may be shared; existing symlinks are refused.
        for (relative, member) in &manifest {
            if cancel.is_cancelled() {
                return Err(ArchiveError::Cancelled);
            }
            let path = if kind(member) == ArchiveEntryKind::Directory {
                relative.as_path()
            } else {
                relative.parent().unwrap_or(Path::new(""))
            };
            stage::parents_with(&staging.parent, path, true, &mut |path, identity| {
                published.insert(path.to_path_buf(), identity);
                published_dirs.insert(path.to_path_buf());
                done.dirs += 1;
            })?;
        }
        // Publish real files before symlinks so every link follows the
        // archive's chosen keep-both name, never a pre-existing namesake.
        let ordered = manifest
            .iter()
            .filter(|(_, member)| kind(member) != ArchiveEntryKind::Symlink)
            .chain(
                manifest
                    .iter()
                    .filter(|(_, member)| kind(member) == ArchiveEntryKind::Symlink),
            );
        for (relative, member) in ordered {
            if kind(member) == ArchiveEntryKind::Directory || refused_links.contains(relative) {
                continue;
            }
            if cancel.is_cancelled() {
                return Err(ArchiveError::Cancelled);
            }
            let path = relative.parent().unwrap_or(Path::new(""));
            let (source, _) = stage::parents(&staging.dir, path, false)?;
            let (target, _) = stage::parents(&staging.parent, path, false)?;
            let name = relative.file_name().ok_or_else(protocol_error)?;
            if kind(member) == ArchiveEntryKind::Symlink {
                let link = normalized_link(path, member.link.as_ref().ok_or_else(protocol_error)?)
                    .ok_or_else(protocol_error)?;
                let final_target = placed_names.get(&link).unwrap_or(&link);
                let link = relative_link(path, final_target);
                rustix::fs::symlinkat(link.as_os_str(), source.as_fd(), name)?;
            }
            let expected = crate::anchor::FileIdentity::from_stat(&source.stat_child(name)?);
            let mut moved = false;
            for suffix in 0..10_000 {
                let chosen = if suffix == 0 {
                    name.to_os_string()
                } else {
                    let mut chosen = name.to_os_string();
                    chosen.push(format!(" ({})", suffix + 1));
                    chosen
                };
                match crate::anchor::rename_noreplace(&source, name, &target, &chosen) {
                    Ok(()) => {
                        let placed = path.join(&chosen);
                        placed_names.insert(relative.clone(), placed.clone());
                        published.insert(placed, expected);
                        done.files += 1;
                        if chosen != name {
                            done.renamed.push((
                                relative.to_string_lossy().into_owned(),
                                chosen.to_string_lossy().into_owned(),
                            ));
                        }
                        let now =
                            crate::anchor::FileIdentity::from_stat(&target.stat_child(&chosen)?);
                        if !expected.matches(&now) {
                            return Err(ArchiveError::Failed("Archive staging changed during publication; published work was preserved.".into()));
                        }
                        rustix::fs::fsync(target.as_fd())?;
                        moved = true;
                        progress(ArchivePhase::Publishing, seen, actual);
                        break;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(e.into()),
                }
            }
            if !moved {
                return Err(ArchiveError::Failed(
                    "No free archive publication name is available.".into(),
                ));
            }
        }
        Ok(())
    })();
    for (relative, mut identity) in published {
        if published_dirs.contains(&relative) {
            // Child publication changes a directory's mtime. Refresh that
            // metadata only for the very directory we exclusively created;
            // file receipts always retain their pre-publication identity.
            let current = stage::parents(
                &staging.parent,
                relative.parent().unwrap_or(Path::new("")),
                false,
            )
            .and_then(|(parent, _)| {
                parent.stat_child(
                    relative
                        .file_name()
                        .ok_or_else(|| std::io::Error::other("invalid published path"))?,
                )
            });
            match current {
                Ok(stat) if stat.st_dev == identity.dev && stat.st_ino == identity.ino => {
                    identity = crate::anchor::FileIdentity::from_stat(&stat);
                }
                _ => {
                    publication = Err(ArchiveError::Failed(
                        "A published archive folder changed; receipt retained for inspection."
                            .into(),
                    ));
                }
            }
        }
        done.published.push((dest.join(relative), identity.into()));
    }
    if let Err(cause) = publication {
        return if done.published.is_empty() {
            Err(cause)
        } else {
            Err(ArchiveError::Partial {
                receipt: done,
                cause: Box::new(cause),
            })
        };
    }
    if let Err(error) = staging.cleanup() {
        return Err(ArchiveError::Partial {
            receipt: done,
            cause: Box::new(error.into()),
        });
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ordinary_line_is_parsed() {
        let e = parse_listing_line("-rw-r--r--  0 user   user        2 Sep 13 19:48 a.txt")
            .expect("should parse");
        assert_eq!(e.name, "a.txt");
        assert_eq!(e.size, 2);
        assert_eq!(e.kind, ArchiveEntryKind::Regular);
    }

    #[test]
    fn a_directory_is_recognised_and_its_slash_dropped() {
        let e = parse_listing_line("drwxr-xr-x  0 user   user        0 Sep 13 19:48 sub/").unwrap();
        assert_eq!(e.kind, ArchiveEntryKind::Directory);
        assert_eq!(e.name, "sub");
    }

    #[test]
    fn a_symlink_keeps_its_target() {
        let e = parse_listing_line(
            "lrwxrwxrwx  0 user   user        0 Sep 13 19:48 link -> ../outside",
        )
        .unwrap();
        assert_eq!(e.kind, ArchiveEntryKind::Symlink);
        assert_eq!(e.name, "link");
        assert_eq!(e.link_target.as_deref(), Some("../outside"));
    }

    #[test]
    fn a_fifo_is_recognised_as_special() {
        let e = parse_listing_line("prw-r--r--  0 user   user        0 Sep 13 19:48 pipe").unwrap();
        assert_eq!(e.kind, ArchiveEntryKind::Special);
    }

    #[test]
    fn a_setuid_entry_carries_its_privileged_bit() {
        let e = parse_listing_line("-rwsr-xr-x  0 root   root     1234 Sep 13 19:48 evil").unwrap();
        assert_ne!(e.mode & 0o4000, 0, "setuid must survive parsing");
    }

    #[test]
    fn a_name_with_spaces_survives() {
        let e = parse_listing_line(
            "-rw-r--r--  0 user   user       10 Sep 13 19:48 my holiday photos.txt",
        )
        .unwrap();
        assert_eq!(e.name, "my holiday photos.txt");
    }

    #[test]
    fn a_year_dated_entry_parses_too() {
        let e =
            parse_listing_line("-rw-r--r--  0 user   user       10 Jan  1  2020 old.txt").unwrap();
        assert_eq!(e.name, "old.txt");
    }

    #[test]
    fn junk_is_skipped_not_guessed_at() {
        assert!(parse_listing_line("").is_none());
        assert!(parse_listing_line("   ").is_none());
        assert!(parse_listing_line("not a listing line").is_none());
    }

    #[test]
    fn archive_names_are_recognised_by_ending_only() {
        assert!(looks_like_archive("backup.tar.zst"));
        assert!(looks_like_archive("photos.ZIP"));
        assert!(!looks_like_archive("notes.txt"));
        // A name is a hint, never proof — nothing is run to find out.
        assert!(!looks_like_archive("tar"));
    }

    #[test]
    fn no_name_can_become_an_option() {
        let args = compress_args(
            Path::new("/tmp"),
            Path::new("/tmp/out.tar.zst"),
            &["--checkpoint-action=exec=sh".to_string()],
        );
        let dashdash = args.iter().position(|a| a == "--").expect("-- present");
        let name_at = args
            .iter()
            .position(|a| a == "--checkpoint-action=exec=sh")
            .unwrap();
        assert!(
            name_at > dashdash,
            "every name must come after --, or it is an option"
        );
    }

    #[test]
    fn extraction_does_not_preserve_owner_or_permissions() {
        let args = extract_args(Path::new("/tmp/a.tar"), Path::new("/tmp/stage"));
        assert!(args.iter().any(|a| a == "--no-same-owner"));
        assert!(args.iter().any(|a| a == "--no-same-permissions"));
        // -P would let absolute paths through; it must never be passed.
        assert!(!args.iter().any(|a| a == "-P"));
    }

    #[test]
    fn a_summary_says_what_was_refused() {
        let e = Extracted {
            files: 3,
            dirs: 1,
            refused: vec!["evil (asks for extra privilege)".into()],
            ..Default::default()
        };
        let s = e.summary();
        assert!(s.contains("3 files"));
        assert!(s.contains("1 folder"));
        assert!(s.contains("refused 1 unsafe entry"));
    }

    #[test]
    fn an_empty_extraction_says_so() {
        assert_eq!(Extracted::default().summary(), "Nothing was extracted");
    }

    #[test]
    fn a_second_archive_does_not_replace_the_first() {
        let d = std::env::temp_dir().join("gust-arch-unique");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();

        // Nothing there yet: the wanted name is free.
        assert_eq!(unique_name(&d, "photos.tar.zst"), d.join("photos.tar.zst"));

        std::fs::write(d.join("photos.tar.zst"), "x").unwrap();
        // The compound extension must survive intact.
        assert_eq!(
            unique_name(&d, "photos.tar.zst"),
            d.join("photos-2.tar.zst")
        );

        std::fs::write(d.join("photos-2.tar.zst"), "x").unwrap();
        assert_eq!(
            unique_name(&d, "photos.tar.zst"),
            d.join("photos-3.tar.zst")
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn errors_are_actionable() {
        assert!(ArchiveError::NoTool.message().contains("bsdtar"));
        assert!(ArchiveError::Unsupported("x.bin".into())
            .message()
            .contains("x.bin"));
    }
}
