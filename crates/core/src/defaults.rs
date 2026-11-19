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

/// The app a `mimeapps.list` value selects: the first of a `;`-separated
/// list, which is how GIO writes even a single choice (`gust.desktop;`).
fn first_app(value: &str) -> Option<&str> {
    value.split(';').map(str::trim).find(|app| !app.is_empty())
}

fn is_gust(value: Option<&str>) -> bool {
    value.and_then(first_app) == Some(GUST_DESKTOP)
}

/// Is the value still what Gust wrote: Gust and nothing else? `gust.desktop`
/// and GIO's `gust.desktop;` are; `gust.desktop;thunar.desktop;` is not,
/// because someone has since added their own fallback to the list.
fn is_only_gust(value: &str) -> bool {
    let mut apps = value
        .split(';')
        .map(str::trim)
        .filter(|app| !app.is_empty());
    apps.next() == Some(GUST_DESKTOP) && apps.next().is_none()
}

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
        let found = current.get(key);
        // Already Gust's (set by `xdg-mime` or the desktop's own chooser):
        // there is nothing to claim, and recording it would make rollback
        // "restore" Gust over itself.
        if is_gust(found.as_deref()) {
            continue;
        }
        prev.entries.insert((*key).to_string(), found);
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
            // A record written before Gust knew better than to remember
            // itself: there is no earlier choice in it to put back.
            Some(now) if is_gust(Some(now)) && is_gust(old.as_deref()) => {
                skipped.push(format!("{key} was already gust before it was claimed"));
            }
            Some(now) if is_only_gust(now) => next.set(key, old.as_deref()),
            // Gust still leads, but the list was edited since: putting the
            // old value back would throw away the fallbacks added to it.
            Some(now) if is_gust(Some(now)) => {
                skipped.push(format!("{key} was changed to {now} after it was claimed"));
            }
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

/// Follow symbolic links to the file a write should land in.
///
/// A dotfile manager keeps the real `mimeapps.list` in its own tree and
/// links to it; renaming over the link would quietly detach the two. The
/// last hop may not exist yet.
fn write_target(path: &Path) -> std::io::Result<PathBuf> {
    let mut at = path.to_path_buf();
    // The kernel's own limit on nested links.
    for _ in 0..40 {
        match std::fs::symlink_metadata(&at) {
            Ok(m) if m.file_type().is_symlink() => {
                let to = std::fs::read_link(&at)?;
                at = match at.parent() {
                    Some(dir) => dir.join(to),
                    None => to,
                };
            }
            _ => return Ok(at),
        }
    }
    Err(std::io::Error::other(format!(
        "{} is a loop of links",
        path.display()
    )))
}

/// Replace a file's contents atomically.
///
/// A new file is owner-only; an existing one keeps its mode. A symbolic link
/// is written through to the file it names and stays a link.
pub fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let path = &write_target(path)?;
    let Some(dir) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(dir)?;
    let mode = match std::fs::metadata(path) {
        Ok(m) if m.is_file() => m.permissions().mode() & 0o7777,
        _ => 0o600,
    };
    // Staged beside the real file, so the rename never crosses filesystems.
    let stage = dir.join(format!(".gust-default-{}.tmp", std::process::id()));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&stage)?;
    let r = f
        .write_all(contents.as_bytes())
        // Explicitly, because the mode given to open() is cut by the umask.
        .and_then(|_| f.set_permissions(std::fs::Permissions::from_mode(mode)))
        .and_then(|_| f.sync_all());
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

/// A desktop-specific list that decides the folder handler before
/// `mimeapps.list` is consulted.
#[derive(Debug, Clone, PartialEq)]
pub struct Shadow {
    pub file: PathBuf,
    pub app: String,
}

/// The desktop-specific list that overrides `mimeapps.list` for folders.
///
/// The association lookup reads `<desktop>-mimeapps.list` for each name in
/// `$XDG_CURRENT_DESKTOP` (lowercased, in order) before `mimeapps.list` in
/// the same folder, so an entry there wins however Gust's claim went.
pub fn desktop_shadow(config_home: &Path, desktops: &str) -> Option<Shadow> {
    for desktop in desktops.split(':').filter(|d| !d.is_empty()) {
        let file = config_home.join(format!("{}-mimeapps.list", desktop.to_ascii_lowercase()));
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Some(value) = MimeApps::parse(&text).get("inode/directory") else {
            continue;
        };
        // The first list with an answer decides, whichever way it answers.
        return match first_app(&value) {
            Some(app) if app != GUST_DESKTOP => Some(Shadow {
                file,
                app: app.to_string(),
            }),
            _ => None,
        };
    }
    None
}

