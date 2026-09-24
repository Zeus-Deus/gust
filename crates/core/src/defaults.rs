//! Registering Gust as the default file manager, reversibly.
//!
//! AGENTS.md promises `--default` is reversible, and the plan's §8 requires
//! that "uninstall/rollback preserves later edits". Those two together mean
//! the rollback is the delicate part, not the claim:
//!
//! - what was there before is recorded *before* anything changes, including
//!   the case where nothing was set at all;
//! - every association we did not claim survives byte-for-byte, comments
//!   and ordering included — a user's config file is not ours to rewrite;
//! - if something else changed the association after Gust claimed it, that
//!   newer choice is left alone rather than being stamped over.
//!
//! See `docs/DESIGN.md` §6.2.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The MIME keys a file manager claims.
pub const CLAIMED_KEYS: &[&str] = &[
    "inode/directory",
    "x-directory/normal",
    "x-scheme-handler/file",
];

/// Gust's own desktop entry.
pub const GUST_DESKTOP: &str = "gust.desktop";

/// What was set before Gust claimed these keys.
///
/// `None` for a key means it had no association at all, which rollback must
/// restore as absence — not as some invented fallback.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PreviousDefaults {
    pub entries: BTreeMap<String, Option<String>>,
}

impl PreviousDefaults {
    pub fn to_text(&self) -> String {
        let mut out = String::from(
            "# What was the default before Gust claimed it.\n\
             # Removing this file makes `gust --default off` a no-op.\n",
        );
        for (k, v) in &self.entries {
            match v {
                Some(app) => out.push_str(&format!("{k} = {app}\n")),
                // An explicit marker: "there was nothing here" is a fact to
                // restore, not a missing line to guess about.
                None => out.push_str(&format!("{k} =\n")),
            }
        }
        out
    }

    pub fn from_text(text: &str) -> Self {
        let mut entries = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let key = k.trim().to_string();
            if !CLAIMED_KEYS.contains(&key.as_str()) {
                continue; // never restore a key Gust does not claim
            }
            let val = v.trim();
            entries.insert(
                key,
                if val.is_empty() {
                    None
                } else {
                    Some(val.to_string())
                },
            );
        }
        PreviousDefaults { entries }
    }
}

/// A parsed `mimeapps.list`, preserving everything we did not touch.
///
/// Deliberately line-based rather than a general INI parser: the goal is to
/// change the few lines we claim and leave every other byte — comments,
/// blank lines, ordering, unknown sections — exactly as found.
#[derive(Debug, Clone)]
pub struct MimeApps {
    lines: Vec<String>,
}

impl MimeApps {
    pub fn parse(text: &str) -> Self {
        MimeApps {
            lines: text.lines().map(|l| l.to_string()).collect(),
        }
    }

    pub fn to_text(&self) -> String {
        let mut s = self.lines.join("\n");
        if !s.ends_with('\n') {
            s.push('\n');
        }
        s
    }

    /// The current association for a key in `[Default Applications]`.
    pub fn get(&self, key: &str) -> Option<String> {
        let mut in_defaults = false;
        for line in &self.lines {
            let t = line.trim();
            if t.starts_with('[') {
                in_defaults = t == "[Default Applications]";
                continue;
            }
            if !in_defaults {
                continue;
            }
            if let Some((k, v)) = t.split_once('=') {
                if k.trim() == key {
                    let v = v.trim();
                    return if v.is_empty() {
                        None
                    } else {
                        Some(v.to_string())
                    };
                }
            }
        }
        None
    }

    /// Set (or with `None`, remove) an association, touching nothing else.
    pub fn set(&mut self, key: &str, value: Option<&str>) {
        let mut in_defaults = false;
        let mut defaults_at: Option<usize> = None;
        let mut section_end = self.lines.len();

        for (i, line) in self.lines.iter().enumerate() {
            let t = line.trim();
            if t.starts_with('[') {
                if in_defaults {
                    section_end = i;
                    break;
                }
                in_defaults = t == "[Default Applications]";
                if in_defaults {
                    defaults_at = Some(i);
                }
                continue;
            }
            if in_defaults {
                if let Some((k, _)) = t.split_once('=') {
                    if k.trim() == key {
                        match value {
                            Some(v) => self.lines[i] = format!("{key}={v}"),
                            None => {
                                self.lines.remove(i);
                            }
                        }
                        return;
                    }
                }
            }
        }

        let Some(v) = value else {
            return; // removing something that is not there
        };

        match defaults_at {
            Some(_) => self.lines.insert(section_end, format!("{key}={v}")),
            None => {
                if !self.lines.is_empty() && !self.lines.last().is_some_and(|l| l.is_empty()) {
                    self.lines.push(String::new());
                }
                self.lines.push("[Default Applications]".into());
                self.lines.push(format!("{key}={v}"));
            }
        }
    }
}

