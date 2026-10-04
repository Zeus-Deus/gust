//! How each entry is drawn: a folder or a page, its file family, the label
//! printed on it, and its emblems (DESIGN §6.21).
//!
//! Gust draws its own icons in the theme's colours (DESIGN §6.21, D51): a
//! folder shape for folders, a page with the extension printed on it for
//! files, coloured by family (code, data, text, documents…). Nothing here
//! returns an image.
//!
//! Two halves, so the GUI thread never touches the filesystem:
//!
//! - [`key_for`] and [`emblems_for`] are pure. They look only at an entry's
//!   name, kind and already-read metadata, so the model can call them from
//!   `data()`.
//! - [`Looks::load_current`] reads the shared MIME database and the XDG user
//!   folders on a worker; [`Looks::look`] is then a lookup in memory.

use crate::entry::{EntryKind, Metadata};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

// --- pure half ---------------------------------------------------------------

/// What an entry looks like, as a string [`Looks::look`] can resolve.
///
/// - `d:<path>` a folder (the path picks Documents, Downloads… icons)
/// - `f<flags>:<name>` a file, flags `x` (executable) and `z` (empty)
/// - `broken` a link whose target is gone
/// - `special` a FIFO, socket or device node
///
/// A symlink to a folder looks like a folder and a symlink to a file like
/// that file: the link itself is shown as an emblem, not as the icon.
pub fn key_for(name: &OsStr, kind: EntryKind, meta: Option<&Metadata>, full: &Path) -> String {
    let file_key = |meta: Option<&Metadata>| {
        let mut flags = String::new();
        if let Some(m) = meta {
            if kind != EntryKind::Symlink && m.mode & 0o111 != 0 {
                flags.push('x');
            }
            if kind != EntryKind::Symlink && m.size == 0 {
                flags.push('z');
            }
        }
        format!("f{flags}:{}", name.to_string_lossy().to_lowercase())
    };
    match kind {
        EntryKind::Dir => format!("d:{}", full.to_string_lossy()),
        EntryKind::Symlink => match meta {
            Some(m) if m.link_ok == Some(false) => "broken".into(),
            Some(m) if m.link_dir => format!("d:{}", full.to_string_lossy()),
            _ => file_key(None),
        },
        EntryKind::Special => "special".into(),
        EntryKind::File | EntryKind::Unknown => file_key(meta),
    }
}

/// Emblems for an entry, most important first: `unreadable` or `readonly`,
/// `mount`, `link` or `broken-link`.
///
/// Read-only and unreadable come from the mode bits against this process's
/// user and groups — no `access()` call per entry, so a 100k listing costs
/// nothing extra. A link's own mode is always 0777 and says nothing, so links
/// get only the link emblem.
pub fn emblems_for(kind: EntryKind, meta: Option<&Metadata>) -> Vec<&'static str> {
    let mut out = Vec::new();
    let Some(m) = meta else {
        return out;
    };
    if kind != EntryKind::Symlink {
        let who = Access::current();
        let dir = kind == EntryKind::Dir;
        if !who.can_read(m, dir) {
            out.push("unreadable");
        } else if !who.can_write(m) {
            out.push("readonly");
        }
    }
    if m.mount_root {
        out.push("mount");
    }
    if kind == EntryKind::Symlink {
        out.push(if m.link_ok == Some(false) {
            "broken-link"
        } else {
            "link"
        });
    }
    out
}

/// This process's identity, for reading permission bits.
#[derive(Debug, Clone)]
pub struct Access {
    uid: u32,
    gids: Vec<u32>,
}

impl Access {
    pub fn new(uid: u32, gids: Vec<u32>) -> Self {
        Access { uid, gids }
    }

