//! Remembering the sort a user chose for a folder (per-folder sort).
//!
//! Same rules as `views.rs`: only an explicit choice is recorded, the file
//! is a cache of preferences (anything unreadable degrades to defaults), keys
//! are `LocationKey` tokens (hex OS bytes), and it is saved atomically 0600.
//!
//! A folder with nothing recorded sorts by name, ascending — it does NOT
//! inherit whatever the previous folder happened to use, or a "by size" set
//! for one downloads folder would silently follow the user everywhere.

use crate::entry::{SortField, SortSpec};
use crate::location::LocationKey;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// At most this many remembered folders. Oldest evicted first.
const MAX_ENTRIES: usize = 500;

fn field_str(f: SortField) -> &'static str {
    match f {
        SortField::Name => "name",
        SortField::Size => "size",
        SortField::Modified => "modified",
        SortField::Kind => "kind",
    }
}

/// "kind-desc" and friends. Unknown text is ignored, never guessed at.
pub fn parse_sort(s: &str) -> Option<(SortField, bool)> {
    let (f, dir) = s.split_once('-')?;
    let field = match f {
        "name" => SortField::Name,
        "size" => SortField::Size,
        "modified" => SortField::Modified,
        "kind" => SortField::Kind,
        _ => return None,
    };
    let ascending = match dir {
        "asc" => true,
        "desc" => false,
        _ => return None,
    };
    Some((field, ascending))
}

pub fn sort_text(field: SortField, ascending: bool) -> String {
    format!(
        "{}-{}",
        field_str(field),
        if ascending { "asc" } else { "desc" }
    )
}

/// The sort a folder with no recorded choice uses.
pub fn default_sort() -> (SortField, bool) {
    (SortField::Name, true)
}

#[derive(Debug, Default, Clone)]
pub struct SortMemory {
    entries: BTreeMap<String, ((SortField, bool), u64)>,
    next_seq: u64,
}

impl SortMemory {
    pub fn new() -> Self {
        Self::default()
    }

    /// The recorded choice, or `None` (the caller applies `default_sort`).
    pub fn get(&self, key: &LocationKey) -> Option<(SortField, bool)> {
        self.entries.get(&key.to_token()).map(|(v, _)| *v)
    }

    /// What this folder sorts by: its recorded choice, else the default.
    pub fn for_folder(&self, key: &LocationKey) -> (SortField, bool) {
        self.get(key).unwrap_or_else(default_sort)
    }

    /// Record an EXPLICIT choice. Choosing the default forgets the folder,
    /// so the file only holds folders that differ from it.
    pub fn remember(&mut self, key: &LocationKey, field: SortField, ascending: bool) {
        let token = key.to_token();
        if (field, ascending) == default_sort() {
            self.entries.remove(&token);
            return;
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        self.entries.insert(token, ((field, ascending), seq));
        while self.entries.len() > MAX_ENTRIES {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, s))| *s)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn to_text(&self) -> String {
        let mut out = String::from(
            "# Gust remembers the sort you chose for a folder.\n\
             # Keys are hex-encoded OS bytes. Delete a line to go back to by-name.\n",
        );
        for (token, ((f, asc), _)) in &self.entries {
            out.push_str(token);
            out.push_str(" = \"");
            out.push_str(&sort_text(*f, *asc));
            out.push_str("\"\n");
        }
        out
    }

    /// Never fails: a corrupt or hostile file yields fewer (or zero) entries.
    pub fn from_text(text: &str) -> Self {
        let mut m = SortMemory::new();
        for line in text.lines().take(MAX_ENTRIES * 4) {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let token = k.trim();
            if LocationKey::from_token(token).is_none() {
                continue;
            }
            let Some(sort) = parse_sort(v.trim().trim_matches('"')) else {
                continue;
            };
            if m.entries.len() >= MAX_ENTRIES {
                break;
            }
            let seq = m.next_seq;
            m.next_seq += 1;
            m.entries.insert(token.to_string(), (sort, seq));
        }
        m
    }

