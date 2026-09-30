//! Custom actions: the user's own commands in the right-click menu and the
//! palette (Strata's most-requested feature; Flea has "user scripts").
//!
//! One file per action in `$XDG_CONFIG_HOME/gust/actions/*.toml`:
//!
//! ```toml
//! name = "Convert to PNG"
//! command = ["magick", "{file}", "{dir}/{stem}.png"]
//! # optional:
//! for = "files"          # "files" | "folders" | "any"   (default "any")
//! extensions = ["jpg", "jpeg", "webp"]   # only offered for these
//! many = "each"          # "each": run once per item; "once": {files} expands to all
//! ```
//!
//! Safety rules, all tested:
//! - `command` is an argv array and is executed directly, never through a
//!   shell: a filename like `$(rm -rf ~)` is one literal argument.
//! - Placeholders expand only INSIDE an argument; a path never splits into
//!   two arguments and never becomes an option (a leading `-` in a name is
//!   passed as `./-name` when it would be the whole argument).
//! - An action file must be owned by the user and not group/world-writable,
//!   or it is ignored: another account must not be able to plant a command.
//! - Only files from the user's own config directory are read.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applies {
    Files,
    Folders,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Many {
    /// Run once per selected item.
    Each,
    /// Run once; `{files}` expands to every selected path (one arg each).
    Once,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub id: String,
    pub name: String,
    pub command: Vec<String>,
    pub applies: Applies,
    pub extensions: Vec<String>,
    pub many: Many,
}

/// One selected item, as the matcher and expander see it.
#[derive(Debug, Clone)]
pub struct Target {
    pub path: PathBuf,
    pub is_dir: bool,
}

/// Parse one action file. `id` is the file stem. Errors are sentences.
pub fn parse(id: &str, text: &str) -> Result<Action, String> {
    let v: toml::Value = toml::from_str(text).map_err(|e| format!("{id}: not valid TOML ({e})"))?;
    let name = v
        .get("name")
        .and_then(|x| x.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("{id}: needs a name"))?
        .to_string();
    let command: Vec<String> = v
        .get("command")
        .and_then(|x| x.as_array())
        .ok_or_else(|| format!("{id}: command must be a list, e.g. [\"prog\", \"{{file}}\"]"))?
        .iter()
        .map(|a| a.as_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| format!("{id}: every command part must be text"))?;
    if command.is_empty() || command[0].trim().is_empty() {
        return Err(format!("{id}: command is empty"));
    }
    if command[0].contains('{') {
        return Err(format!("{id}: the program itself cannot be a placeholder"));
    }
    let applies = match v.get("for").and_then(|x| x.as_str()).unwrap_or("any") {
        "files" => Applies::Files,
        "folders" => Applies::Folders,
        "any" => Applies::Any,
        other => {
            return Err(format!(
                "{id}: for = \"{other}\" is not files, folders or any"
            ))
        }
    };
    let many = match v.get("many").and_then(|x| x.as_str()).unwrap_or("each") {
        "each" => Many::Each,
        "once" => Many::Once,
        other => return Err(format!("{id}: many = \"{other}\" is not each or once")),
    };
    let extensions = v
        .get("extensions")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|e| e.as_str())
                .map(|e| e.trim_start_matches('.').to_lowercase())
                .collect()
        })
        .unwrap_or_default();
    Ok(Action {
        id: id.to_string(),
        name,
        command,
        applies,
        extensions,
        many,
    })
}

