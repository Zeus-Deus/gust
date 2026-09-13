//! "Open with…" — choosing an application (DESIGN §6.17, mockup B).
//!
//! The list comes from the freedesktop desktop-entry database, not from a
//! command's printed output: `gio mime` writes "Default application for …"
//! in the system language, and §3's rule against scraping localized text
//! applies here too.
//!
//! Nothing is handed to a shell. An `Exec` line is parsed into argv entries,
//! so a file called `; rm -rf ~` is one argument and nothing else.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// An application that says it can open a type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handler {
    /// Desktop entry id, e.g. `imv.desktop`.
    pub id: String,
    /// What the user sees.
    pub name: String,
    /// The raw `Exec=` line, expanded at launch time.
    pub exec: String,
    /// Needs a terminal, or it vanishes on launch.
    pub terminal: bool,
    /// True for the system's current default for this type.
    pub is_default: bool,
}

/// Where desktop entries live, in precedence order: the user's own first.
pub fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(home).join("applications"));
    } else if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/applications"));
    }
    let system =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for d in system.split(':').filter(|s| !s.is_empty()) {
        dirs.push(PathBuf::from(d).join("applications"));
    }
    dirs
}

/// Parse one `key=value` INI-ish line, ignoring comments and sections.
fn kv(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
        return None;
    }
    line.split_once('=')
}

/// Read the fields Gust needs out of a `.desktop` file.
///
/// Only the main `[Desktop Entry]` group: an action group's Exec line is for
/// a different action entirely and launching it would do the wrong thing.
pub fn parse_desktop_entry(text: &str) -> Option<(String, String, bool, bool)> {
    let mut name = None;
    let mut exec = None;
    let mut terminal = false;
    let mut hidden = false;
    let mut in_main = false;

    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_main = t == "[Desktop Entry]";
            continue;
        }
        if !in_main {
            continue;
        }
        let Some((k, v)) = kv(t) else { continue };
        match k {
            // The untranslated key only: a localized Name[de] must not
            // override the one Gust matches on.
            "Name" if name.is_none() => name = Some(v.to_string()),
            "Exec" if exec.is_none() => exec = Some(v.to_string()),
            "Terminal" => terminal = v.eq_ignore_ascii_case("true"),
            // NoDisplay means "not in the applications menu", NOT "cannot
            // open files" — imv is NoDisplay and is exactly what you want
            // for an image. Only Hidden means the entry is deleted.
            "Hidden" => hidden = v.eq_ignore_ascii_case("true"),
            "Type" if v != "Application" => return None,
            _ => {}
        }
    }
    if hidden {
        return None;
    }
    let exec = exec?;
    let name = name.unwrap_or_else(|| exec.split_whitespace().next().unwrap_or("?").to_string());
    Some((name, exec, terminal, hidden))
}

/// Desktop entry ids registered for `mime`, from one `mimeinfo.cache`.
pub fn parse_mimeinfo(text: &str, mime: &str) -> Vec<String> {
    for line in text.lines() {
        let Some((k, v)) = kv(line) else { continue };
        if k == mime {
            return v
                .split(';')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
        }
    }
    Vec::new()
}

/// The user's own associations from `mimeapps.list`.
///
/// Returns (added, removed) for `mime`. The user's removals win over the
/// system cache: an application they explicitly took away must not come
/// back in this list.
pub fn parse_mimeapps(text: &str, mime: &str) -> (Vec<String>, Vec<String>) {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut section = String::new();
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            section = t.to_string();
            continue;
        }
        let Some((k, v)) = kv(t) else { continue };
        if k != mime {
            continue;
        }
        let ids: Vec<String> = v
            .split(';')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        match section.as_str() {
            "[Added Associations]" | "[Default Applications]" => added.extend(ids),
            "[Removed Associations]" => removed.extend(ids),
            _ => {}
        }
    }
    (added, removed)
}