/// Where Gust remembers what it replaced.
///
/// Beside Gust's own config, never among the user's XDG defaults: removing
/// Gust must not be able to corrupt unrelated associations.
pub fn state_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("previous-defaults.conf"))
}

fn config_dir() -> Option<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return Some(PathBuf::from(x).join("gust"));
        }
    }
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(h).join(".config").join("gust"))
}

/// The user's `mimeapps.list`.
pub fn mimeapps_path() -> Option<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return Some(PathBuf::from(x).join("mimeapps.list"));
        }
    }
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(h).join(".config").join("mimeapps.list"))
}

/// Decide what `--default on` should write, without touching anything.
///
/// Returns the new file contents and what to remember, so the caller can
/// show it, test it, or write it — the decision is separable from the act.
pub fn plan_claim(current: &MimeApps) -> (MimeApps, PreviousDefaults) {
    let mut next = current.clone();
    let mut prev = PreviousDefaults::default();
    for key in CLAIMED_KEYS {
        prev.entries.insert((*key).to_string(), current.get(key));
        next.set(key, Some(GUST_DESKTOP));
    }
    (next, prev)
}

/// What `--default off` should restore.
///
/// A key whose association is no longer Gust's was changed by something
/// else after we claimed it. That later choice wins: we leave it alone
/// rather than stamping an old value over a newer decision.
pub fn plan_release(current: &MimeApps, prev: &PreviousDefaults) -> (MimeApps, Vec<String>) {
    let mut next = current.clone();
    let mut skipped = Vec::new();
    for (key, old) in &prev.entries {
        match current.get(key).as_deref() {
            Some(GUST_DESKTOP) => next.set(key, old.as_deref()),
            Some(other) => skipped.push(format!("{key} is now {other}")),
            None => {
                // Nothing is set: another tool cleared it. Restoring our
                // remembered value would resurrect a choice nobody made.
                skipped.push(format!("{key} is no longer set by anything"));
            }
        }
    }
    (next, skipped)
}