fn ext_of(p: &Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

impl Action {
    /// Is this action offered for this selection? Every item must fit.
    pub fn applies_to(&self, targets: &[Target]) -> bool {
        if targets.is_empty() {
            return false;
        }
        targets.iter().all(|t| {
            let kind_ok = match self.applies {
                Applies::Files => !t.is_dir,
                Applies::Folders => t.is_dir,
                Applies::Any => true,
            };
            let ext_ok = self.extensions.is_empty()
                || (!t.is_dir && self.extensions.contains(&ext_of(&t.path)));
            kind_ok && ext_ok
        })
    }

    /// The argv lists to run, one per invocation.
    pub fn argvs(&self, targets: &[Target]) -> Vec<Vec<String>> {
        match self.many {
            Many::Each => targets
                .iter()
                .map(|t| expand(&self.command, std::slice::from_ref(t)))
                .collect(),
            Many::Once => vec![expand(&self.command, targets)],
        }
    }
}

/// A path as an argument that can never be read as an option.
fn safe_arg(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    if s.starts_with('-') {
        format!("./{s}")
    } else {
        s
    }
}

fn expand(command: &[String], targets: &[Target]) -> Vec<String> {
    let first = targets.first();
    let mut out = Vec::with_capacity(command.len() + targets.len());
    for (i, part) in command.iter().enumerate() {
        if i > 0 && part == "{files}" {
            out.extend(targets.iter().map(|t| safe_arg(&t.path)));
            continue;
        }
        if i == 0 {
            out.push(part.clone());
            continue;
        }
        let Some(t) = first else {
            out.push(part.clone());
            continue;
        };
        let file = t.path.to_string_lossy().into_owned();
        let dir = t
            .path
            .parent()
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = t
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stem = t
            .path
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if part == "{file}" {
            // Whole-argument path: guard against option injection.
            out.push(safe_arg(&t.path));
            continue;
        }
        let s = part
            .replace("{file}", &file)
            .replace("{dir}", &dir)
            .replace("{name}", &name)
            .replace("{stem}", &stem)
            .replace("{ext}", &ext_of(&t.path));
        out.push(s);
    }
    out
}

/// Where action files live.
pub fn actions_dir() -> Option<PathBuf> {
    let cfg = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(cfg.join("gust/actions"))
}

/// Would another account be able to change this file? Then it is ignored.
fn trusted(md: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    let me = rustix::process::getuid().as_raw();
    md.is_file() && md.uid() == me && md.mode() & 0o022 == 0
}

/// Load every action, sorted by name. Returns (actions, problems): a broken
/// or untrusted file is reported, never fatal.
pub fn load(dir: &Path) -> (Vec<Action>, Vec<String>) {
    let mut acts = Vec::new();
    let mut problems = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return (acts, problems);
    };
    let mut files: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    for p in files.into_iter().take(200) {
        let id = p
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        match p.symlink_metadata() {
            Ok(md) if trusted(&md) => {}
            _ => {
                problems.push(format!(
                    "{id}: ignored, the file must be yours and not writable by others"
                ));
                continue;
            }
        }
        match std::fs::read_to_string(&p) {
            Ok(t) if t.len() <= 64 * 1024 => match parse(&id, &t) {
                Ok(a) => acts.push(a),
                Err(e) => problems.push(e),
            },
            _ => problems.push(format!("{id}: could not be read")),
        }
    }
    acts.sort_by_key(|a| a.name.to_lowercase());
    (acts, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(p: &str, d: bool) -> Target {
        Target {
            path: PathBuf::from(p),
            is_dir: d,
        }
    }

    #[test]
    fn a_hostile_filename_stays_one_literal_argument() {
        let a = parse(
            "x",
            r#"name = "Show"
command = ["echo", "{file}"]"#,
        )
        .unwrap();
        let argv = a.argvs(&[t("/tmp/$(rm -rf ~); x.txt", false)]);
        assert_eq!(
            argv,
            vec![vec![
                "echo".to_string(),
                "/tmp/$(rm -rf ~); x.txt".to_string()
            ]]
        );
    }

    #[test]
    fn a_name_starting_with_a_dash_is_never_an_option() {
        let a = parse(
            "x",
            r#"name = "Rm"
command = ["ls", "{file}"]"#,
        )
        .unwrap();
        assert_eq!(a.argvs(&[t("-rf", false)])[0][1], "./-rf");
    }

    #[test]
    fn placeholders_expand_inside_an_argument() {
        let a = parse(
            "c",
            r#"name = "Convert"
command = ["magick", "{file}", "{dir}/{stem}.png"]"#,
        )
        .unwrap();
        assert_eq!(
            a.argvs(&[t("/p/My Photo.jpg", false)])[0],
            vec!["magick", "/p/My Photo.jpg", "/p/My Photo.png"]
        );
    }

    #[test]
    fn once_passes_every_file_as_its_own_argument() {
        let a = parse(
            "z",
            r#"name = "Zip"
command = ["zip", "-r", "out.zip", "{files}"]
many = "once""#,
        )
        .unwrap();
        let v = a.argvs(&[t("/a b", false), t("/c", true)]);
        assert_eq!(v, vec![vec!["zip", "-r", "out.zip", "/a b", "/c"]]);
    }

    #[test]
    fn each_runs_once_per_item() {
        let a = parse(
            "e",
            r#"name = "E"
command = ["touch", "{file}"]"#,
        )
        .unwrap();
        assert_eq!(a.argvs(&[t("/a", false), t("/b", false)]).len(), 2);
    }

    #[test]
    fn filters_by_kind_and_extension() {
        let a = parse(
            "j",
            r#"name = "J"
command = ["x", "{file}"]
for = "files"
extensions = [".JPG", "png"]"#,
        )
        .unwrap();
        assert!(a.applies_to(&[t("/a.jpg", false), t("/b.PNG", false)]));
        assert!(!a.applies_to(&[t("/a.jpg", false), t("/c.txt", false)]));
        assert!(!a.applies_to(&[t("/d", true)]));
        assert!(!a.applies_to(&[]));
    }

    #[test]
    fn bad_files_explain_themselves() {
        assert!(parse("a", "name = \"x\"\ncommand = \"rm -rf\"")
            .unwrap_err()
            .contains("must be a list"));
        assert!(parse("b", "command = [\"x\"]")
            .unwrap_err()
            .contains("needs a name"));
        assert!(parse("c", "name = \"x\"\ncommand = [\"{file}\"]")
            .unwrap_err()
            .contains("placeholder"));
        assert!(parse("d", "name = \"x\"\ncommand = [\"x\"]\nfor = \"all\"").is_err());
        assert!(parse("e", "not toml [").is_err());
    }

    #[test]
    fn a_group_writable_action_file_is_ignored() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("gust-actions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("good.toml"), "name = \"Good\"\ncommand = [\"true\"]").unwrap();
        std::fs::write(d.join("bad.toml"), "name = \"Bad\"\ncommand = [\"true\"]").unwrap();
        std::fs::set_permissions(d.join("good.toml"), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        std::fs::set_permissions(d.join("bad.toml"), std::fs::Permissions::from_mode(0o666))
            .unwrap();
        let (acts, problems) = load(&d);
        assert_eq!(
            acts.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            vec!["Good"]
        );
        assert!(problems
            .iter()
            .any(|p| p.contains("bad") && p.contains("not writable")));
        std::fs::remove_dir_all(&d).ok();
    }
}