    /// The real process identity, read once.
    pub fn current() -> &'static Access {
        static ME: OnceLock<Access> = OnceLock::new();
        ME.get_or_init(|| {
            let uid = rustix::process::geteuid().as_raw();
            let mut gids: Vec<u32> = rustix::process::getgroups()
                .map(|g| g.into_iter().map(|g| g.as_raw()).collect())
                .unwrap_or_default();
            gids.push(rustix::process::getegid().as_raw());
            Access { uid, gids }
        })
    }

    /// The three permission bits (rwx) that apply to us.
    fn bits(&self, m: &Metadata) -> u32 {
        if m.uid == self.uid {
            (m.mode >> 6) & 7
        } else if self.gids.contains(&m.gid) {
            (m.mode >> 3) & 7
        } else {
            m.mode & 7
        }
    }

    pub fn can_write(&self, m: &Metadata) -> bool {
        self.uid == 0 || self.bits(m) & 2 != 0
    }

    /// A folder needs read AND search (x) to be opened.
    pub fn can_read(&self, m: &Metadata, dir: bool) -> bool {
        if self.uid == 0 {
            return true;
        }
        let b = self.bits(m);
        b & 4 != 0 && (!dir || b & 1 != 0)
    }
}

// --- the shared MIME database ------------------------------------------------

/// Name → MIME type → generic kind, from freedesktop's shared-mime-info.
#[derive(Debug, Default)]
pub struct MimeDb {
    /// `tar.gz` → (weight, mime), from `*.tar.gz` globs.
    ext: HashMap<String, (u32, String)>,
    /// `makefile` → (weight, mime), from globs with no wildcard.
    literal: HashMap<String, (u32, String)>,
    /// mime → generic icon (`generic-icons`), i.e. its broad kind.
    generic: HashMap<String, String>,
}

impl MimeDb {
    /// Read every `mime` directory given, highest precedence first: a type
    /// the user's own database defines wins over the system's.
    pub fn load(dirs: &[PathBuf]) -> Self {
        let mut db = MimeDb::default();
        for d in dirs {
            if let Ok(t) = std::fs::read_to_string(d.join("globs2")) {
                db.add_globs2(&t);
            }
            if let Ok(t) = std::fs::read_to_string(d.join("generic-icons")) {
                add_pairs(&mut db.generic, &t);
            }
        }
        db
    }

    /// `weight:mime:glob[:flags]` lines. Only plain suffix globs (`*.json`)
    /// and literal names (`Makefile`) are used: they cover what a file
    /// manager meets, and anything fancier is not worth a matcher.
    pub fn add_globs2(&mut self, text: &str) {
        for line in text.lines() {
            if line.starts_with('#') {
                continue;
            }
            let mut parts = line.splitn(4, ':');
            let (Some(w), Some(mime), Some(glob)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let Ok(weight) = w.parse::<u32>() else {
                continue;
            };
            if glob == "__NOGLOBS__" || mime.is_empty() {
                continue;
            }
            let glob = glob.to_lowercase();
            let wild = |s: &str| s.contains(['*', '?', '[']);
            let (map, k) = if let Some(rest) = glob.strip_prefix("*.") {
                if wild(rest) {
                    continue;
                }
                (&mut self.ext, rest.to_string())
            } else if !wild(&glob) {
                (&mut self.literal, glob)
            } else {
                continue;
            };
            // Highest weight wins; on a tie the first one seen (the higher
            // precedence directory, or the earlier line) stays.
            match map.get(&k) {
                Some((have, _)) if *have >= weight => {}
                _ => {
                    map.insert(k, (weight, mime.to_string()));
                }
            }
        }
    }

    /// The MIME type for a lowercase file name, if any glob knows it. The
    /// longest suffix wins, so `a.tar.gz` is a compressed tar, not a gzip.
    pub fn mime_for_name(&self, lower: &str) -> Option<&str> {
        if let Some((_, m)) = self.literal.get(lower) {
            return Some(m);
        }
        for (i, c) in lower.char_indices() {
            if c != '.' {
                continue;
            }
            if let Some((_, m)) = self.ext.get(&lower[i + 1..]) {
                return Some(m);
            }
        }
        None
    }

    /// The generic icon name shared-mime-info gives a type
    /// (`text-x-script`, `package-x-generic`…): its broad kind.
    pub fn generic_of(&self, mime: &str) -> Option<&str> {
        self.generic.get(mime).map(String::as_str)
    }
}

fn add_pairs(map: &mut HashMap<String, String>, text: &str) {
    for line in text.lines() {
        if let Some((k, v)) = line.split_once(':') {
            map.entry(k.to_string()).or_insert_with(|| v.to_string());
        }
    }
}

// --- families and labels ----------------------------------------------------

/// The broad kind of a file. Each family has one theme colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Code,
    Data,
    Text,
    Doc,
    Image,
    Video,
    Audio,
    Archive,
    App,
    Font,
    Other,
}

