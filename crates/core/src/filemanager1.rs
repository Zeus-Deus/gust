//! `org.freedesktop.FileManager1` — "Show in folder" from other apps.
//!
//! This module is the pure part: URIs in, windows to open out. The D-Bus
//! listener lives in the app crate (`gust --dbus-service`).

use crate::clipboard::uri_to_path;
use std::path::{Component, Path, PathBuf};

/// One window to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Open {
    pub folder: PathBuf,
    /// Name of the entry to put the cursor on, for `ShowItems`.
    pub select: Option<PathBuf>,
}

/// At most this many windows per request: a caller passing hundreds of
/// URIs must not be able to flood the desktop.
pub const MAX_WINDOWS: usize = 8;

/// Lexical normalisation: resolves `.` and `..` without touching the disk,
/// so the path shown is the path the URI actually named.
fn normalise(p: &Path) -> PathBuf {
    let mut out = PathBuf::from("/");
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(n) => out.push(n),
            Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
        }
    }
    out
}

/// `items`: true for ShowItems (open the parent, select the item), false
/// for ShowFolders (open the folder itself). Non-`file://` URIs are ignored:
/// Gust never fetches anything on a caller's behalf.
pub fn plan(uris: &[String], items: bool) -> Vec<Open> {
    let mut out: Vec<Open> = Vec::new();
    for u in uris {
        let Some(path) = uri_to_path(u) else {
            continue;
        };
        let path = normalise(&path);
        let open = if items {
            match (path.parent(), path.file_name()) {
                (Some(parent), Some(name)) => Open {
                    folder: parent.to_path_buf(),
                    select: Some(PathBuf::from(name)),
                },
                _ => Open { folder: path, select: None },
            }
        } else {
            Open { folder: path, select: None }
        };
        // Several items in one folder: one window (first item selected).
        if out.iter().any(|o| o.folder == open.folder) {
            continue;
        }
        out.push(open);
        if out.len() == MAX_WINDOWS {
            break;
        }
    }
    out
}

const SERVICE_FILE: &str = "org.freedesktop.FileManager1.service";
/// Marks a file as Gust's, so Gust never removes one it did not write.
const OURS: &str = "# Written by Gust (Settings > Default app). Removed by choosing Previous app.";

/// The per-user D-Bus activation folder, which the bus prefers over the
/// system one — so Gust can answer "Show in folder" without replacing any
/// installed file manager's package files.
pub fn user_service_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("dbus-1/services"))
}

pub fn service_text(exe: &Path) -> String {
    let e = exe.to_string_lossy();
    let exec = if e.contains(' ') { format!("\"{e}\"") } else { e.into_owned() };
    format!("{OURS}\n[D-BUS Service]\nName=org.freedesktop.FileManager1\nExec={exec} --dbus-service\n")
}

fn is_ours(file: &Path) -> bool {
    std::fs::read_to_string(file).is_ok_and(|t| t.starts_with(OURS))
}

/// Write the activation file. Refuses to replace one another app wrote.
pub fn claim_service(dir: &Path, exe: &Path) -> std::io::Result<()> {
    let f = dir.join(SERVICE_FILE);
    if f.exists() && !is_ours(&f) {
        return Err(std::io::Error::other(format!(
            "{} belongs to another app; left as it is",
            f.display()
        )));
    }
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{SERVICE_FILE}.gust-{}", std::process::id()));
    std::fs::write(&tmp, service_text(exe))?;
    std::fs::rename(&tmp, &f)
}

/// Remove the activation file, only if Gust wrote it.
pub fn release_service(dir: &Path) -> std::io::Result<()> {
    let f = dir.join(SERVICE_FILE);
    if is_ours(&f) {
        std::fs::remove_file(f)?;
    }
    Ok(())
}