    /// `$XDG_CONFIG_HOME/gust/sorts.toml` (or `~/.config/…`).
    pub fn default_path() -> Option<PathBuf> {
        if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").filter(|x| !x.is_empty()) {
            return Some(PathBuf::from(x).join("gust").join("sorts.toml"));
        }
        std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(|h| PathBuf::from(h).join(".config/gust/sorts.toml"))
    }

    /// Bounded, symlink-refusing read; anything wrong gives an empty memory.
    pub fn load(path: &Path) -> Self {
        const MAX_FILE: u64 = 512 * 1024;
        let Ok(md) = path.symlink_metadata() else {
            return SortMemory::new();
        };
        if !md.is_file() || md.len() > MAX_FILE {
            return SortMemory::new();
        }
        std::fs::read_to_string(path)
            .map(|t| SortMemory::from_text(&t))
            .unwrap_or_default()
    }

    /// Stage, fsync, rename; owner-only.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let Some(dir) = path.parent() else {
            return Ok(());
        };
        std::fs::create_dir_all(dir)?;
        let stage = dir.join(format!(".sorts-{}.tmp", std::process::id()));
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&stage)?;
        let w = f
            .write_all(self.to_text().as_bytes())
            .and_then(|_| f.sync_all());
        drop(f);
        if let Err(e) = w.and_then(|_| std::fs::rename(&stage, path)) {
            let _ = std::fs::remove_file(&stage);
            return Err(e);
        }
        Ok(())
    }
}

/// Apply a folder's sort to a spec (dirs-first is not per-folder).
pub fn spec_for(mem: &SortMemory, key: &LocationKey) -> SortSpec {
    let (field, ascending) = mem.for_folder(key);
    SortSpec {
        field,
        ascending,
        dirs_first: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(p: &str) -> LocationKey {
        LocationKey::local(Path::new(p))
    }

    #[test]
    fn a_choice_is_per_folder_and_others_use_the_default() {
        let mut m = SortMemory::new();
        m.remember(&k("/a"), SortField::Kind, false);
        assert_eq!(m.for_folder(&k("/a")), (SortField::Kind, false));
        assert_eq!(m.for_folder(&k("/b")), default_sort(), "no inheritance");
    }

    #[test]
    fn choosing_the_default_forgets_the_folder() {
        let mut m = SortMemory::new();
        m.remember(&k("/a"), SortField::Size, true);
        m.remember(&k("/a"), SortField::Name, true);
        assert!(m.is_empty());
    }

    #[test]
    fn round_trips_and_ignores_garbage() {
        let mut m = SortMemory::new();
        m.remember(&k("/a b/ü"), SortField::Modified, false);
        let mut text = m.to_text();
        text.push_str("zz = \"kind-desc\"\n");
        text.push_str(&format!("{} = \"sideways-up\"\n", k("/c").to_token()));
        text.push_str("no equals sign here\n");
        let back = SortMemory::from_text(&text);
        assert_eq!(back.len(), 1);
        assert_eq!(back.get(&k("/a b/ü")), Some((SortField::Modified, false)));
    }

    #[test]
    fn bounded_and_evicts_the_oldest() {
        let mut m = SortMemory::new();
        for i in 0..(MAX_ENTRIES + 5) {
            m.remember(&k(&format!("/d{i}")), SortField::Size, false);
        }
        assert_eq!(m.len(), MAX_ENTRIES);
        assert!(m.get(&k("/d0")).is_none());
        assert!(m.get(&k(&format!("/d{}", MAX_ENTRIES + 4))).is_some());
    }

    #[test]
    fn saves_owner_only_and_loads_back_but_refuses_a_symlink() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("gust-sorts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join("sorts.toml");
        let mut m = SortMemory::new();
        m.remember(&k("/x"), SortField::Kind, true);
        m.save(&p).unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(
            SortMemory::load(&p).get(&k("/x")),
            Some((SortField::Kind, true))
        );
        let link = dir.join("link.toml");
        std::os::unix::fs::symlink(&p, &link).unwrap();
        assert!(SortMemory::load(&link).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
