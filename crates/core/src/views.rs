//! Remembering the view a user explicitly chose for a location.
//!
//! The plan requires Gust to "remember the explicit choice per stable
//! location" and to "suggest Grid for recognised XDG Pictures locations"
//! *without* switching a view under the user. Those two rules only coexist
//! if a **suggestion is never recorded as a decision** — otherwise the first
//! visit to Pictures would harden into a stored preference nobody made, and
//! a later change of the suggestion rule could never reach that user.
//!
//! See `docs/DESIGN.md` §4.0. The file is a cache of preferences, never a
//! source of truth: anything unreadable, truncated or hostile degrades to
//! defaults rather than refusing to start.

use crate::location::LocationKey;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The views a location can be remembered in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Rows,
    Grid,
    Columns,
}

impl ViewMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ViewMode::Rows => "rows",
            ViewMode::Grid => "grid",
            ViewMode::Columns => "columns",
        }
    }

    /// Parse a stored name. Unknown names are ignored, never guessed at.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "rows" => Some(ViewMode::Rows),
            "grid" => Some(ViewMode::Grid),
            "columns" => Some(ViewMode::Columns),
            _ => None,
        }
    }
}

/// At most this many remembered locations. Oldest evicted first.
const MAX_ENTRIES: usize = 500;

/// Remembered explicit view choices, keyed by `LocationKey::to_token()`.
#[derive(Debug, Default, Clone)]
pub struct ViewMemory {
    /// token -> (view, sequence number for eviction order)
    entries: BTreeMap<String, (ViewMode, u64)>,
    next_seq: u64,
    dirty: bool,
}

impl ViewMemory {
    pub fn new() -> Self {
        Self::default()
    }

    /// The view the user explicitly chose here, if any.
    ///
    /// `None` means "no decision recorded" — the caller applies its own
    /// default or suggestion. It never means "Rows".
    pub fn get(&self, key: &LocationKey) -> Option<ViewMode> {
        self.entries.get(&key.to_token()).map(|(v, _)| *v)
    }

    /// Record an EXPLICIT choice. Never call this for a suggestion.
    pub fn remember(&mut self, key: &LocationKey, view: ViewMode) {
        let token = key.to_token();
        let seq = self.next_seq;
        self.next_seq += 1;
        if self.entries.insert(token, (view, seq)).is_none() {
            self.evict_if_needed();
        }
        self.dirty = true;
    }

    /// Forget a location's preference, so it follows defaults again.
    pub fn forget(&mut self, key: &LocationKey) {
        if self.entries.remove(&key.to_token()).is_some() {
            self.dirty = true;
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Has anything changed since the last load or save?
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn evict_if_needed(&mut self) {
        while self.entries.len() > MAX_ENTRIES {
            // Oldest by sequence number.
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, seq))| *seq)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    /// Serialize to the on-disk form.
    ///
    /// Deliberately hand-written rather than a TOML crate: the format is two
    /// safe fields, and a parser we control cannot be surprised by a feature
    /// of someone else's grammar.
    pub fn to_text(&self) -> String {
        let mut out = String::from(
            "# Gust remembers the view you explicitly chose for a location.\n\
             # Keys are hex-encoded OS bytes, so unusual filenames work too.\n\
             # Delete a line to go back to the default for that folder.\n",
        );
        // Stable order: BTreeMap iterates sorted, so the file does not churn.
        for (token, (view, _)) in &self.entries {
            out.push_str(token);
            out.push_str(" = \"");
            out.push_str(view.as_str());
            out.push_str("\"\n");
        }
        out
    }

    /// Parse the on-disk form. Anything unrecognised is skipped silently.
    ///
    /// A preferences file is never allowed to break startup, so this has no
    /// error case: a corrupt file simply yields fewer (or zero) entries.
    pub fn from_text(text: &str) -> Self {
        let mut m = ViewMemory::new();
        // Bounded: refuse to build an unbounded map from a hostile file.
        for line in text.lines().take(MAX_ENTRIES * 4) {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let token = k.trim();
            let value = v.trim().trim_matches('"');
            // The token must be one we could have written: a real key.
            if LocationKey::from_token(token).is_none() {
                continue;
            }
            let Some(view) = ViewMode::parse(value) else {
                continue;
            };
            if m.entries.len() >= MAX_ENTRIES {
                break;
            }
            let seq = m.next_seq;
            m.next_seq += 1;
            m.entries.insert(token.to_string(), (view, seq));
        }
        m.dirty = false;
        m
    }

    /// Where preferences live. Honours `XDG_CONFIG_HOME`.
    pub fn default_path() -> Option<PathBuf> {
        if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
            if !x.is_empty() {
                return Some(PathBuf::from(x).join("gust").join("views.toml"));
            }
        }
        std::env::var_os("HOME").filter(|h| !h.is_empty()).map(|h| {
            PathBuf::from(h)
                .join(".config")
                .join("gust")
                .join("views.toml")
        })
    }

