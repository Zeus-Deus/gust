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
use std::ffi::OsString;
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

/// One piece of an `Exec` argument: literal text or a `%x` field code.
enum Piece {
    Text(String),
    Code(char),
}

/// Split an `Exec` value into arguments, per the desktop-entry spec.
///
/// Arguments are separated by spaces unless double-quoted; inside quotes a
/// backslash escapes `"`, `` ` ``, `$` and `\`. `%%` is a literal percent and
/// any other `%x` is a field code, left for the caller to expand. This is a
/// tokenizer, not a shell: nothing it produces is ever evaluated.
fn exec_args(exec: &str) -> Vec<Vec<Piece>> {
    // The value is a desktop-entry string first, with its own escapes.
    let mut value = String::with_capacity(exec.len());
    let mut raw = exec.chars();
    while let Some(c) = raw.next() {
        if c != '\\' {
            value.push(c);
            continue;
        }
        match raw.next() {
            Some('s') => value.push(' '),
            Some('n') => value.push('\n'),
            Some('t') => value.push('\t'),
            Some('r') => value.push('\r'),
            Some('\\') => value.push('\\'),
            Some(other) => {
                value.push('\\');
                value.push(other);
            }
            None => value.push('\\'),
        }
    }

    fn text(arg: &mut Vec<Piece>, c: char) {
        match arg.last_mut() {
            Some(Piece::Text(t)) => t.push(c),
            _ => arg.push(Piece::Text(c.to_string())),
        }
    }

    let mut args = Vec::new();
    // None between arguments, so `""` is still an (empty) argument.
    let mut current: Option<Vec<Piece>> = None;
    let mut quoted = false;
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' {
            quoted = !quoted;
            current.get_or_insert_with(Vec::new);
            continue;
        }
        if !quoted && c.is_whitespace() {
            args.extend(current.take());
            continue;
        }
        let arg = current.get_or_insert_with(Vec::new);
        match c {
            '\\' if quoted => match chars.peek() {
                Some(&next) if matches!(next, '"' | '`' | '$' | '\\') => {
                    chars.next();
                    text(arg, next);
                }
                _ => text(arg, '\\'),
            },
            '%' => match chars.next() {
                Some('%') => text(arg, '%'),
                Some(code) => arg.push(Piece::Code(code)),
                None => {}
            },
            other => text(arg, other),
        }
    }
    args.extend(current);
    args
}

/// What a file field code stands for: the path, or its `file://` URI.
fn file_arg(code: char, path: &Path) -> OsString {
    if matches!(code, 'u' | 'U') {
        // Percent-encoded from the raw bytes: a `#` or a space in a name
        // would otherwise cut the URI short at the receiving end.
        crate::clipboard::path_to_uri(path).into()
    } else {
        path.as_os_str().to_owned()
    }
}

/// Expand every argument of `exec` over `paths`; also says whether any file
/// code was present.
fn expand(exec: &str, paths: &[impl AsRef<Path>]) -> (Vec<OsString>, bool) {
    let mut out = Vec::new();
    let mut substituted = false;
    for arg in exec_args(exec) {
        // A list code standing alone becomes one argument per file.
        if let [Piece::Code(code @ ('F' | 'U'))] = arg[..] {
            substituted = true;
            out.extend(paths.iter().map(|p| file_arg(code, p.as_ref())));
            continue;
        }
        let mut word = OsString::new();
        // An argument made only of dropped codes disappears with them.
        let mut kept = arg.is_empty();
        for piece in &arg {
            match piece {
                Piece::Text(t) => {
                    word.push(t);
                    kept = true;
                }
                // A code embedded in a longer word, e.g. --file=%f.
                Piece::Code(code @ ('f' | 'F' | 'u' | 'U')) => {
                    substituted = true;
                    if let Some(first) = paths.first() {
                        word.push(file_arg(*code, first.as_ref()));
                        kept = true;
                    }
                }
                // Icon, caption and entry-path codes carry nothing Gust
                // needs; the deprecated ones (%d %D %n %N %v %m) and
                // anything unknown are removed, as the spec requires.
                Piece::Code(_) => {}
            }
        }
        if kept {
            out.push(word);
        }
    }
    (out, substituted)
}