/// What `--default on` did.
#[derive(Debug, Clone, PartialEq)]
pub struct Claim {
    /// Gust already handled folders before this call.
    pub already: bool,
    /// What handled folders before, when this call changed it.
    pub was: Option<String>,
    /// A list this desktop reads first, which still sends folders elsewhere.
    pub shadow: Option<Shadow>,
}

impl Claim {
    /// What to tell the user, one line each.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.already {
            out.push("gust is already the default file manager".to_string());
        } else {
            let was = self.was.as_deref().unwrap_or("nothing");
            out.push(format!("gust is now the default file manager (was: {was})"));
        }
        if let Some(shadow) = &self.shadow {
            out.push(format!(
                "warning: folders will still open with {app}, because {file} says so \
                 and this desktop reads that file first. Remove its inode/directory \
                 line to let gust open folders.",
                app = shadow.app,
                file = shadow.file.display()
            ));
        }
        out
    }
}

/// `--default on`: claim the keys, remembering what was there.
///
/// `desktops` is `$XDG_CURRENT_DESKTOP`.
pub fn claim(mime_path: &Path, state: &Path, desktops: &str) -> std::io::Result<Claim> {
    let current = MimeApps::parse(&std::fs::read_to_string(mime_path).unwrap_or_default());
    let shadow = mime_path
        .parent()
        .and_then(|dir| desktop_shadow(dir, desktops));
    let already = is_gust(current.get("inode/directory").as_deref());
    // Nothing to do, and nothing new to remember.
    if already && state.exists() {
        return Ok(Claim {
            already,
            was: None,
            shadow,
        });
    }

    let (next, mut prev) = plan_claim(&current);
    // A key Gust still holds from an earlier claim keeps the record made
    // then: that, not "gust", is what came before.
    if let Ok(text) = std::fs::read_to_string(state) {
        for (key, old) in PreviousDefaults::from_text(&text).entries {
            if is_gust(current.get(&key).as_deref()) && !is_gust(old.as_deref()) {
                prev.entries.entry(key).or_insert(old);
            }
        }
    }
    // Written even when it records nothing, so `off` knows `on` was run.
    write_atomic(state, &prev.to_text())?;
    if next.to_text() != current.to_text() {
        write_atomic(mime_path, &next.to_text())?;
    }
    Ok(Claim {
        already,
        was: prev.entries.get("inode/directory").and_then(|v| v.clone()),
        shadow,
    })
}

/// What `--default off` did. `None` from [`release`] means Gust had never
/// claimed anything.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    /// The claimed keys that were put back.
    pub restored: Vec<String>,
    /// Keys left as they are, each with the reason.
    pub left_alone: Vec<String>,
}

impl Release {
    /// What to tell the user, one line each.
    pub fn lines(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .left_alone
            .iter()
            // A later choice by something else is reported, not undone.
            .map(|s| format!("left alone: {s}"))
            .collect();
        out.push(
            if self.restored.iter().any(|k| k == "inode/directory") {
                "the previous default file manager has been restored"
            } else {
                "the default file manager was not changed"
            }
            .to_string(),
        );
        out
    }
}