    /// Load preferences, or an empty set if anything at all goes wrong.
    pub fn load(path: &Path) -> Self {
        // Bounded read: a preferences file has no business being large, and
        // we will not map an arbitrary file into memory because it sits at
        // the expected path.
        const MAX_FILE: u64 = 512 * 1024;
        let Ok(md) = path.symlink_metadata() else {
            return ViewMemory::new();
        };
        // A symlink here would be a way to make Gust read something else.
        if !md.is_file() || md.len() > MAX_FILE {
            return ViewMemory::new();
        }
        match std::fs::read_to_string(path) {
            Ok(text) => ViewMemory::from_text(&text),
            Err(_) => ViewMemory::new(),
        }
    }

    /// Save atomically with owner-only permissions.
    ///
    /// Stage then rename, so a crash never leaves a half-written file; mode
    /// 0600 because which directories somebody browses is their business.
    pub fn save(&mut self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let Some(dir) = path.parent() else {
            return Ok(());
        };
        std::fs::create_dir_all(dir)?;

        let stage = dir.join(format!(".views-{}.tmp", std::process::id()));
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&stage)?;
        let write = f
            .write_all(self.to_text().as_bytes())
            .and_then(|_| f.sync_all());
        drop(f);
        if let Err(e) = write {
            let _ = std::fs::remove_file(&stage);
            return Err(e);
        }
        if let Err(e) = std::fs::rename(&stage, path) {
            let _ = std::fs::remove_file(&stage);
            return Err(e);
        }
        self.dirty = false;
        Ok(())
    }
}

/// Should Gust *suggest* Grid here?
///
/// A suggestion is not a decision: it applies only when nothing was stored,
/// and is never written back. Recognised by the XDG user-dirs name, which is
/// why this asks about the path rather than scanning the folder — the plan
/// forbids reading a huge directory to guess a layout.
pub fn suggests_grid(key: &LocationKey) -> bool {
    let Some(p) = key.as_path() else {
        return false;
    };
    let Some(pictures) = xdg_pictures_dir() else {
        return false;
    };
    p == pictures
}