/// Write a file atomically with owner-only permissions.
pub fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let Some(dir) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(dir)?;
    let stage = dir.join(format!(".gust-default-{}.tmp", std::process::id()));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&stage)?;
    let r = f.write_all(contents.as_bytes()).and_then(|_| f.sync_all());
    drop(f);
    if let Err(e) = r {
        let _ = std::fs::remove_file(&stage);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&stage, path) {
        let _ = std::fs::remove_file(&stage);
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_WORLD: &str = "\
# a comment the user wrote
[Added Associations]
text/plain=nvim.desktop;

[Default Applications]
text/html=chromium.desktop
x-scheme-handler/http=chromium.desktop
inode/directory=org.gnome.Nautilus.desktop
x-scheme-handler/mailto=HEY.desktop
";

    #[test]
    fn claiming_replaces_only_what_gust_claims() {
        let before = MimeApps::parse(REAL_WORLD);
        let (after, _) = plan_claim(&before);
        let text = after.to_text();

        assert_eq!(after.get("inode/directory").as_deref(), Some(GUST_DESKTOP));
        // Everything else is untouched.
        assert_eq!(
            after.get("text/html").as_deref(),
            Some("chromium.desktop"),
            "an unrelated association was changed"
        );
        assert_eq!(
            after.get("x-scheme-handler/mailto").as_deref(),
            Some("HEY.desktop")
        );
        assert!(
            text.contains("# a comment the user wrote"),
            "the user's comment was destroyed"
        );
        assert!(
            text.contains("[Added Associations]") && text.contains("text/plain=nvim.desktop;"),
            "an unrelated section was destroyed"
        );
    }

    #[test]
    fn rollback_restores_exactly_what_was_there() {
        let before = MimeApps::parse(REAL_WORLD);
        let (claimed, prev) = plan_claim(&before);
        let (restored, skipped) = plan_release(&claimed, &prev);

        assert!(skipped.is_empty(), "nothing should have been skipped");
        assert_eq!(
            restored.get("inode/directory").as_deref(),
            Some("org.gnome.Nautilus.desktop"),
            "the previous file manager was not restored"
        );
        // A full round trip must leave the file equivalent to the original.
        assert_eq!(
            restored.get("text/html"),
            before.get("text/html"),
            "an unrelated key drifted across a claim/release cycle"
        );
    }

    #[test]
    fn a_key_that_was_unset_is_restored_as_unset() {
        // "nothing was set" is a fact to restore, not a gap to fill in.
        let before = MimeApps::parse("[Default Applications]\ntext/html=chromium.desktop\n");
        assert_eq!(before.get("inode/directory"), None);

        let (claimed, prev) = plan_claim(&before);
        assert_eq!(
            claimed.get("inode/directory").as_deref(),
            Some(GUST_DESKTOP)
        );

        let (restored, _) = plan_release(&claimed, &prev);
        assert_eq!(
            restored.get("inode/directory"),
            None,
            "rollback invented an association that never existed"
        );
    }

    #[test]
    fn a_later_choice_by_someone_else_is_never_overwritten() {
        // §8: uninstall/rollback preserves later edits.
        let before =
            MimeApps::parse("[Default Applications]\ninode/directory=org.gnome.Nautilus.desktop\n");
        let (claimed, prev) = plan_claim(&before);

        // The user then picks something else entirely.
        let mut later = claimed.clone();
        later.set("inode/directory", Some("thunar.desktop"));

        let (restored, skipped) = plan_release(&later, &prev);
        assert_eq!(
            restored.get("inode/directory").as_deref(),
            Some("thunar.desktop"),
            "rollback stamped an old value over a newer choice"
        );
        assert!(
            skipped.iter().any(|s| s.contains("thunar")),
            "the skip was not reported: {skipped:?}"
        );
    }

    #[test]
    fn a_cleared_association_is_not_resurrected() {
        let before =
            MimeApps::parse("[Default Applications]\ninode/directory=org.gnome.Nautilus.desktop\n");
        let (claimed, prev) = plan_claim(&before);

        // Something removed the association entirely.
        let mut cleared = claimed.clone();
        cleared.set("inode/directory", None);
        assert_eq!(cleared.get("inode/directory"), None);

        let (restored, skipped) = plan_release(&cleared, &prev);
        assert_eq!(
            restored.get("inode/directory"),
            None,
            "rollback resurrected a choice nobody made"
        );
        assert!(!skipped.is_empty());
    }

    #[test]
    fn a_file_with_no_defaults_section_gains_one() {
        let before = MimeApps::parse("[Added Associations]\ntext/plain=nvim.desktop;\n");
        let (after, _) = plan_claim(&before);
        assert_eq!(after.get("inode/directory").as_deref(), Some(GUST_DESKTOP));
        assert!(
            after.to_text().contains("[Added Associations]"),
            "the existing section was lost"
        );
    }

    #[test]
    fn an_empty_or_missing_file_is_handled() {
        let (after, prev) = plan_claim(&MimeApps::parse(""));
        assert_eq!(after.get("inode/directory").as_deref(), Some(GUST_DESKTOP));
        // Every claimed key recorded as previously unset.
        for k in CLAIMED_KEYS {
            assert_eq!(prev.entries.get(*k), Some(&None));
        }
    }

    #[test]
    fn the_state_file_round_trips_including_absence() {
        let mut p = PreviousDefaults::default();
        p.entries
            .insert("inode/directory".into(), Some("nautilus.desktop".into()));
        p.entries.insert("x-directory/normal".into(), None);

        let back = PreviousDefaults::from_text(&p.to_text());
        assert_eq!(back, p, "absence did not survive the state file");
    }

    #[test]
    fn the_state_file_never_restores_a_key_gust_does_not_claim() {
        // A tampered state file must not become a way to rewrite arbitrary
        // associations.
        let text = "inode/directory = gust.desktop\n\
                    x-scheme-handler/https = evil.desktop\n\
                    text/html = evil.desktop\n";
        let p = PreviousDefaults::from_text(text);
        assert!(p.entries.contains_key("inode/directory"));
        assert!(
            !p.entries.contains_key("x-scheme-handler/https")
                && !p.entries.contains_key("text/html"),
            "a state file could rewrite an unrelated association: {:?}",
            p.entries
        );
    }

    #[test]
    fn claiming_twice_does_not_lose_the_original() {
        let before =
            MimeApps::parse("[Default Applications]\ninode/directory=org.gnome.Nautilus.desktop\n");
        let (once, first_prev) = plan_claim(&before);
        let (twice, _second_prev) = plan_claim(&once);

        // The SECOND claim would record gust.desktop as "previous", so the
        // caller must keep the first record. Prove the first still restores.
        let (restored, _) = plan_release(&twice, &first_prev);
        assert_eq!(
            restored.get("inode/directory").as_deref(),
            Some("org.gnome.Nautilus.desktop"),
            "claiming twice lost the real previous default"
        );
    }
}