/// Expand a desktop `Exec` line into argv.
///
/// Field codes per the freedesktop spec: `%f`/`%F` the path, `%u`/`%U` its
/// percent-encoded URI, `%%` a percent sign, and `%i`/`%c`/`%k` and the
/// deprecated codes dropped. Nothing is passed through a shell, so a path
/// containing shell metacharacters stays one argument, and the path is never
/// re-encoded, so a name that is not UTF-8 still names the same file.
pub fn exec_argv(exec: &str, path: &Path) -> Vec<OsString> {
    let (mut out, substituted) = expand(exec, &[path]);

    // An Exec with no field code still has to receive the file, or "open
    // with" opens the application and not the document.
    if !substituted {
        out.push(path.as_os_str().to_owned());
    }
    out
}

/// Can this handler take several files in one launch?
///
/// The freedesktop spec is explicit: `%F` and `%U` accept a list, `%f` and
/// `%u` are single-file and the launcher must invoke the program once per
/// file. Guessing wrong the other way is worse than an extra window —
/// passing five paths to a `%f` program opens the first and silently drops
/// four, so the files *look* opened.
pub fn takes_many(exec: &str) -> bool {
    exec_args(exec)
        .iter()
        .flatten()
        .any(|piece| matches!(piece, Piece::Code('F' | 'U')))
}

/// Expand an `Exec` line for several files at once.
///
/// Only valid when [`takes_many`] is true; the caller launches once per file
/// otherwise. A single-file code here would only ever carry the first path.
pub fn exec_argv_many(exec: &str, paths: &[PathBuf]) -> Vec<OsString> {
    expand(exec, paths).0
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
    // The program comes from the Exec text, so it is always UTF-8; only the
    // file arguments have to stay raw.
    let program = argv[0].to_string_lossy().into_owned();
    let args = &argv[1..];

    if handler.terminal {
        return super::run_in_terminal(&program, args).map(|_| handler.name.clone());
    }

    if super::which(&program).is_none() {
        return Err(super::OpenRefusal::Failed(format!(
            "{program} is not installed"
        )));
    }
    std::process::Command::new(&program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| handler.name.clone())
        .map_err(|e| super::OpenRefusal::Failed(e.to_string()))
}

/// How many files one activation may open at once.
///
/// Past this the question is no longer "open these" but "did you mean to":
/// a hundred viewer windows is a denial of service the user performed on
/// themselves.
pub const MAX_OPEN_AT_ONCE: usize = 50;

/// What a multi-file open did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiOpen {
    /// One launch with every path.
    Together {
        app: String,
        count: usize,
    },
    /// One launch per file, because the handler is single-file.
    Separately {
        app: String,
        count: usize,
    },
    Refused(String),
}

impl MultiOpen {
    pub fn message(&self) -> String {
        match self {
            MultiOpen::Together { app, count } => {
                format!("Opened {count} files with {app}")
            }
            MultiOpen::Separately { app, count } => {
                format!("Opened {count} files with {app}, one window each")
            }
            MultiOpen::Refused(why) => why.clone(),
        }
    }
}