impl Family {
    pub fn as_str(self) -> &'static str {
        match self {
            Family::Code => "code",
            Family::Data => "data",
            Family::Text => "text",
            Family::Doc => "doc",
            Family::Image => "image",
            Family::Video => "video",
            Family::Audio => "audio",
            Family::Archive => "archive",
            Family::App => "app",
            Family::Font => "font",
            Family::Other => "other",
        }
    }
}

/// Types the general rule below gets wrong for a file manager's purposes.
/// Kept short on purpose: everything else sorts itself.
const FAMILY_FIXES: &[(&str, Family)] = &[
    ("text/markdown", Family::Text),
    ("text/rust", Family::Code),
    ("text/html", Family::Code),
    ("text/css", Family::Code),
    ("text/x-log", Family::Text),
    ("text/x-gettext-translation", Family::Text),
    ("text/x-patch", Family::Code),
    // `.ts` is claimed by Qt Linguist, `.tsx` by Tiled; both are almost
    // always TypeScript in a developer's folders.
    ("text/vnd.trolltech.linguist", Family::Code),
    ("application/x-tiled-tsx", Family::Code),
    ("application/x-ipynb+json", Family::Code),
    ("application/x-desktop", Family::Data),
    ("application/pdf", Family::Doc),
    ("application/epub+zip", Family::Doc),
    ("image/svg+xml", Family::Image),
    ("application/vnd.efi.iso", Family::Archive),
    ("application/vnd.efi.img", Family::Archive),
    ("application/x-cd-image", Family::Archive),
    ("application/x-iso9660-image", Family::Archive),
    ("application/x-sharedlib", Family::App),
    ("application/x-object", Family::App),
    ("application/x-executable", Family::App),
    ("application/x-pie-executable", Family::App),
    ("application/x-msdownload", Family::App),
];

/// The family of a MIME type. `generic` is its shared-mime-info generic
/// icon, the database's own broad kind.
pub fn family_of(mime: &str, generic: Option<&str>) -> Family {
    if let Some((_, f)) = FAMILY_FIXES.iter().find(|(m, _)| *m == mime) {
        return *f;
    }
    let (top, sub) = mime.split_once('/').unwrap_or((mime, ""));
    let media = matches!(top, "image" | "audio" | "video" | "font" | "model");
    // Structured text (JSON, YAML, TOML, XML, CSV, SQL…) is data, whatever
    // generic icon the database gives it.
    let data = ["json", "yaml", "toml", "xml", "csv", "sql", "tab-separated"]
        .iter()
        .any(|d| sub.contains(d));
    if data && !media {
        return Family::Data;
    }
    match generic {
        Some("package-x-generic" | "media-optical" | "media-floppy" | "application-x-addon") => {
            return Family::Archive
        }
        Some("application-x-executable") => return Family::App,
        Some("image-x-generic") => return Family::Image,
        Some("video-x-generic") => return Family::Video,
        Some("audio-x-generic") => return Family::Audio,
        Some("font-x-generic") => return Family::Font,
        Some("text-x-script" | "text-html") => return Family::Code,
        Some(g) if g.starts_with("x-office-") => return Family::Doc,
        Some("text-x-generic" | "text-plain" | "text-x-generic-template" | "emblem-mail") => {
            return Family::Text
        }
        _ => {}
    }
    match top {
        "image" | "model" => Family::Image,
        "audio" => Family::Audio,
        "video" => Family::Video,
        "font" => Family::Font,
        // text/x-python, text/x-go…: source code. README, AUTHORS and
        // friends are prose.
        "text"
            if sub.starts_with("x-")
                && !matches!(
                    sub,
                    "x-readme" | "x-authors" | "x-copying" | "x-changelog" | "x-install"
                ) =>
        {
            Family::Code
        }
        "text" => Family::Text,
        _ => Family::Other,
    }
}