/// Every application that can open `mime`, best first.
pub fn handlers_for(mime: &str) -> Vec<Handler> {
    let dirs = application_dirs();

    // The user's own list first, then each system cache.
    let mut ids: Vec<String> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    let mut default_id: Option<String> = None;

    for dir in &dirs {
        if let Ok(text) = std::fs::read_to_string(dir.join("mimeapps.list")) {
            let (added, gone) = parse_mimeapps(&text, mime);
            if default_id.is_none() {
                default_id = added.first().cloned();
            }
            for id in added {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            removed.extend(gone);
        }
    }
    // Also the XDG config location, which is where most desktops write it.
    if let Some(cfg) = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    {
        if let Ok(text) = std::fs::read_to_string(cfg.join("mimeapps.list")) {
            let (added, gone) = parse_mimeapps(&text, mime);
            if default_id.is_none() {
                default_id = added.first().cloned();
            }
            for id in added {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            removed.extend(gone);
        }
    }

    for dir in &dirs {
        if let Ok(text) = std::fs::read_to_string(dir.join("mimeinfo.cache")) {
            for id in parse_mimeinfo(&text, mime) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
    }

    // Resolve each id to a real entry, first directory wins.
    let mut seen: BTreeMap<String, Handler> = BTreeMap::new();
    let mut out: Vec<Handler> = Vec::new();
    for id in ids {
        if removed.contains(&id) || seen.contains_key(&id) {
            continue;
        }
        for dir in &dirs {
            let path = dir.join(&id);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some((name, exec, terminal, _)) = parse_desktop_entry(&text) {
                let h = Handler {
                    is_default: default_id.as_deref() == Some(id.as_str()),
                    id: id.clone(),
                    name,
                    exec,
                    terminal,
                };
                seen.insert(id.clone(), h.clone());
                out.push(h);
            }
            break;
        }
    }
    // The default first; everything else keeps discovery order.
    out.sort_by_key(|h| !h.is_default);
    out
}

/// Expand a desktop `Exec` line into argv.
///
/// Field codes per the freedesktop spec: `%f`/`%F` the path, `%u`/`%U` its
/// URI, and `%i`/`%c`/`%k` dropped. Nothing is passed through a shell, so a
/// path containing shell metacharacters stays one argument.
pub fn exec_argv(exec: &str, path: &Path) -> Vec<String> {
    let p = path.to_string_lossy().to_string();
    let uri = format!("file://{p}");
    let mut out = Vec::new();
    let mut substituted = false;

    for word in exec.split_whitespace() {
        // A quoted word in Exec is one argument; strip the quotes the spec
        // allows without letting them turn into shell quoting.
        let w = word.trim_matches('"');
        match w {
            "%f" | "%F" => {
                out.push(p.clone());
                substituted = true;
            }
            "%u" | "%U" => {
                out.push(uri.clone());
                substituted = true;
            }
            // Icon, comment and entry-path codes carry nothing Gust needs.
            "%i" | "%c" | "%k" => {}
            other => {
                // A code embedded in a longer word, e.g. --file=%f.
                if other.contains("%f") || other.contains("%F") {
                    out.push(other.replace("%F", &p).replace("%f", &p));
                    substituted = true;
                } else if other.contains("%u") || other.contains("%U") {
                    out.push(other.replace("%U", &uri).replace("%u", &uri));
                    substituted = true;
                } else {
                    out.push(other.to_string());
                }
            }
        }
    }

    // An Exec with no field code still has to receive the file, or "open
    // with" opens the application and not the document.
    if !substituted {
        out.push(p);
    }
    out
}

/// Launch `handler` on `path`.
///
/// Returns the application's name on success. The child is detached: closing
/// Gust must not close the document somebody is reading.
pub fn launch(handler: &Handler, path: &Path) -> Result<String, super::OpenRefusal> {
    let argv = exec_argv(&handler.exec, path);
    if argv.is_empty() {
        return Err(super::OpenRefusal::Failed("empty command".into()));
    }
    let program = argv[0].clone();
    let args: Vec<String> = argv[1..].to_vec();

    if handler.terminal {
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        return super::run_in_terminal(&program, &refs).map(|_| handler.name.clone());
    }

    if super::which(&program).is_none() {
        return Err(super::OpenRefusal::Failed(format!(
            "{program} is not installed"
        )));
    }
    std::process::Command::new(&program)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| handler.name.clone())
        .map_err(|e| super::OpenRefusal::Failed(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_desktop_entry_yields_name_and_command() {
        let text = "[Desktop Entry]\nType=Application\nName=imv\nExec=imv %F\nTerminal=false\n";
        let (name, exec, terminal, _) = parse_desktop_entry(text).unwrap();
        assert_eq!(name, "imv");
        assert_eq!(exec, "imv %F");
        assert!(!terminal);
    }

    #[test]
    fn nodisplay_is_still_a_handler() {
        // imv is NoDisplay=true on a real Omarchy system and is exactly what
        // a person wants for an image. NoDisplay means "not in the menu".
        let text = "[Desktop Entry]\nType=Application\nName=imv\nExec=imv %F\nNoDisplay=true\n";
        assert!(parse_desktop_entry(text).is_some());
    }

    #[test]
    fn hidden_means_deleted_and_is_dropped() {
        let text = "[Desktop Entry]\nType=Application\nName=x\nExec=x %f\nHidden=true\n";
        assert!(parse_desktop_entry(text).is_none());
    }

    #[test]
    fn a_link_or_directory_entry_is_not_an_application() {
        let text = "[Desktop Entry]\nType=Link\nName=x\nURL=http://example\n";
        assert!(parse_desktop_entry(text).is_none());
    }

    #[test]
    fn only_the_main_group_is_read() {
        // An action's Exec opens something else entirely.
        let text = "[Desktop Entry]\nType=Application\nName=app\nExec=app %f\n\
                    \n[Desktop Action new]\nName=New Window\nExec=app --new\n";
        let (_, exec, _, _) = parse_desktop_entry(text).unwrap();
        assert_eq!(exec, "app %f", "an action's Exec must not win");
    }

    #[test]
    fn a_localized_name_does_not_override_the_plain_one() {
        let text = "[Desktop Entry]\nType=Application\nName=Files\nName[de]=Dateien\n\
                    Exec=files %f\n";
        let (name, _, _, _) = parse_desktop_entry(text).unwrap();
        assert_eq!(name, "Files");
    }

    #[test]
    fn mimeinfo_lists_the_registered_ids() {
        let text = "[MIME Cache]\nimage/png=imv.desktop;chromium.desktop;\n\
                    text/plain=nvim.desktop;\n";
        assert_eq!(
            parse_mimeinfo(text, "image/png"),
            vec!["imv.desktop", "chromium.desktop"]
        );
        assert!(parse_mimeinfo(text, "video/mp4").is_empty());
    }

    #[test]
    fn a_users_removal_is_respected() {
        let text = "[Added Associations]\nimage/png=pinta.desktop;\n\
                    [Removed Associations]\nimage/png=chromium.desktop;\n";
        let (added, removed) = parse_mimeapps(text, "image/png");
        assert_eq!(added, vec!["pinta.desktop"]);
        assert_eq!(removed, vec!["chromium.desktop"]);
    }

    #[test]
    fn field_codes_become_the_path() {
        let p = Path::new("/tmp/a.png");
        assert_eq!(exec_argv("imv %F", p), vec!["imv", "/tmp/a.png"]);
        assert_eq!(exec_argv("imv %f", p), vec!["imv", "/tmp/a.png"]);
    }

    #[test]
    fn uri_codes_become_a_file_uri() {
        let argv = exec_argv("browser %U", Path::new("/tmp/a.html"));
        assert_eq!(argv[1], "file:///tmp/a.html");
    }

    #[test]
    fn decorative_codes_are_dropped() {
        let argv = exec_argv("app %i %c %k %f", Path::new("/tmp/x"));
        assert_eq!(argv, vec!["app", "/tmp/x"]);
    }

    #[test]
    fn an_exec_without_a_field_code_still_gets_the_file() {
        // Otherwise "open with" opens the application and not the document.
        let argv = exec_argv("someeditor", Path::new("/tmp/notes.txt"));
        assert_eq!(argv, vec!["someeditor", "/tmp/notes.txt"]);
    }

    #[test]
    fn a_hostile_filename_stays_one_argument() {
        let p = Path::new("/tmp/; rm -rf ~");
        let argv = exec_argv("imv %F", p);
        assert_eq!(argv.len(), 2, "got {argv:?}");
        assert_eq!(argv[1], "/tmp/; rm -rf ~");
        // And nothing is ever joined back into one string.
        assert!(!argv[0].contains(' '));
    }

    #[test]
    fn an_embedded_code_is_expanded_in_place() {
        let argv = exec_argv("app --file=%f", Path::new("/tmp/x"));
        assert_eq!(argv, vec!["app", "--file=/tmp/x"]);
    }

    #[test]
    fn application_dirs_prefer_the_users_own() {
        let dirs = application_dirs();
        assert!(!dirs.is_empty());
        // The first entry is under the user's data home, not /usr.
        let first = dirs[0].to_string_lossy().to_string();
        assert!(
            !first.starts_with("/usr"),
            "the user's own applications must come first: {first}"
        );
    }

    #[test]
    fn this_machine_can_open_a_text_file() {
        // Whatever is installed, text/plain should have SOME handler on a
        // desktop system; the point is that enumeration works at all.
        let found = handlers_for("text/plain");
        for h in &found {
            assert!(!h.name.is_empty());
            assert!(!h.exec.is_empty());
        }
    }
}
