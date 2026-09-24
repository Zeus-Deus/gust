//! Omarchy key bindings for Gust (opt-in, reversible).
//!
//! Omarchy binds Super+Shift+F to `omarchy-launch-nautilus`. Its documented
//! override is `hl.unbind` + `o.bind` in `~/.config/hypr/bindings.lua`, which
//! is loaded after the defaults. Gust writes one marked block there on
//! request and removes exactly that block on request. Nothing else in the
//! user's file is ever changed.

use std::path::{Path, PathBuf};

pub const BLOCK_BEGIN: &str = "-- >>> gust: file manager keys (remove with: gust --bind-keys off)";
pub const BLOCK_END: &str = "-- <<< gust";

/// A string that is safe inside a Lua double-quoted string AND as one shell
/// word: single-quote for the shell, then escape for Lua.
fn lua_shell_word(s: &str) -> String {
    let sh = format!("'{}'", s.replace('\'', r"'\''"));
    sh.replace('\\', r"\\").replace('"', r#"\""#)
}

fn block(exe: &str) -> String {
    let g = lua_shell_word(exe);
    format!(
        "{BLOCK_BEGIN}\n\
         hl.unbind(\"SUPER + SHIFT + F\")\n\
         o.bind(\"SUPER + SHIFT + F\", \"File manager\", \"uwsm-app -- {g}\")\n\
         hl.unbind(\"SUPER + ALT + SHIFT + F\")\n\
         o.bind(\"SUPER + ALT + SHIFT + F\", \"File manager (cwd)\", \"uwsm-app -- {g} \\\"$(omarchy-cmd-terminal-cwd)\\\"\")\n\
         {BLOCK_END}\n"
    )
}

pub fn has_block(text: &str) -> bool {
    text.lines().any(|l| l == BLOCK_BEGIN)
}

pub fn add_block(text: &str, exe: &str) -> String {
    if has_block(text) {
        return text.to_string();
    }
    let mut out = text.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&block(exe));
    out
}

pub fn remove_block(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end_matches('\n');
        if bare == BLOCK_BEGIN {
            inside = true;
            continue;
        }
        if inside {
            if bare == BLOCK_END {
                inside = false;
            }
            continue;
        }
        out.push_str(line);
    }
    out
}

/// `~/.config/hypr/bindings.lua`, following XDG_CONFIG_HOME.
pub fn bindings_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("hypr/bindings.lua"))
}

/// Rewrite the file atomically, keeping its permissions.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let dir = path.parent().ok_or_else(|| std::io::Error::other("no parent folder"))?;
    let tmp = dir.join(format!(".bindings.lua.gust-{}", std::process::id()));
    std::fs::write(&tmp, text)?;
    if let Ok(md) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, md.permissions());
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}
