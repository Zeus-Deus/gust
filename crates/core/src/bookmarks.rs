//! Bookmarks in the GTK format (`~/.config/gtk-3.0/bookmarks`).
//!
//! One entry per line: a URI, then optionally a space and a label. The file
//! is shared with Nautilus, Thunar and every GTK file dialog, so Gust keeps
//! every line it does not understand (sftp://, smb://, …) exactly as it was,
//! and writes atomically so a crash cannot leave the desktop's bookmarks
//! half-written.

use crate::clipboard::{path_to_uri, uri_to_path};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    /// Local folder, for `file://` entries.
    pub path: PathBuf,
    pub label: Option<String>,
    /// The original line, for entries Gust cannot open (kept verbatim).
    pub uri: Option<String>,
}

impl Bookmark {
    /// What the sidebar shows.
    pub fn display(&self) -> String {
        if let Some(l) = &self.label {
            return l.clone();
        }
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.to_string_lossy().into_owned())
    }
}

/// The default location, following `XDG_CONFIG_HOME`.
pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("gtk-3.0/bookmarks"))
}

pub fn parse(text: &str) -> Vec<Bookmark> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let (uri, label) = match line.split_once(' ') {
                Some((u, l)) if !l.is_empty() => (u, Some(l.to_string())),
                _ => (line, None),
            };
            match uri_to_path(uri) {
                Some(path) => Bookmark {
                    path,
                    label,
                    uri: None,
                },
                None => Bookmark {
                    path: PathBuf::new(),
                    label: None,
                    uri: Some(line.to_string()),
                },
            }
        })
        .collect()
}

pub fn render(list: &[Bookmark]) -> String {
    let mut out = String::new();
    for b in list {
        match &b.uri {
            Some(raw) => out.push_str(raw),
            None => {
                out.push_str(&path_to_uri(&b.path));
                if let Some(l) = &b.label {
                    out.push(' ');
                    out.push_str(l);
                }
            }
        }
        out.push('\n');
    }
    out
}

pub fn add(list: &mut Vec<Bookmark>, path: PathBuf) {
    if list.iter().any(|b| b.uri.is_none() && b.path == path) {
        return;
    }
    list.push(Bookmark {
        path,
        label: None,
        uri: None,
    });
}

pub fn remove(list: &mut Vec<Bookmark>, path: &Path) {
    list.retain(|b| !(b.uri.is_none() && b.path == path));
}

pub fn load(file: &Path) -> Vec<Bookmark> {
    std::fs::read_to_string(file)
        .map(|t| parse(&t))
        .unwrap_or_default()
}

/// Write via a temporary file in the same folder and rename it into place.
pub fn save(file: &Path, list: &[Bookmark]) -> std::io::Result<()> {
    let dir = file
        .parent()
        .ok_or_else(|| std::io::Error::other("no parent folder"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".bookmarks.gust-{}", std::process::id()));
    std::fs::write(&tmp, render(list))?;
    std::fs::rename(&tmp, file).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}