/// The user's Pictures directory from XDG user-dirs, if configured.
fn xdg_pictures_dir() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("XDG_PICTURES_DIR") {
        if !v.is_empty() {
            return Some(PathBuf::from(v));
        }
    }
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    let cfg = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|x| !x.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&home).join(".config"));
    let dirs = cfg.join("user-dirs.dirs");
    let text = std::fs::read_to_string(dirs).ok()?;
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("XDG_PICTURES_DIR=") else {
            continue;
        };
        let val = rest.trim().trim_matches('"');
        let expanded = val.strip_prefix("$HOME/").map_or_else(
            || PathBuf::from(val),
            |tail| PathBuf::from(&home).join(tail),
        );
        return Some(expanded);
    }
    // Not configured: fall back to the conventional location only if it
    // actually exists, so we never suggest Grid for a folder that is not
    // really the user's Pictures.
    let conventional = PathBuf::from(&home).join("Pictures");
    conventional.is_dir().then_some(conventional)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    fn key(p: &str) -> LocationKey {
        LocationKey::local(p)
    }

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gust-views-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn an_explicit_choice_is_remembered_per_location() {
        let mut m = ViewMemory::new();
        m.remember(&key("/a"), ViewMode::Grid);
        m.remember(&key("/b"), ViewMode::Columns);

        assert_eq!(m.get(&key("/a")), Some(ViewMode::Grid));
        assert_eq!(m.get(&key("/b")), Some(ViewMode::Columns));
        // A location nobody chose for has NO opinion — not a default.
        assert_eq!(m.get(&key("/c")), None);
    }

    #[test]
    fn a_round_trip_through_the_file_format_keeps_every_choice() {
        let mut m = ViewMemory::new();
        m.remember(&key("/one"), ViewMode::Grid);
        m.remember(&key("/two"), ViewMode::Rows);
        m.remember(&key("/three"), ViewMode::Columns);

        let back = ViewMemory::from_text(&m.to_text());
        assert_eq!(back.get(&key("/one")), Some(ViewMode::Grid));
        assert_eq!(back.get(&key("/two")), Some(ViewMode::Rows));
        assert_eq!(back.get(&key("/three")), Some(ViewMode::Columns));
        assert_eq!(back.len(), 3);
    }

    #[test]
    fn a_non_utf8_directory_name_keeps_its_preference() {
        // 0xFF is not valid UTF-8. This is the case a String-keyed
        // preferences file would silently corrupt or drop.
        let raw = OsString::from_vec(b"/tmp/pi\xffcs".to_vec());
        let k = LocationKey::local(PathBuf::from(&raw));
        let mut m = ViewMemory::new();
        m.remember(&k, ViewMode::Grid);

        let back = ViewMemory::from_text(&m.to_text());
        assert_eq!(
            back.get(&k),
            Some(ViewMode::Grid),
            "a non-UTF-8 path must survive the preferences file"
        );
    }

    #[test]
    fn a_corrupt_file_degrades_to_defaults_rather_than_failing() {
        for junk in [
            "",
            "not a config at all",
            "= = = =",
            "\0\0\0",
            "local:zzzz = \"grid\"",           // not valid hex
            "local:6161 = \"telepathy\"",      // unknown view
            "unknownprovider:6161 = \"grid\"", // unknown provider
            "6161 = \"grid\"",                 // no provider prefix
            &"x".repeat(100_000),
        ] {
            let m = ViewMemory::from_text(junk);
            assert_eq!(m.len(), 0, "junk produced entries: {junk:?}");
        }
    }

    #[test]
    fn a_hostile_file_cannot_grow_memory_without_bound() {
        let mut text = String::new();
        for i in 0..5000 {
            let k = key(&format!("/dir{i}"));
            text.push_str(&format!("{} = \"grid\"\n", k.to_token()));
        }
        let m = ViewMemory::from_text(&text);
        assert!(m.len() <= MAX_ENTRIES, "unbounded: {} entries", m.len());
    }

    #[test]
    fn remembering_many_locations_evicts_the_oldest() {
        let mut m = ViewMemory::new();
        for i in 0..(MAX_ENTRIES + 50) {
            m.remember(&key(&format!("/d{i}")), ViewMode::Grid);
        }
        assert!(m.len() <= MAX_ENTRIES);
        // The most recent choices survived.
        assert_eq!(
            m.get(&key(&format!("/d{}", MAX_ENTRIES + 49))),
            Some(ViewMode::Grid)
        );
        // The very first was evicted.
        assert_eq!(m.get(&key("/d0")), None);
    }

    #[test]
    fn saving_is_atomic_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("save");
        let f = d.join("views.toml");
        let mut m = ViewMemory::new();
        m.remember(&key("/secret/place"), ViewMode::Grid);
        m.save(&f).unwrap();

        let mode = std::fs::metadata(&f).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "preferences must not be world-readable");

        // No staging file left behind.
        let leftovers: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .filter(|n| n.to_string_lossy().starts_with(".views-"))
            .collect();
        assert!(leftovers.is_empty(), "staging debris: {leftovers:?}");

        let back = ViewMemory::load(&f);
        assert_eq!(back.get(&key("/secret/place")), Some(ViewMode::Grid));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn loading_a_missing_or_odd_file_never_panics() {
        let d = tmp("odd");
        assert_eq!(ViewMemory::load(&d.join("nope.toml")).len(), 0);

        // A directory where the file should be.
        std::fs::create_dir_all(d.join("adir")).unwrap();
        assert_eq!(ViewMemory::load(&d.join("adir")).len(), 0);

        // A symlink must not be followed: it is a way to make Gust read
        // a file it was never pointed at.
        std::fs::write(d.join("real.toml"), "x").unwrap();
        std::os::unix::fs::symlink(d.join("real.toml"), d.join("link.toml")).unwrap();
        assert_eq!(ViewMemory::load(&d.join("link.toml")).len(), 0);

        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn forgetting_a_location_restores_the_default() {
        let mut m = ViewMemory::new();
        m.remember(&key("/x"), ViewMode::Columns);
        assert_eq!(m.get(&key("/x")), Some(ViewMode::Columns));
        m.forget(&key("/x"));
        assert_eq!(m.get(&key("/x")), None);
    }

    #[test]
    fn a_suggestion_is_never_written_to_the_file() {
        // The API makes this structural: `suggests_grid` returns a bool and
        // has no way to touch a ViewMemory. This test documents the rule and
        // fails loudly if someone adds a write path.
        let mut m = ViewMemory::new();
        let pics = key("/home/someone/Pictures");
        let _ = suggests_grid(&pics);
        assert_eq!(
            m.get(&pics),
            None,
            "asking for a suggestion must not record a preference"
        );
        assert!(!m.is_dirty(), "a suggestion must not dirty the file");
        // Only an explicit choice writes.
        m.remember(&pics, ViewMode::Rows);
        assert!(m.is_dirty());
        assert_eq!(
            m.get(&pics),
            Some(ViewMode::Rows),
            "an explicit choice must beat the suggestion"
        );
    }

    #[test]
    fn a_save_writes_only_what_is_in_memory() {
        // Deleting the file under a running Gust does not erase its
        // in-memory copy: the next save legitimately writes those entries
        // back. This is the documented behaviour, and the reason a test
        // that clears preferences must restart the app rather than just
        // removing the file.
        let d = tmp("inmemory");
        let f = d.join("views.toml");
        let mut m = ViewMemory::new();
        m.remember(&key("/kept"), ViewMode::Grid);
        m.save(&f).unwrap();

        std::fs::remove_file(&f).unwrap();
        // A second save restores what memory still holds.
        m.remember(&key("/also"), ViewMode::Rows);
        m.save(&f).unwrap();

        let back = ViewMemory::load(&f);
        assert_eq!(back.get(&key("/kept")), Some(ViewMode::Grid));
        assert_eq!(back.get(&key("/also")), Some(ViewMode::Rows));
        assert_eq!(back.len(), 2, "a save writes exactly the live set");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn unparseable_entries_are_dropped_rather_than_preserved() {
        // A line we cannot parse is not carried forward on the next save:
        // Gust must never re-persist something it refused to understand.
        let hostile = "evil:../../etc/passwd = \"grid\"\nlocal:zz = \"grid\"\n";
        let mut m = ViewMemory::from_text(hostile);
        assert_eq!(m.len(), 0, "hostile entries must not load");

        m.remember(&key("/real"), ViewMode::Columns);
        let text = m.to_text();
        assert!(
            !text.contains("etc/passwd") && !text.contains("evil:"),
            "an unparseable entry was written back: {text}"
        );
        assert!(text.contains("columns"));
    }

    #[test]
    fn the_file_does_not_churn_between_saves() {
        let mut a = ViewMemory::new();
        for p in ["/z", "/a", "/m"] {
            a.remember(&key(p), ViewMode::Grid);
        }
        let first = a.to_text();
        let second = ViewMemory::from_text(&first).to_text();
        assert_eq!(first, second, "round-tripping must be stable");
    }
}
