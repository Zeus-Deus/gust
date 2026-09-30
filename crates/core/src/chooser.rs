//! The file-chooser portal (`org.freedesktop.impl.portal.FileChooser`):
//! when a browser, chat app or editor shows an Open or Save dialog, Gust
//! answers it. This module is the pure part; the D-Bus service is in the
//! app crate (`gust --portal`).
//!
//! The service starts a Gust window in chooser mode and waits for it. The
//! window writes what the user picked to a private result file (NUL
//! separated, so any name survives), or nothing if they cancelled.

use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Open,
    OpenMany,
    OpenFolder,
    Save,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Open => "open",
            Mode::OpenMany => "open-many",
            Mode::OpenFolder => "open-folder",
            Mode::Save => "save",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub mode: Mode,
    /// Where to start, if the app asked and it is an absolute path.
    pub folder: Option<PathBuf>,
    /// Suggested file name for Save — a name only, never a path.
    pub name: Option<String>,
}

impl Request {
    /// `kind` is "open" or "save"; the rest are the portal's options.
    pub fn new(
        kind: &str,
        directory: bool,
        multiple: bool,
        current_folder: Option<Vec<u8>>,
        current_name: Option<String>,
    ) -> Request {
        let mode = match (kind, directory, multiple) {
            ("save", _, _) => Mode::Save,
            (_, true, _) => Mode::OpenFolder,
            (_, false, true) => Mode::OpenMany,
            _ => Mode::Open,
        };
        // `current_folder` is a NUL-terminated byte string.
        let folder = current_folder.and_then(|mut b| {
            while b.last() == Some(&0) {
                b.pop();
            }
            let p = PathBuf::from(std::ffi::OsString::from_vec(b));
            p.is_absolute().then_some(p)
        });
        let name = current_name
            .and_then(|n| {
                Path::new(&n)
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
            })
            .filter(|n| !n.is_empty() && n != "." && n != "..");
        Request { mode, folder, name }
    }
}

/// Write what was picked. Private (0600) because paths are personal.
pub fn write_result(out: &Path, picked: &[PathBuf]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(out)?;
    for p in picked {
        f.write_all(p.as_os_str().as_bytes())?;
        f.write_all(b"\0")?;
    }
    f.sync_all()
}

/// Read what was picked; empty means cancelled. Only absolute paths count.
pub fn parse_result(out: &Path) -> Vec<PathBuf> {
    let Ok(bytes) = std::fs::read(out) else {
        return Vec::new();
    };
    bytes
        .split(|b| *b == 0)
        .filter(|s| s.first() == Some(&b'/'))
        .map(|s| PathBuf::from(std::ffi::OsStr::from_bytes(s)))
        .collect()
}

/// The portal's `uris` answer.
pub fn answer_uris(picked: &[PathBuf]) -> Vec<String> {
    picked
        .iter()
        .map(|p| crate::clipboard::path_to_uri(p))
        .collect()
}