/// The extension printed on a file's page: lowercase, ASCII letters and
/// digits only, at most five characters. `a.tar.gz` says `tar`; a name
/// with no extension (LICENSE, .bashrc) has none.
pub fn label_for(lower: &str) -> String {
    let body = lower.strip_prefix('.').unwrap_or(lower);
    let Some((stem, ext)) = body.rsplit_once('.') else {
        return String::new();
    };
    if stem.is_empty() {
        return String::new();
    }
    let ext = if stem.ends_with(".tar") && matches!(ext, "gz" | "xz" | "zst" | "bz2" | "lz" | "lz4")
    {
        "tar"
    } else if ext == "appimage" {
        "app"
    } else {
        ext
    };
    if ext.is_empty() || ext.len() > 5 || !ext.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return String::new();
    }
    ext.to_string()
}

/// Extensions no MIME type claims that are still obviously configuration.
const DATA_EXTS: &[&str] = &["conf", "cfg", "ini", "env", "lock", "props"];

// --- what the UI asks ---------------------------------------------------------

/// How one entry is drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Look {
    /// `folder`, `page`, `broken` (a link whose target is gone) or
    /// `special` (a FIFO, socket or device node).
    pub shape: &'static str,
    /// The file family (pages only).
    pub family: Option<Family>,
    /// The extension printed on a page; "" for none.
    pub label: String,
    /// A special folder's glyph name (`documents`, `downloads`…), or
    /// `empty` for a blank page (an empty file with no type).
    pub glyph: Option<&'static str>,
}

impl Look {
    /// `shape\tfamily\tlabel\tglyph`, the form QML receives.
    pub fn wire(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}",
            self.shape,
            self.family.map(Family::as_str).unwrap_or(""),
            self.label,
            self.glyph.unwrap_or("")
        )
    }
}

/// Everything needed to turn a [`key_for`] key into a [`Look`]. The default
/// (nothing loaded) still draws folders and labelled pages, just without
/// family colours or special folders.
#[derive(Debug, Default)]
pub struct Looks {
    pub mime: MimeDb,
    /// Folder path → its glyph (Documents → `documents`).
    special: HashMap<PathBuf, &'static str>,
}

impl Looks {
    pub fn new(mime: MimeDb) -> Self {
        Looks {
            mime,
            special: HashMap::new(),
        }
    }

    /// The MIME database and the user's special folders. Reads files: call
    /// it on a worker.
    pub fn load_current() -> Self {
        let mut looks = Looks::new(MimeDb::load(&mime_dirs()));
        if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
            let home = PathBuf::from(home);
            looks.special = special_folders(&home, &read_user_dirs(&home));
        }
        looks
    }

    /// Mark `path` as a special folder (tests; [`Looks::load_current`]
    /// fills these from user-dirs.dirs).
    pub fn set_special(&mut self, path: PathBuf, glyph: &'static str) {
        self.special.insert(path, glyph);
    }

    pub fn look(&self, key: &str) -> Look {
        let page = |family, label: String| Look {
            shape: "page",
            family: Some(family),
            label,
            glyph: None,
        };
        if let Some(path) = key.strip_prefix("d:") {
            return Look {
                shape: "folder",
                family: None,
                label: String::new(),
                glyph: self.special.get(Path::new(path)).copied(),
            };
        }
        if key == "broken" {
            return Look {
                shape: "broken",
                family: None,
                label: "?".into(),
                glyph: None,
            };
        }
        let Some((flags, name)) = key.strip_prefix('f').and_then(|k| k.split_once(':')) else {
            return Look {
                shape: "special",
                family: None,
                label: String::new(),
                glyph: None,
            };
        };
        let label = label_for(name);
        if let Some(m) = self.mime.mime_for_name(name) {
            return page(family_of(m, self.mime.generic_of(m)), label);
        }
        if flags.contains('x') {
            return page(Family::App, label);
        }
        if label.is_empty() {
            // LICENSE, Makefile-like names, dotfiles: prose or config, drawn
            // as a page of text. An empty one is drawn blank.
            let mut l = page(Family::Text, label);
            if flags.contains('z') {
                l.glyph = Some("empty");
            }
            return l;
        }
        let family = if DATA_EXTS.contains(&label.as_str()) {
            Family::Data
        } else {
            Family::Other
        };
        page(family, label)
    }
}