/// Open several files with one handler, respecting whether it takes a list.
pub fn launch_many(handler: &Handler, paths: &[PathBuf]) -> MultiOpen {
    if paths.is_empty() {
        return MultiOpen::Refused("nothing selected".into());
    }
    if paths.len() > MAX_OPEN_AT_ONCE {
        return MultiOpen::Refused(format!(
            "{} files is more than Gust will open at once — select up to {}.",
            paths.len(),
            MAX_OPEN_AT_ONCE
        ));
    }

    if takes_many(&handler.exec) {
        let argv = exec_argv_many(&handler.exec, paths);
        if argv.is_empty() {
            return MultiOpen::Refused("empty command".into());
        }
        let program = argv[0].to_string_lossy().into_owned();
        if super::which(&program).is_none() {
            return MultiOpen::Refused(format!("{program} is not installed"));
        }
        return match std::process::Command::new(&program)
            .args(&argv[1..])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => MultiOpen::Together {
                app: handler.name.clone(),
                count: paths.len(),
            },
            Err(e) => MultiOpen::Refused(e.to_string()),
        };
    }

    // Single-file handler: once per file, as the spec requires.
    let mut opened = 0usize;
    for p in paths {
        if launch(handler, p).is_ok() {
            opened += 1;
        }
    }
    if opened == 0 {
        return MultiOpen::Refused(format!("{} could not be started", handler.name));
    }
    MultiOpen::Separately {
        app: handler.name.clone(),
        count: opened,
    }
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
        assert_eq!(argv[0], "imv");
    }

    #[test]
    fn an_embedded_code_is_expanded_in_place() {
        let argv = exec_argv("app --file=%f", Path::new("/tmp/x"));
        assert_eq!(argv, vec!["app", "--file=/tmp/x"]);
    }

    #[test]
    fn a_quoted_program_path_with_a_space_is_one_argument() {
        let argv = exec_argv(r#""/opt/My App/app" --new-window %U"#, Path::new("/tmp/a"));
        assert_eq!(
            argv,
            vec!["/opt/My App/app", "--new-window", "file:///tmp/a"]
        );
    }

    #[test]
    fn quoted_escapes_are_undone_without_a_shell() {
        // Inside double quotes a backslash escapes " ` $ and \ only; the
        // result is still plain argv, so $1 and `x` are never evaluated.
        let argv = exec_argv(
            r#"app "say \"hi\" \$1 \`x\` C:\\\\dir" %f"#,
            Path::new("/tmp/x"),
        );
        assert_eq!(argv, vec!["app", r#"say "hi" $1 `x` C:\dir"#, "/tmp/x"]);
    }

    #[test]
    fn a_doubled_percent_is_a_literal_percent() {
        let argv = exec_argv("app --zoom=100%% %f", Path::new("/tmp/x"));
        assert_eq!(argv, vec!["app", "--zoom=100%", "/tmp/x"]);
        assert!(!takes_many("app %%F %f"), "%%F is the text %F, not a list");
    }

    #[test]
    fn deprecated_codes_are_dropped() {
        let argv = exec_argv("app %d %D %n %N %v %m %f", Path::new("/tmp/x"));
        assert_eq!(argv, vec!["app", "/tmp/x"]);
    }

    #[test]
    fn a_uri_is_percent_encoded() {
        // An unencoded # starts a fragment: the browser would open "Report ".
        let p = PathBuf::from("/tmp/Report #1.html");
        let one = exec_argv("browser %u", &p);
        assert_eq!(one, vec!["browser", "file:///tmp/Report%20%231.html"]);
        let many = exec_argv_many("browser %U", &[p, PathBuf::from("/tmp/100%.html")]);
        assert_eq!(
            many,
            vec![
                "browser",
                "file:///tmp/Report%20%231.html",
                "file:///tmp/100%25.html"
            ]
        );
    }

    #[test]
    fn a_name_that_is_not_utf8_reaches_the_program_unchanged() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let raw: &[u8] = b"/tmp/caf\xe9.txt";
        let p = PathBuf::from(OsStr::from_bytes(raw));

        let argv = exec_argv("ed %f", &p);
        assert_eq!(OsStr::new(&argv[1]).as_bytes(), raw);
        let argv = exec_argv("ed --file=%f", &p);
        assert_eq!(OsStr::new(&argv[1]).as_bytes(), b"--file=/tmp/caf\xe9.txt");
        let argv = exec_argv_many("ed %F", std::slice::from_ref(&p));
        assert_eq!(OsStr::new(&argv[1]).as_bytes(), raw);
        let argv = exec_argv("browser %u", &p);
        assert_eq!(argv[1], "file:///tmp/caf%E9.txt");
    }

    #[test]
    fn a_hostile_filename_stays_one_argument_inside_quotes_too() {
        let p = Path::new("/tmp/a\" ; rm -rf ~ \"$(x)`y` %f %%");
        for exec in ["imv %f", "imv \"%f\"", "imv %F"] {
            let argv = exec_argv(exec, p);
            assert_eq!(argv.len(), 2, "{exec}: {argv:?}");
            assert_eq!(OsString::from(&argv[1]), p.as_os_str(), "{exec}");
        }
    }

    #[test]
    fn list_codes_are_distinguished_from_single_file_codes() {
        // The spec's distinction, and the reason it matters: passing five
        // paths to a %f program opens the first and drops four silently.
        assert!(takes_many("imv %F"));
        assert!(takes_many("browser %U"));
        assert!(!takes_many("xournalpp-wrapper %f"));
        assert!(!takes_many("editor %u"));
        assert!(!takes_many("someapp"), "no code means assume single-file");
    }

    #[test]
    fn a_list_code_expands_to_every_path() {
        let paths = vec![
            PathBuf::from("/tmp/a.png"),
            PathBuf::from("/tmp/b.png"),
            PathBuf::from("/tmp/c.png"),
        ];
        let argv = exec_argv_many("imv %F", &paths);
        assert_eq!(argv.len(), 4, "got {argv:?}");
        assert_eq!(argv[0], "imv");
        assert_eq!(argv[3], "/tmp/c.png");
    }

    #[test]
    fn a_uri_list_code_expands_to_uris() {
        let paths = vec![PathBuf::from("/tmp/a.html"), PathBuf::from("/tmp/b.html")];
        let argv = exec_argv_many("browser %U", &paths);
        assert_eq!(argv[1], "file:///tmp/a.html");
        assert_eq!(argv[2], "file:///tmp/b.html");
    }

    #[test]
    fn hostile_names_stay_separate_arguments_in_a_list() {
        let paths = vec![
            PathBuf::from("/tmp/; rm -rf ~"),
            PathBuf::from("/tmp/ok.png"),
        ];
        let argv = exec_argv_many("imv %F", &paths);
        assert_eq!(argv.len(), 3);
        assert_eq!(argv[1], "/tmp/; rm -rf ~");
    }

    #[test]
    fn too_many_files_is_refused_with_the_number() {
        let h = Handler {
            id: "imv.desktop".into(),
            name: "imv".into(),
            exec: "imv %F".into(),
            terminal: false,
            is_default: true,
        };
        let many: Vec<PathBuf> = (0..MAX_OPEN_AT_ONCE + 1)
            .map(|i| PathBuf::from(format!("/tmp/{i}.png")))
            .collect();
        match launch_many(&h, &many) {
            MultiOpen::Refused(why) => {
                assert!(why.contains(&(MAX_OPEN_AT_ONCE + 1).to_string()));
                assert!(why.contains(&MAX_OPEN_AT_ONCE.to_string()));
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_selection_opens_nothing() {
        let h = Handler {
            id: "x.desktop".into(),
            name: "x".into(),
            exec: "x %F".into(),
            terminal: false,
            is_default: false,
        };
        assert!(matches!(launch_many(&h, &[]), MultiOpen::Refused(_)));
    }

    #[test]
    fn messages_say_which_way_it_opened() {
        assert!(MultiOpen::Together {
            app: "imv".into(),
            count: 5
        }
        .message()
        .contains("5 files with imv"));
        assert!(MultiOpen::Separately {
            app: "ed".into(),
            count: 3
        }
        .message()
        .contains("one window each"));
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
