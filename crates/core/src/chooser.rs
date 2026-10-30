//! The file-chooser portal (`org.freedesktop.impl.portal.FileChooser`):
//! when a browser, chat app or editor shows an Open or Save dialog, Gust
//! answers it. This module is the pure part; the D-Bus service is in the
//! app crate (`gust --portal`).
//!
//! The service starts a Gust window in chooser mode and waits for it. The
//! window writes what the user picked to a private result file (NUL
//! separated, so any name survives), or nothing if they cancelled.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Open,
    OpenMany,
    OpenFolder,
    Save,
    SaveMany,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Open => "open",
            Mode::OpenMany => "open-many",
            Mode::OpenFolder => "open-folder",
            Mode::Save => "save",
            Mode::SaveMany => "save-many",
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
    /// The app's words for the primary button, mnemonics removed.
    pub accept: Option<String>,
    /// Names to place in the chosen folder (`SaveFiles`), as raw bytes: two
    /// names must never become one through a lossy conversion.
    pub files: Vec<OsString>,
    pub filters: Filters,
}

/// One entry of an Open dialog's file type list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub name: String,
    pub globs: Vec<String>,
}

/// What the portal sends: `(name, [(0 glob | 1 MIME, pattern)])`.
pub type RawFilter = (String, Vec<(u32, String)>);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    pub items: Vec<Filter>,
    pub current: usize,
}

// An app's request is untrusted, and every glob runs against every file on the
// UI thread. These bound that work, and keep the JSON under Linux's limit for
// one environment string (MAX_ARG_STRLEN, 128 KiB).
const MAX_FILTERS: usize = 64;
const MAX_RULES: usize = 256;
const MAX_GLOB_LEN: usize = 128;
const MAX_FILTER_BYTES: usize = 8 * 1024;
const MAX_FILTERS_JSON: usize = 120 * 1024;
const MAX_FILTER_NAME: usize = 256;
/// The most any Linux filesystem takes for one name.
const MAX_NAME_BYTES: usize = 255;

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
            ("save-many", _, _) => Mode::SaveMany,
            (_, true, _) => Mode::OpenFolder,
            (_, false, true) => Mode::OpenMany,
            _ => Mode::Open,
        };
        // `current_folder` is a NUL-terminated byte string.
        let folder = current_folder
            .map(nul_terminated_path)
            .filter(|p| p.is_absolute());
        let name = current_name.and_then(|n| bare_name(&n));
        Request {
            mode,
            folder,
            name,
            accept: None,
            files: Vec::new(),
            filters: Filters::default(),
        }
    }

    /// `current_file`: the file being re-saved. Its folder and name win over
    /// `current_folder` and `current_name`, as the portal documents. The name
    /// field holds text, so a name that is not UTF-8 is left for the user to
    /// type rather than offered changed.
    pub fn with_current_file(mut self, file: Option<Vec<u8>>) -> Request {
        let Some(path) = file.map(nul_terminated_path).filter(|p| p.is_absolute()) else {
            return self;
        };
        if let (Some(dir), Some(name)) = (path.parent(), path.file_name()) {
            self.folder = Some(dir.to_path_buf());
            self.name = name.to_str().and_then(bare_name);
        }
        self
    }

    /// `SaveFiles`: the answer must match the names one to one, so a name that
    /// cannot be used, or two that land on one target, refuse the whole call.
    pub fn with_files(mut self, files: Vec<Vec<u8>>) -> Option<Request> {
        let mut names = Vec::with_capacity(files.len());
        for file in files {
            let path = nul_terminated_path(file);
            let name = path.file_name()?.to_os_string();
            let bytes = name.as_bytes();
            if bytes.len() > MAX_NAME_BYTES || bytes.contains(&0) || names.contains(&name) {
                return None;
            }
            names.push(name);
        }
        if names.is_empty() {
            return None;
        }
        self.files = names;
        Some(self)
    }
}

/// The names for the window. `/` cannot be in a name and the environment holds
/// bytes, so this keeps every name exactly.
pub fn files_to_env(files: &[OsString]) -> OsString {
    let mut out = OsString::new();
    for (i, file) in files.iter().enumerate() {
        if i > 0 {
            out.push("/");
        }
        out.push(file);
    }
    out
}

