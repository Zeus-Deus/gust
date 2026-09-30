//! Choosing Gust as the file-chooser portal (other apps' Open/Save dialogs).
//!
//! xdg-desktop-portal reads `$XDG_CONFIG_HOME/xdg-desktop-portal/
//! <desktop>-portals.conf` before the system one. Gust changes one key in
//! the `[preferred]` section of the user's copy and records what was there,
//! so opting out restores it exactly. Any other line is left as it is.

use std::path::PathBuf;

pub const CHOOSER_KEY: &str = "org.freedesktop.impl.portal.FileChooser";
const GUST: &str = "gust";

fn set_key(text: &str, value: Option<&str>) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_pref = false;
    let mut have_pref = false;
    let mut done = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            if in_pref && !done {
                if let Some(v) = value {
                    out.push(format!("{CHOOSER_KEY}={v}"));
                }
                done = true;
            }
            in_pref = t == "[preferred]";
            have_pref |= in_pref;
            out.push(line.to_string());
            continue;
        }
        if in_pref
            && t.split_once('=')
                .is_some_and(|(k, _)| k.trim() == CHOOSER_KEY)
        {
            if let Some(v) = value {
                out.push(format!("{CHOOSER_KEY}={v}"));
            }
            done = true;
            continue;
        }
        out.push(line.to_string());
    }
    if !done {
        if let Some(v) = value {
            if !have_pref {
                out.push("[preferred]".into());
            }
            out.push(format!("{CHOOSER_KEY}={v}"));
        }
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

fn current(text: &str) -> Option<String> {
    let mut in_pref = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_pref = t == "[preferred]";
            continue;
        }
        if in_pref {
            if let Some((k, v)) = t.split_once('=') {
                if k.trim() == CHOOSER_KEY {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

/// `user`: the user's file if it exists; `system`: the system default used
/// as a starting point when it does not. Returns the new text and the
/// chooser value that was set before (to restore on release).
pub fn claim(user: Option<&str>, system: &str) -> (String, Option<String>) {
    let base = user.unwrap_or(system);
    let previous = current(base).filter(|v| v != GUST);
    (set_key(base, Some(GUST)), previous)
}

/// Undo `claim`: only if the chooser is still Gust.
pub fn release(text: &str, previous: Option<&str>) -> String {
    if current(text).as_deref() != Some(GUST) {
        return text.to_string();
    }
    set_key(text, previous)
}

pub fn is_claimed(text: &str) -> bool {
    current(text).as_deref() == Some(GUST)
}

/// The user's portals.conf for this desktop, and the system one.
pub fn paths() -> Option<(PathBuf, PathBuf)> {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let desktop = desktop.split(':').next().unwrap_or("").to_lowercase();
    let name = if desktop.is_empty() {
        "portals.conf".to_string()
    } else {
        format!("{desktop}-portals.conf")
    };
    let cfg = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some((
        cfg.join("xdg-desktop-portal").join(&name),
        PathBuf::from("/usr/share/xdg-desktop-portal").join(&name),
    ))
}

/// Same configuration, ignoring trailing whitespace and blank lines — the
/// test for "this file still says only what Gust started it from".
pub fn same_config(a: &str, b: &str) -> bool {
    fn norm(s: &str) -> Vec<&str> {
        s.lines()
            .map(str::trim_end)
            .filter(|l| !l.is_empty())
            .collect()
    }
    norm(a) == norm(b)
}
