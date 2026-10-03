//! The open tabs, kept so the next start can put them back.
//!
//! Same rules as every other state file (`sorts.rs`, `views.rs`): keys are
//! `LocationKey` tokens (hex OS bytes, so any folder name round-trips), the
//! read is bounded and refuses a symlink, anything unreadable degrades to
//! "nothing to restore", and the save is staged, fsynced, renamed, 0600 —
//! which folders somebody had open is private.
//!
//! Only folders are remembered: no selection, no history, no scroll. A tab
//! comes back where it was, nothing more.

use crate::location::LocationKey;
use std::path::{Path, PathBuf};

/// At most this many tabs are kept; more than this is not a session.
pub const MAX_TABS: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTab {
    pub key: LocationKey,
    /// "rows", "grid" or "columns". Anything else reads as "rows".
    pub view: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Session {
    pub tabs: Vec<SessionTab>,
    /// Index of the tab that was on screen.
    pub active: usize,
}

fn clean_view(v: &str) -> String {
    match v {
        "grid" | "columns" => v.to_string(),
        _ => "rows".to_string(),
    }
}

impl Session {
    /// Build from what the window reports: `(token, view)` per tab.
    /// Unreadable tokens are dropped; the active index is clamped.
    pub fn from_parts(parts: &[(String, String)], active: usize) -> Session {
        let tabs: Vec<SessionTab> = parts
            .iter()
            .filter_map(|(t, v)| {
                LocationKey::from_token(t).map(|key| SessionTab {
                    key,
                    view: clean_view(v),
                })
            })
            .take(MAX_TABS)
            .collect();
        let active = active.min(tabs.len().saturating_sub(1));
        Session { tabs, active }
    }

    pub fn to_text(&self) -> String {
        let mut out = String::from(
            "# Gust: the tabs that were open, restored at the next start.\n\
             # Keys are hex-encoded OS bytes. Delete this file to start fresh.\n",
        );
        out.push_str(&format!("active = {}\n", self.active));
        for t in &self.tabs {
            out.push_str(&format!("tab = \"{} {}\"\n", t.key.to_token(), t.view));
        }
        out
    }

    /// Never fails: a corrupt or hostile file yields fewer (or zero) tabs.
    pub fn from_text(text: &str) -> Session {
        let mut parts = Vec::new();
        let mut active = 0usize;
        for line in text.lines().take(MAX_TABS * 4) {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let v = v.trim().trim_matches('"');
            match k.trim() {
                "active" => active = v.parse().unwrap_or(0),
                "tab" => {
                    if let Some((tok, view)) = v.split_once(' ') {
                        parts.push((tok.to_string(), view.to_string()));
                    }
                }
                _ => {}
            }
        }
        Session::from_parts(&parts, active)
    }

    /// `$XDG_STATE_HOME/gust/tabs.toml` (or `~/.local/state/…`).
    pub fn default_path() -> Option<PathBuf> {
        if let Some(x) = std::env::var_os("XDG_STATE_HOME").filter(|x| !x.is_empty()) {
            return Some(PathBuf::from(x).join("gust").join("tabs.toml"));
        }
        std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(|h| PathBuf::from(h).join(".local/state/gust/tabs.toml"))
    }

    /// Bounded, symlink-refusing read; anything wrong gives no session.
    pub fn load(path: &Path) -> Session {
        const MAX_FILE: u64 = 64 * 1024;
        let Ok(md) = path.symlink_metadata() else {
            return Session::default();
        };
        if !md.is_file() || md.len() > MAX_FILE {
            return Session::default();
        }
        std::fs::read_to_string(path)
            .map(|t| Session::from_text(&t))
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
        let stage = dir.join(format!(
            ".tabs-{}-{:?}.tmp",
            std::process::id(),
            std::thread::current().id()
        ));
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

    /// The tabs worth opening: local folders that still exist. A folder on a
    /// place that can stall (network, FUSE) is kept without being touched —
    /// the tab's own navigation probes it, so a dead server cannot stop
    /// Gust from starting.
    pub fn still_there(&self) -> Session {
        let parts: Vec<(String, String)> = self
            .tabs
            .iter()
            .filter(|t| match t.key.as_path() {
                Some(p) => crate::stall::needs_probe(p) || p.is_dir(),
                None => false,
            })
            .map(|t| (t.key.to_token(), t.view.clone()))
            .collect();
        // Keep the same tab active when it survived; else the first.
        let was = self.tabs.get(self.active).map(|t| t.key.to_token());
        let active = was
            .and_then(|w| parts.iter().position(|(t, _)| *t == w))
            .unwrap_or(0);
        Session::from_parts(&parts, active)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gust-session-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn tok(p: &Path) -> String {
        LocationKey::local(p).to_token()
    }

    #[test]
    fn tabs_round_trip_with_views_and_the_active_one() {
        let s = Session::from_parts(
            &[
                (tok(Path::new("/a")), "grid".into()),
                (tok(Path::new("/b")), "columns".into()),
                (tok(Path::new("/c")), "nonsense".into()),
            ],
            1,
        );
        let back = Session::from_text(&s.to_text());
        assert_eq!(back, s);
        assert_eq!(back.active, 1);
        assert_eq!(back.tabs[2].view, "rows", "an unknown view reads as rows");
    }

    #[test]
    fn a_non_utf8_folder_name_survives() {
        let raw = std::ffi::OsStr::from_bytes(b"/tmp/caf\xe9");
        let s = Session::from_parts(&[(tok(Path::new(raw)), "rows".into())], 0);
        let back = Session::from_text(&s.to_text());
        assert_eq!(back.tabs[0].key.as_path().unwrap().as_os_str(), raw);
    }

    #[test]
    fn garbage_gives_no_tabs_and_never_panics() {
        for t in [
            "",
            "tab = \"zz:nothex rows\"",
            "active = 99\ntab = \"local:2f61\"",
            &"x".repeat(50_000),
            "tab = \"local:2f612f2e2e rows\"\nactive = -3",
        ] {
            let s = Session::from_text(t);
            assert!(
                s.active < s.tabs.len().max(1),
                "active is always a real tab (or 0)"
            );
        }
        let s = Session::from_text("active = 99\ntab = \"local:2f61 grid\"\n");
        assert_eq!(s.tabs.len(), 1);
        assert_eq!(s.active, 0, "an out-of-range active tab is clamped");
    }

    #[test]
    fn at_most_max_tabs_are_kept() {
        let parts: Vec<(String, String)> = (0..100)
            .map(|i| (tok(Path::new(&format!("/t{i}"))), "rows".into()))
            .collect();
        assert_eq!(Session::from_parts(&parts, 0).tabs.len(), MAX_TABS);
    }

    #[test]
    fn saved_owner_only_and_a_symlink_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("save");
        let f = d.join("gust/tabs.toml");
        let s = Session::from_parts(&[(tok(&d), "grid".into())], 0);
        s.save(&f).unwrap();
        assert_eq!(f.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(Session::load(&f), s);
        // A link at the path is not followed: it could name any file.
        std::fs::write(d.join("other"), s.to_text()).unwrap();
        let l = d.join("link.toml");
        std::os::unix::fs::symlink(d.join("other"), &l).unwrap();
        assert_eq!(Session::load(&l), Session::default());
        assert!(
            std::fs::read_dir(d.join("gust")).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")),
            "no staging file is left behind"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn folders_that_are_gone_are_dropped_and_the_active_tab_follows() {
        let d = tmp("gone");
        std::fs::create_dir_all(d.join("keep")).unwrap();
        std::fs::create_dir_all(d.join("also")).unwrap();
        let s = Session::from_parts(
            &[
                (tok(&d.join("deleted")), "rows".into()),
                (tok(&d.join("keep")), "grid".into()),
                (tok(&d.join("also")), "rows".into()),
            ],
            2,
        );
        let t = s.still_there();
        assert_eq!(t.tabs.len(), 2);
        assert_eq!(t.tabs[t.active].key, LocationKey::local(d.join("also")));
        std::fs::remove_dir_all(&d).ok();
    }
}