fn data_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from);
    match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(d) => out.push(PathBuf::from(d)),
        None => {
            if let Some(h) = &home {
                out.push(h.join(".local/share"));
            }
        }
    }
    let sys = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    out.extend(sys.split(':').filter(|s| !s.is_empty()).map(PathBuf::from));
    out
}

fn mime_dirs() -> Vec<PathBuf> {
    data_dirs().into_iter().map(|d| d.join("mime")).collect()
}

/// `XDG_*_DIR="$HOME/…"` lines of `user-dirs.dirs`, expanded.
fn read_user_dirs(home: &Path) -> HashMap<String, PathBuf> {
    let cfg = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|x| !x.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let text = std::fs::read_to_string(cfg.join("user-dirs.dirs")).unwrap_or_default();
    parse_user_dirs(&text, home)
}

/// Parse `user-dirs.dirs` text. Pure, for tests.
pub fn parse_user_dirs(text: &str, home: &Path) -> HashMap<String, PathBuf> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let Some(name) = k.strip_prefix("XDG_").and_then(|k| k.strip_suffix("_DIR")) else {
            continue;
        };
        let v = v.trim().trim_matches('"');
        let path = match v.strip_prefix("$HOME") {
            Some(rest) => home.join(rest.trim_start_matches('/')),
            None if v.starts_with('/') => PathBuf::from(v),
            None => continue,
        };
        out.insert(name.to_string(), path);
    }
    out
}