/// `--default off`: put back what [`claim`] recorded, and forget it.
pub fn release(mime_path: &Path, state: &Path) -> std::io::Result<Option<Release>> {
    let Ok(text) = std::fs::read_to_string(state) else {
        return Ok(None);
    };
    let current = MimeApps::parse(&std::fs::read_to_string(mime_path).unwrap_or_default());
    let prev = PreviousDefaults::from_text(&text);
    let (next, left_alone) = plan_release(&current, &prev);
    let restored: Vec<String> = prev
        .entries
        .keys()
        .filter(|key| next.get(key) != current.get(key))
        .cloned()
        .collect();
    // A file nothing was restored in is not rewritten at all.
    if !restored.is_empty() {
        write_atomic(mime_path, &next.to_text())?;
    }
    let _ = std::fs::remove_file(state);
    Ok(Some(Release {
        restored,
        left_alone,
    }))
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
    fn a_single_gust_entry_is_restored_however_it_is_spelled() {
        for spelling in ["gust.desktop", "gust.desktop;", " gust.desktop ; "] {
            let before =
                MimeApps::parse("[Default Applications]\ninode/directory=nautilus.desktop\n");
            let (claimed, prev) = plan_claim(&before);
            let mut now = claimed.clone();
            now.set("inode/directory", Some(spelling));
            let (restored, skipped) = plan_release(&now, &prev);
            assert_eq!(
                restored.get("inode/directory").as_deref(),
                Some("nautilus.desktop"),
                "{spelling:?}"
            );
            assert!(
                !skipped.iter().any(|s| s.starts_with("inode/directory")),
                "{spelling:?}: {skipped:?}"
            );
        }
    }

    #[test]
    fn fallbacks_added_after_the_claim_survive_turning_it_off() {
        // §8: rollback preserves later edits. Gust still leads the list,
        // but the user has since added their own fallback to it.
        let before = MimeApps::parse("[Default Applications]\ninode/directory=nautilus.desktop\n");
        let (claimed, prev) = plan_claim(&before);
        let mut later = claimed.clone();
        later.set("inode/directory", Some("gust.desktop;thunar.desktop;"));
        let (restored, skipped) = plan_release(&later, &prev);
        assert_eq!(
            restored.get("inode/directory").as_deref(),
            Some("gust.desktop;thunar.desktop;"),
            "the edited list must be left exactly as it is"
        );
        assert!(
            skipped
                .iter()
                .any(|s| s.starts_with("inode/directory") && s.contains("thunar.desktop")),
            "it must be reported as left alone: {skipped:?}"
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

    /// A private config folder under the system temp dir, removed on drop.
    /// Never the real one: every path below is built from this.
    struct Config(PathBuf);

    impl Config {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("gust-defaults-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Config(dir)
        }
        fn mime(&self) -> PathBuf {
            self.0.join("mimeapps.list")
        }
        fn state(&self) -> PathBuf {
            self.0.join("gust").join("previous-defaults.conf")
        }
        fn mime_text(&self) -> String {
            std::fs::read_to_string(self.mime()).unwrap()
        }
    }

    impl Drop for Config {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const ALREADY_GUST: &str = "\
[Default Applications]
text/html=chromium.desktop
inode/directory=gust.desktop
x-directory/normal=gust.desktop
x-scheme-handler/file=gust.desktop
";

    #[test]
    fn on_then_off_leaves_the_file_exactly_as_it_was_found() {
        let c = Config::new("round-trip");
        std::fs::write(c.mime(), REAL_WORLD).unwrap();

        let claim = claim(&c.mime(), &c.state(), "").unwrap();
        assert!(!claim.already);
        assert_eq!(claim.was.as_deref(), Some("org.gnome.Nautilus.desktop"));
        assert_eq!(
            MimeApps::parse(&c.mime_text())
                .get("inode/directory")
                .as_deref(),
            Some(GUST_DESKTOP)
        );

        let release = release(&c.mime(), &c.state()).unwrap().unwrap();
        assert_eq!(c.mime_text(), REAL_WORLD, "undo did not restore the file");
        assert!(release.left_alone.is_empty());
        assert!(
            release.restored.contains(&"inode/directory".to_string()),
            "the restore was not reported: {release:?}"
        );
        assert_eq!(
            release.lines().last().map(String::as_str),
            Some("the previous default file manager has been restored")
        );
        assert!(!c.state().exists(), "the record outlived the undo");
    }

    #[test]
    fn off_without_a_claim_touches_nothing() {
        let c = Config::new("never-claimed");
        std::fs::write(c.mime(), REAL_WORLD).unwrap();
        assert_eq!(release(&c.mime(), &c.state()).unwrap(), None);
        assert_eq!(c.mime_text(), REAL_WORLD);
    }

    #[test]
    fn gust_set_by_another_tool_is_not_recorded_as_its_own_predecessor() {
        // `xdg-mime default gust.desktop inode/directory`, or Omarchy's own
        // chooser, made Gust the default: there is no earlier app to recall.
        let c = Config::new("already");
        std::fs::write(c.mime(), ALREADY_GUST).unwrap();

        let claim = claim(&c.mime(), &c.state(), "").unwrap();
        assert!(claim.already, "{claim:?}");
        assert_eq!(claim.was, None);
        assert_eq!(c.mime_text(), ALREADY_GUST);
        let recorded = PreviousDefaults::from_text(&std::fs::read_to_string(c.state()).unwrap());
        assert!(
            !recorded
                .entries
                .values()
                .any(|v| v.as_deref() == Some(GUST_DESKTOP)),
            "gust was recorded as what came before gust: {recorded:?}"
        );

        let release = release(&c.mime(), &c.state()).unwrap().unwrap();
        assert_eq!(c.mime_text(), ALREADY_GUST, "off changed what it found");
        assert!(release.restored.is_empty(), "{release:?}");
        assert!(
            !release.lines().iter().any(|l| l.contains("been restored")),
            "off claimed a restore it did not make: {:?}",
            release.lines()
        );
    }

    #[test]
    fn a_record_that_names_gust_as_the_previous_app_restores_nothing() {
        // Written by an earlier Gust that had this bug.
        let c = Config::new("legacy-record");
        std::fs::write(c.mime(), ALREADY_GUST).unwrap();
        std::fs::create_dir_all(c.state().parent().unwrap()).unwrap();
        std::fs::write(c.state(), "inode/directory = gust.desktop\n").unwrap();

        let release = release(&c.mime(), &c.state()).unwrap().unwrap();
        assert_eq!(c.mime_text(), ALREADY_GUST);
        assert!(release.restored.is_empty(), "{release:?}");
        assert!(
            release
                .left_alone
                .iter()
                .any(|s| s.contains("inode/directory")),
            "{release:?}"
        );
        assert!(!release.lines().iter().any(|l| l.contains("been restored")));
    }

    #[test]
    fn claiming_again_after_a_partial_change_keeps_the_original_record() {
        let c = Config::new("reclaim");
        std::fs::write(
            c.mime(),
            "[Default Applications]\n\
             inode/directory=org.gnome.Nautilus.desktop\n\
             x-scheme-handler/file=org.gnome.Nautilus.desktop\n",
        )
        .unwrap();
        claim(&c.mime(), &c.state(), "").unwrap();

        // Something else takes folders back, but not the other keys.
        let mut later = MimeApps::parse(&c.mime_text());
        later.set("inode/directory", Some("thunar.desktop"));
        std::fs::write(c.mime(), later.to_text()).unwrap();

        let again = claim(&c.mime(), &c.state(), "").unwrap();
        assert_eq!(again.was.as_deref(), Some("thunar.desktop"));

        release(&c.mime(), &c.state()).unwrap().unwrap();
        let after = MimeApps::parse(&c.mime_text());
        assert_eq!(
            after.get("inode/directory").as_deref(),
            Some("thunar.desktop")
        );
        assert_eq!(
            after.get("x-scheme-handler/file").as_deref(),
            Some("org.gnome.Nautilus.desktop"),
            "the second claim recorded gust as what came before gust"
        );
    }

    #[test]
    fn off_says_nothing_was_restored_when_every_key_was_left_alone() {
        let c = Config::new("all-skipped");
        std::fs::write(c.mime(), REAL_WORLD).unwrap();
        claim(&c.mime(), &c.state(), "").unwrap();

        // The user then picks another file manager for everything.
        let mut later = MimeApps::parse(&c.mime_text());
        for key in CLAIMED_KEYS {
            later.set(key, Some("thunar.desktop"));
        }
        std::fs::write(c.mime(), later.to_text()).unwrap();
        let found = c.mime_text();

        let release = release(&c.mime(), &c.state()).unwrap().unwrap();
        assert_eq!(c.mime_text(), found);
        assert!(release.restored.is_empty(), "{release:?}");
        assert_eq!(release.left_alone.len(), CLAIMED_KEYS.len());
        let lines = release.lines();
        assert!(
            !lines.iter().any(|l| l.contains("been restored")),
            "off claimed a restore it did not make: {lines:?}"
        );
        assert_eq!(
            lines.last().map(String::as_str),
            Some("the default file manager was not changed")
        );
    }

    #[test]
    fn a_linked_list_is_written_through_and_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        // A dotfile manager keeps the real file elsewhere and links to it.
        let c = Config::new("symlink");
        let dotfiles = c.0.join("dotfiles");
        std::fs::create_dir_all(&dotfiles).unwrap();
        let target = dotfiles.join("mimeapps.list");
        std::fs::write(&target, REAL_WORLD).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        // Relative, as stow writes them.
        std::os::unix::fs::symlink("dotfiles/mimeapps.list", c.mime()).unwrap();

        claim(&c.mime(), &c.state(), "").unwrap();

        assert!(
            std::fs::symlink_metadata(c.mime())
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link was replaced by a regular file"
        );
        let written = std::fs::read_to_string(&target).unwrap();
        assert_eq!(
            MimeApps::parse(&written).get("inode/directory").as_deref(),
            Some(GUST_DESKTOP),
            "the linked file was not updated"
        );
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o7777,
            0o644,
            "the linked file's mode changed"
        );
        assert_eq!(
            std::fs::read_dir(&dotfiles).unwrap().count(),
            1,
            "a staging file was left behind"
        );

        release(&c.mime(), &c.state()).unwrap().unwrap();
        assert!(std::fs::symlink_metadata(c.mime())
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), REAL_WORLD);
    }

    #[test]
    fn a_link_to_a_file_that_does_not_exist_yet_is_written_through() {
        let c = Config::new("dangling");
        std::fs::create_dir_all(c.0.join("dotfiles")).unwrap();
        std::os::unix::fs::symlink(c.0.join("dotfiles/mimeapps.list"), c.mime()).unwrap();

        write_atomic(&c.mime(), "[Default Applications]\n").unwrap();

        assert!(std::fs::symlink_metadata(c.mime())
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_to_string(c.0.join("dotfiles/mimeapps.list")).unwrap(),
            "[Default Applications]\n"
        );
    }

    #[test]
    fn a_new_file_is_private_and_an_existing_one_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let c = Config::new("modes");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o7777;

        write_atomic(&c.state(), "x\n").unwrap();
        assert_eq!(mode(&c.state()), 0o600);

        std::fs::write(c.mime(), "old\n").unwrap();
        std::fs::set_permissions(c.mime(), std::fs::Permissions::from_mode(0o640)).unwrap();
        write_atomic(&c.mime(), "new\n").unwrap();
        assert_eq!(mode(&c.mime()), 0o640);
        assert_eq!(c.mime_text(), "new\n");
    }

    #[test]
    fn a_desktop_specific_list_that_overrides_the_claim_is_named() {
        let c = Config::new("shadow");
        std::fs::write(c.mime(), REAL_WORLD).unwrap();
        let theirs = c.0.join("hyprland-mimeapps.list");
        std::fs::write(
            &theirs,
            "[Default Applications]\ninode/directory=org.gnome.Nautilus.desktop;\n",
        )
        .unwrap();

        let claim = claim(&c.mime(), &c.state(), "Hyprland").unwrap();
        assert_eq!(
            claim.shadow,
            Some(Shadow {
                file: theirs.clone(),
                app: "org.gnome.Nautilus.desktop".into(),
            })
        );
        let lines = claim.lines();
        assert!(
            lines.iter().any(|l| l.starts_with("warning:")
                && l.contains(&theirs.display().to_string())
                && l.contains("org.gnome.Nautilus.desktop")),
            "the overriding file was not named: {lines:?}"
        );
        // Their file is theirs: reported, never edited.
        assert!(std::fs::read_to_string(&theirs)
            .unwrap()
            .contains("org.gnome.Nautilus.desktop;"));

        // Asking again still says so rather than "already the default".
        let again = claim_again(&c);
        assert!(again.already);
        assert!(again.shadow.is_some());
    }

    fn claim_again(c: &Config) -> Claim {
        claim(&c.mime(), &c.state(), "Hyprland").unwrap()
    }

    #[test]
    fn a_desktop_specific_list_is_only_a_problem_when_it_disagrees() {
        let c = Config::new("no-shadow");
        std::fs::write(c.mime(), REAL_WORLD).unwrap();
        // Another desktop's list, one with no folder handler, and one that
        // already names Gust: none of them override the claim here.
        std::fs::write(
            c.0.join("gnome-mimeapps.list"),
            "[Default Applications]\ninode/directory=org.gnome.Nautilus.desktop\n",
        )
        .unwrap();
        std::fs::write(
            c.0.join("hyprland-mimeapps.list"),
            "[Default Applications]\ntext/html=chromium.desktop\n",
        )
        .unwrap();
        std::fs::write(
            c.0.join("wlroots-mimeapps.list"),
            "[Default Applications]\ninode/directory=gust.desktop;\n",
        )
        .unwrap();
        assert_eq!(desktop_shadow(&c.0, "Hyprland:wlroots"), None);
        assert_eq!(desktop_shadow(&c.0, ""), None);
        // The first desktop named wins, as in the lookup itself.
        assert!(desktop_shadow(&c.0, "GNOME:wlroots").is_some());

        let claim = claim(&c.mime(), &c.state(), "Hyprland:wlroots").unwrap();
        assert_eq!(claim.shadow, None);
        assert!(!claim.lines().iter().any(|l| l.starts_with("warning:")));
    }
}