pub fn files_from_env(value: &OsStr) -> Vec<OsString> {
    value
        .as_bytes()
        .split(|b| *b == b'/')
        .filter(|name| !name.is_empty())
        .map(|name| OsStr::from_bytes(name).to_os_string())
        .collect()
}

fn nul_terminated_path(mut bytes: Vec<u8>) -> PathBuf {
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    PathBuf::from(OsString::from_vec(bytes))
}

fn bare_name(name: &str) -> Option<String> {
    Path::new(name)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty() && n != "." && n != "..")
}

/// A GTK mnemonic label as plain text: `_Open` is Open, `__` is one underscore.
pub fn plain_label(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut chars = label.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '_' {
            if chars.peek() == Some(&'_') {
                chars.next();
                out.push('_');
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// True when `name` matches a shell-style glob (`*`, `?`, `[a-z]`, `[!a]`).
/// ASCII case is ignored: apps send `*.png` and users have `PHOTO.PNG`.
pub fn glob_match(glob: &[u8], name: &[u8]) -> bool {
    // Iterative with one backtrack point, so a hostile glob stays linear-ish
    // instead of recursing once per `*`.
    let (mut g, mut n) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        let step = match glob.get(g) {
            Some(b'*') => {
                star = Some((g + 1, n));
                g += 1;
                continue;
            }
            Some(b'?') => Some(g + 1),
            Some(b'[') => class_match(glob, g, name[n]),
            Some(c) if c.eq_ignore_ascii_case(&name[n]) => Some(g + 1),
            _ => None,
        };
        if let Some(next) = step {
            g = next;
            n += 1;
        } else if let Some((after, from)) = star {
            g = after;
            n = from + 1;
            star = Some((after, n));
        } else {
            return false;
        }
    }
    glob[g..].iter().all(|c| *c == b'*')
}

/// Match `byte` against the class starting at `glob[at]`. Returns the index
/// after the class, or None when it does not match. An unterminated `[` is a
/// literal, as in a shell.
fn class_match(glob: &[u8], at: usize, byte: u8) -> Option<usize> {
    let Some(close) = glob
        .get(at + 2..)
        .and_then(|rest| rest.iter().position(|c| *c == b']'))
        .map(|p| at + 2 + p)
    else {
        return byte.eq_ignore_ascii_case(&b'[').then_some(at + 1);
    };
    let mut body = &glob[at + 1..close];
    let negated = matches!(body.first(), Some(b'!' | b'^'));
    if negated {
        body = &body[1..];
    }
    let low = byte.to_ascii_lowercase();
    let mut hit = false;
    let mut i = 0;
    while i < body.len() {
        let start = body[i].to_ascii_lowercase();
        if body.get(i + 1) == Some(&b'-') && i + 2 < body.len() {
            hit |= (start..=body[i + 2].to_ascii_lowercase()).contains(&low);
            i += 3;
        } else {
            hit |= start == low;
            i += 1;
        }
    }
    (hit != negated).then_some(close + 1)
}

fn mime_matches(pattern: &str, mime: &str) -> bool {
    match pattern.split_once('/') {
        Some((kind, "*")) => mime
            .split_once('/')
            .is_some_and(|(k, _)| k.eq_ignore_ascii_case(kind)),
        _ => pattern.eq_ignore_ascii_case(mime),
    }
}

/// The globs `shared-mime-info` lists for a MIME pattern. `globs2` lines are
/// `weight:mime:glob[:flags]`.
fn globs_for_mime<'a>(pattern: &'a str, globs2: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    globs2
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(move |l| {
            let mut parts = l.splitn(4, ':');
            let (_, mime, glob) = (parts.next()?, parts.next()?, parts.next()?);
            mime_matches(pattern, mime).then_some(glob)
        })
}

fn expand_filter((name, rules): RawFilter, globs2: &str) -> Filter {
    let mut globs: Vec<String> = Vec::new();
    let mut used = 0;
    // False once the budget is spent, so a broad MIME type stops scanning.
    let mut take = |glob: &str| {
        if glob.len() > MAX_GLOB_LEN {
            return true;
        }
        used += glob.len();
        if used > MAX_FILTER_BYTES {
            return false;
        }
        globs.push(glob.to_string());
        true
    };
    for (kind, pattern) in rules.into_iter().take(MAX_RULES) {
        let more = match kind {
            0 => take(&pattern),
            1 if pattern == "*/*" || pattern == "*" => take("*"),
            1 => globs_for_mime(&pattern, globs2).all(&mut take),
            _ => true,
        };
        if !more {
            break;
        }
    }
    Filter {
        name: name.chars().take(MAX_FILTER_NAME).collect(),
        globs,
    }
}

impl Filters {
    /// The app's list as globs only. `current` is the app's preferred filter;
    /// one that is not in the list is added, as GTK does. Names are labels and
    /// may repeat, so the whole filter is compared first.
    pub fn expand(raw: Vec<RawFilter>, current: Option<RawFilter>, globs2: &str) -> Filters {
        let mut items: Vec<Filter> = raw
            .into_iter()
            .take(MAX_FILTERS)
            .map(|f| expand_filter(f, globs2))
            .collect();
        let mut index = 0;
        if let Some(wanted) = current.map(|f| expand_filter(f, globs2)) {
            let found = items
                .iter()
                .position(|f| *f == wanted)
                .or_else(|| items.iter().position(|f| f.name == wanted.name));
            index = found.unwrap_or_else(|| {
                items.truncate(MAX_FILTERS - 1);
                items.push(wanted);
                items.len() - 1
            });
        }
        let mut filters = Filters {
            items,
            current: index,
        };
        // Better a shorter list than a dialog that cannot start. The filter
        // the app prefers is the last to go.
        while filters.items.len() > 1 && filters.to_json().len() > MAX_FILTERS_JSON {
            let last = filters.items.len() - 1;
            let drop = if filters.current == last {
                last - 1
            } else {
                last
            };
            filters.items.remove(drop);
            if drop < filters.current {
                filters.current -= 1;
            }
        }
        filters
    }

    pub fn to_json(&self) -> String {
        let filters: Vec<_> = self
            .items
            .iter()
            .map(|f| serde_json::json!({ "name": f.name, "globs": f.globs }))
            .collect();
        serde_json::json!({ "current": self.current, "filters": filters }).to_string()
    }

    pub fn from_json(text: &str) -> Filters {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
            return Filters::default();
        };
        let items: Vec<Filter> = v["filters"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|f| Filter {
                        name: f["name"].as_str().unwrap_or("").to_string(),
                        globs: f["globs"]
                            .as_array()
                            .map(|g| {
                                g.iter()
                                    .filter_map(|x| x.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let current = v["current"].as_u64().unwrap_or(0) as usize;
        Filters {
            current: current.min(items.len().saturating_sub(1)),
            items,
        }
    }
}

/// Whether a filter shows `name`. An empty glob list shows everything: an
/// app's malformed filter must not make every file unreachable.
pub fn filter_shows(globs: &[String], name: &[u8]) -> bool {
    globs.is_empty() || globs.iter().any(|g| glob_match(g.as_bytes(), name))
}

/// `open-folder`: the one selected folder, otherwise the folder on screen.
/// The cursor is deliberately not an input (DESIGN §6.27).
pub fn folder_answer(selected_folders: &[PathBuf], here: Option<PathBuf>) -> Option<PathBuf> {
    match selected_folders {
        [one] => Some(one.clone()),
        _ => here,
    }
}

/// What saving to these paths would run into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveCheck {
    /// Nothing is in the way.
    Clear,
    /// These names already exist and would be replaced.
    Replaces(Vec<String>),
    /// The one name typed is a folder.
    Folder(PathBuf),
    /// Cannot tell, so it must not be answered.
    Failed(String),
}

/// Stat the targets. This can block on a slow mount: run it on a worker, and
/// have `wanted` turn false when nobody is waiting for the answer any more.
pub fn check_targets(targets: &[PathBuf], wanted: impl Fn() -> bool) -> SaveCheck {
    let mut existing = Vec::new();
    for target in targets {
        if !wanted() {
            return SaveCheck::Failed("no longer needed".to_string());
        }
        let meta = match std::fs::symlink_metadata(target) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return SaveCheck::Failed(e.to_string()),
        };
        // Follow a link only to learn whether it is a folder; a dangling link
        // still occupies the name.
        if meta.is_dir() || std::fs::metadata(target).is_ok_and(|m| m.is_dir()) {
            return SaveCheck::Folder(target.clone());
        }
        existing.push(
            target
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
    }
    if existing.is_empty() {
        SaveCheck::Clear
    } else {
        SaveCheck::Replaces(existing)
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