/// Special folders → glyph names. A folder set to `$HOME` itself (a common
/// way of switching one off) is skipped, or home would get the Desktop glyph.
pub fn special_folders(
    home: &Path,
    dirs: &HashMap<String, PathBuf>,
) -> HashMap<PathBuf, &'static str> {
    const MAP: &[(&str, &str)] = &[
        ("DESKTOP", "desktop"),
        ("DOWNLOAD", "downloads"),
        ("TEMPLATES", "templates"),
        ("PUBLICSHARE", "public"),
        ("DOCUMENTS", "documents"),
        ("MUSIC", "music"),
        ("PICTURES", "pictures"),
        ("VIDEOS", "videos"),
    ];
    let norm = |p: &Path| p.components().collect::<PathBuf>();
    let mut out = HashMap::new();
    out.insert(norm(home), "home");
    for (var, icon) in MAP {
        if let Some(p) = dirs.get(*var) {
            let p = norm(p);
            if p != norm(home) {
                out.insert(p, *icon);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn meta(mode: u32, uid: u32, gid: u32) -> Metadata {
        Metadata {
            mode,
            uid,
            gid,
            size: 10,
            ..Default::default()
        }
    }

    #[test]
    fn keys_describe_folders_files_and_links() {
        let p = Path::new("/home/u/Documents");
        assert_eq!(
            key_for(OsStr::new("Documents"), EntryKind::Dir, None, p),
            "d:/home/u/Documents"
        );
        let f = Path::new("/x/Run.SH");
        assert_eq!(
            key_for(OsStr::new("Run.SH"), EntryKind::File, None, f),
            "f:run.sh"
        );
        let m = Metadata {
            mode: 0o100755,
            size: 0,
            ..Default::default()
        };
        assert_eq!(
            key_for(OsStr::new("tool"), EntryKind::File, Some(&m), f),
            "fxz:tool"
        );
        let link_dir = Metadata {
            link_ok: Some(true),
            link_dir: true,
            mode: 0o777,
            ..Default::default()
        };
        assert_eq!(
            key_for(OsStr::new("l"), EntryKind::Symlink, Some(&link_dir), p),
            "d:/home/u/Documents"
        );
        let broken = Metadata {
            link_ok: Some(false),
            ..Default::default()
        };
        assert_eq!(
            key_for(OsStr::new("l"), EntryKind::Symlink, Some(&broken), p),
            "broken"
        );
        // A link's own mode (0777) must not make it look executable.
        let link_file = Metadata {
            link_ok: Some(true),
            mode: 0o120777,
            ..Default::default()
        };
        assert_eq!(
            key_for(OsStr::new("a.txt"), EntryKind::Symlink, Some(&link_file), p),
            "f:a.txt"
        );
    }

    #[test]
    fn non_utf8_names_still_get_a_key() {
        use std::os::unix::ffi::OsStringExt;
        let n = OsString::from_vec(vec![b'a', 0xff, b'.', b'j', b's', b'o', b'n']);
        let k = key_for(&n, EntryKind::File, None, Path::new("/x"));
        assert!(k.ends_with(".json"), "{k}");
    }

    #[test]
    fn permission_bits_decide_readonly_and_unreadable() {
        let me = Access::new(1000, vec![1000, 10]);
        // Owned by root, 0755: we may read but not write (the ~/models case).
        let root_dir = meta(0o40755, 0, 0);
        assert!(!me.can_write(&root_dir));
        assert!(me.can_read(&root_dir, true));
        // Ours, 0755.
        assert!(me.can_write(&meta(0o40755, 1000, 1000)));
        // Group-writable for a group we are in.
        assert!(me.can_write(&meta(0o40775, 0, 10)));
        // 0700 root folder: no way in.
        assert!(!me.can_read(&meta(0o40700, 0, 0), true));
        // r-- folder without x cannot be opened; r-- file can be read.
        assert!(!me.can_read(&meta(0o40744, 0, 0), true));
        assert!(me.can_read(&meta(0o100744, 0, 0), false));
        // Root writes everything.
        assert!(Access::new(0, vec![0]).can_write(&root_dir));
    }

    #[test]
    fn emblems_show_mounts_links_and_locks() {
        let me = Access::current();
        let mut m = meta(0o40755, me.uid, 0);
        m.mount_root = true;
        assert_eq!(emblems_for(EntryKind::Dir, Some(&m)), vec!["mount"]);
        let link = Metadata {
            mode: 0o120777,
            uid: 0,
            link_ok: Some(true),
            ..Default::default()
        };
        // Not "read-only" just because root owns the link itself.
        assert_eq!(emblems_for(EntryKind::Symlink, Some(&link)), vec!["link"]);
        assert!(emblems_for(EntryKind::File, None).is_empty());
        if me.uid != 0 {
            let theirs = meta(0o40755, me.uid.wrapping_add(4242), 4242);
            assert_eq!(emblems_for(EntryKind::Dir, Some(&theirs)), vec!["readonly"]);
            let closed = meta(0o40700, me.uid.wrapping_add(4242), 4242);
            assert_eq!(
                emblems_for(EntryKind::Dir, Some(&closed)),
                vec!["unreadable"]
            );
        }
    }

    const GLOBS: &str = "# comment\n\
        50:application/x-compressed-tar:*.tar.gz\n\
        50:application/gzip:*.gz\n\
        50:text/x-shellscript:*.sh\n\
        50:application/json:*.json\n\
        50:application/schema+json:*.json\n\
        10:text/x-makefile:makefile\n\
        50:text/x-c:*.[ch]\n\
        0:model/x:__NOGLOBS__\n\
        80:text/html:*.html\n\
        40:text/html:*.htm\n";

    #[test]
    fn globs_pick_the_longest_suffix_and_skip_fancy_patterns() {
        let mut db = MimeDb::default();
        db.add_globs2(GLOBS);
        assert_eq!(
            db.mime_for_name("a.tar.gz"),
            Some("application/x-compressed-tar")
        );
        assert_eq!(db.mime_for_name("a.gz"), Some("application/gzip"));
        assert_eq!(
            db.mime_for_name("skills-lock.json"),
            Some("application/json")
        );
        assert_eq!(db.mime_for_name("makefile"), Some("text/x-makefile"));
        assert_eq!(db.mime_for_name("x.sh"), Some("text/x-shellscript"));
        assert_eq!(db.mime_for_name("noext"), None);
        assert_eq!(
            db.mime_for_name("main.c"),
            None,
            "bracket globs are skipped"
        );
    }

    #[test]
    fn user_dirs_expand_home_and_skip_folders_set_to_home() {
        let home = Path::new("/home/k");
        let dirs = parse_user_dirs(
            "# x\nXDG_DESKTOP_DIR=\"$HOME/\"\nXDG_DOCUMENTS_DIR=\"$HOME/Documents\"\n\
             XDG_MUSIC_DIR=\"/data/Music\"\nXDG_BAD_DIR=\"relative\"\n",
            home,
        );
        assert_eq!(dirs["DOCUMENTS"], PathBuf::from("/home/k/Documents"));
        assert_eq!(dirs["MUSIC"], PathBuf::from("/data/Music"));
        assert!(!dirs.contains_key("BAD"));
        let sp = special_folders(home, &dirs);
        assert_eq!(sp[Path::new("/home/k")], "home", "not desktop");
        assert_eq!(sp[Path::new("/home/k/Documents")], "documents");
        assert_eq!(sp[Path::new("/data/Music")], "music");
    }

    #[test]
    fn families_follow_the_mime_database_with_a_few_fixes() {
        assert_eq!(
            family_of("text/x-python", Some("text-x-script")),
            Family::Code
        );
        assert_eq!(family_of("text/x-go", None), Family::Code);
        assert_eq!(family_of("text/rust", None), Family::Code);
        assert_eq!(
            family_of("application/json", Some("text-x-script")),
            Family::Data
        );
        assert_eq!(
            family_of("application/toml", Some("text-x-generic")),
            Family::Data
        );
        assert_eq!(
            family_of("text/markdown", Some("x-office-document")),
            Family::Text
        );
        assert_eq!(family_of("text/plain", None), Family::Text);
        assert_eq!(family_of("text/x-readme", None), Family::Text);
        assert_eq!(
            family_of("application/pdf", Some("x-office-document")),
            Family::Doc
        );
        assert_eq!(family_of("image/svg+xml", None), Family::Image);
        assert_eq!(family_of("video/mp4", None), Family::Video);
        assert_eq!(family_of("audio/flac", None), Family::Audio);
        assert_eq!(
            family_of("application/x-compressed-tar", Some("package-x-generic")),
            Family::Archive
        );
        assert_eq!(family_of("application/vnd.efi.iso", None), Family::Archive);
        assert_eq!(
            family_of("application/vnd.appimage", Some("application-x-executable")),
            Family::App
        );
        assert_eq!(family_of("font/ttf", Some("font-x-generic")), Family::Font);
        assert_eq!(family_of("application/x-pem-file", None), Family::Other);
    }

    #[test]
    fn labels_are_the_extension_short_and_plain() {
        assert_eq!(label_for("main.rs"), "rs");
        assert_eq!(label_for("skills-lock.json"), "json");
        assert_eq!(label_for("archive.tar.gz"), "tar");
        assert_eq!(label_for("gust.appimage"), "app");
        assert_eq!(label_for("font.woff2"), "woff2");
        assert_eq!(label_for("license"), "");
        assert_eq!(label_for(".bashrc"), "");
        assert_eq!(label_for(".config.toml"), "toml");
        assert_eq!(label_for("trailing."), "");
        assert_eq!(label_for("x.markdown"), "", "longer than five: no label");
        assert_eq!(
            label_for("a.b|c"),
            "",
            "only letters and digits are printed"
        );
    }

    #[test]
    fn looks_for_folders_files_links_and_specials() {
        let mut db = MimeDb::default();
        db.add_globs2("50:text/plain:*.txt\n50:application/json:*.json\n50:text/x-python:*.py\n");
        let mut looks = Looks::new(db);
        looks.set_special(PathBuf::from("/home/k/Documents"), "documents");
        let l = looks.look("d:/home/k/Documents");
        assert_eq!((l.shape, l.glyph), ("folder", Some("documents")));
        assert_eq!(looks.look("d:/home/k/Other").glyph, None);
        let j = looks.look("f:skills-lock.json");
        assert_eq!(
            (j.shape, j.family, j.label.as_str()),
            ("page", Some(Family::Data), "json")
        );
        assert_eq!(looks.look("f:run.py").family, Some(Family::Code));
        assert_eq!(looks.look("f:notes.txt").family, Some(Family::Text));
        // No type known: executable → program, no extension → text,
        // config-looking extension → data, anything else → other.
        assert_eq!(looks.look("fx:tool").family, Some(Family::App));
        let lic = looks.look("f:license");
        assert_eq!((lic.family, lic.label.as_str()), (Some(Family::Text), ""));
        assert_eq!(looks.look("fz:empty").glyph, Some("empty"));
        assert_eq!(looks.look("f:license").glyph, None);
        assert_eq!(looks.look("f:app.conf").family, Some(Family::Data));
        assert_eq!(looks.look("f:blob.xyz").family, Some(Family::Other));
        assert_eq!(looks.look("broken").shape, "broken");
        assert_eq!(looks.look("special").shape, "special");
        assert_eq!(looks.look("f:a.json").wire(), "page\tdata\tjson\t");
        assert_eq!(
            looks.look("d:/home/k/Documents").wire(),
            "folder\t\t\tdocuments"
        );
    }

    /// The real database, when this machine has one: the families the user
    /// was shown on the design page.
    #[test]
    fn the_system_mime_database_sorts_common_files() {
        let dirs = vec![PathBuf::from("/usr/share/mime")];
        if !dirs[0].join("globs2").is_file() {
            return;
        }
        let looks = Looks::new(MimeDb::load(&dirs));
        let fam = |n: &str| looks.look(&format!("f:{n}")).family.map(Family::as_str);
        for (name, want) in [
            ("main.rs", "code"),
            ("app.py", "code"),
            ("index.ts", "code"),
            ("run.sh", "code"),
            ("view.qml", "code"),
            ("page.html", "code"),
            ("style.css", "code"),
            ("skills-lock.json", "data"),
            ("config.toml", "data"),
            ("ci.yaml", "data"),
            ("readme.md", "text"),
            ("notes.txt", "text"),
            ("build.log", "text"),
            ("invoice.pdf", "doc"),
            ("photo.jpg", "image"),
            ("logo.svg", "image"),
            ("clip.mp4", "video"),
            ("song.flac", "audio"),
            ("archive.tar.gz", "archive"),
            ("arch.iso", "archive"),
            ("gust.appimage", "app"),
            ("font.ttf", "font"),
        ] {
            assert_eq!(fam(name), Some(want), "{name}");
        }
    }

    #[test]
    fn emblems_name_broken_links() {
        let broken = Metadata {
            mode: 0o120777,
            link_ok: Some(false),
            ..Default::default()
        };
        assert_eq!(
            emblems_for(EntryKind::Symlink, Some(&broken)),
            vec!["broken-link"]
        );
    }
}
