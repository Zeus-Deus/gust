//! Key presets: one table, used three ways.
//!
//! The window asks [`action_for`] what a key press means under the user's
//! preset, the `?` key map is rendered from [`bindings`], and the tests below
//! check the table itself. A shortcut and its documentation can therefore
//! never drift apart.
//!
//! Only chords whose meaning differs between presets, or that are new, live
//! here; the cursor keys, Enter/Backspace in Standard, the filter `/` and the
//! palette `:` stay in the window's own handler because every preset shares
//! them.
//!
//! Key codes and modifier bits are Qt's (`Qt::Key`, `Qt::KeyboardModifier`),
//! so QML passes `event.key` and `event.modifiers` straight through.

/// Qt modifier bits.
pub const SHIFT: u32 = 0x0200_0000;
pub const CTRL: u32 = 0x0400_0000;
pub const ALT: u32 = 0x0800_0000;
pub const META: u32 = 0x1000_0000;
/// Keypad bit, ignored: keypad digits and operators mean the same thing.
const KEYPAD: u32 = 0x2000_0000;
const MOD_MASK: u32 = SHIFT | CTRL | ALT | META;

/// Qt key codes used by the table.
pub mod key {
    pub const ESCAPE: u32 = 0x0100_0000;
    pub const TAB: u32 = 0x0100_0001;
    pub const BACKSPACE: u32 = 0x0100_0003;
    pub const RETURN: u32 = 0x0100_0004;
    pub const ENTER: u32 = 0x0100_0005;
    pub const DELETE: u32 = 0x0100_0007;
    pub const HOME: u32 = 0x0100_0010;
    pub const END: u32 = 0x0100_0011;
    pub const LEFT: u32 = 0x0100_0012;
    pub const UP: u32 = 0x0100_0013;
    pub const RIGHT: u32 = 0x0100_0014;
    pub const DOWN: u32 = 0x0100_0015;
    pub const F1: u32 = 0x0100_0030;
    pub const F2: u32 = 0x0100_0031;
    pub const F3: u32 = 0x0100_0032;
    pub const PLUS: u32 = 0x2b;
    pub const MINUS: u32 = 0x2d;
    pub const ZERO: u32 = 0x30;
    pub const ONE: u32 = 0x31;
    pub const NINE: u32 = 0x39;
    pub const EQUAL: u32 = 0x3d;
    pub const QUESTION: u32 = 0x3f;
    pub const A: u32 = 0x41;
    pub const E: u32 = 0x45;
    pub const I: u32 = 0x49;
    pub const L: u32 = 0x4c;
    pub const N: u32 = 0x4e;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Standard,
    Vim,
    Mac,
    Windows,
}

impl Preset {
    pub fn parse(s: &str) -> Option<Preset> {
        match s {
            "standard" => Some(Preset::Standard),
            "vim" => Some(Preset::Vim),
            "mac" => Some(Preset::Mac),
            "windows" => Some(Preset::Windows),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Preset::Standard => "standard",
            Preset::Vim => "vim",
            Preset::Mac => "mac",
            Preset::Windows => "windows",
        }
    }
    pub const ALL: [Preset; 4] = [Preset::Standard, Preset::Vim, Preset::Mac, Preset::Windows];
}

/// One binding: what the user presses, what it does, and the words the key
/// map shows for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub key: u32,
    pub mods: u32,
    pub action: &'static str,
    /// How the key is written in the key map, e.g. "Ctrl+1–9".
    pub label: &'static str,
    /// What it does, in words.
    pub does: &'static str,
    /// Group heading in the key map.
    pub group: &'static str,
}

const fn b(
    key: u32,
    mods: u32,
    action: &'static str,
    label: &'static str,
    does: &'static str,
    group: &'static str,
) -> Binding {
    Binding {
        key,
        mods,
        action,
        label,
        does,
        group,
    }
}

// Shared by every preset.
const COMMON: &[Binding] = &[
    b(
        key::QUESTION,
        SHIFT,
        "keymap",
        "?",
        "Show this key map",
        "Help",
    ),
    b(key::F1, 0, "keymap", "F1", "Show this key map", "Help"),
    b(
        key::I,
        CTRL,
        "invert-selection",
        "Ctrl+I",
        "Invert the selection",
        "Select",
    ),
    b(
        key::A,
        CTRL | SHIFT,
        "deselect-all",
        "Ctrl+Shift+A",
        "Select nothing",
        "Select",
    ),
    b(
        key::F3,
        0,
        "toggle-split",
        "F3",
        "Two panes on / off",
        "View",
    ),
    b(
        key::PLUS,
        CTRL,
        "zoom-in",
        "Ctrl+=",
        "Bigger text and rows",
        "View",
    ),
    b(
        key::EQUAL,
        CTRL,
        "zoom-in",
        "Ctrl+=",
        "Bigger text and rows",
        "View",
    ),
    b(
        key::PLUS,
        CTRL | SHIFT,
        "zoom-in",
        "Ctrl+=",
        "Bigger text and rows",
        "View",
    ),
    b(
        key::MINUS,
        CTRL,
        "zoom-out",
        "Ctrl+-",
        "Smaller text and rows",
        "View",
    ),
    b(
        key::ZERO,
        CTRL,
        "zoom-reset",
        "Ctrl+0",
        "Normal size",
        "View",
    ),
    b(
        key::N,
        CTRL | SHIFT,
        "new-file",
        "Ctrl+Shift+N",
        "New empty file",
        "Files",
    ),
    b(
        key::L,
        CTRL,
        "jump",
        "Ctrl+L",
        "Jump to a folder or path",
        "Go",
    ),
    b(key::E, CTRL, "eject", "Ctrl+E", "Eject this drive", "Go"),
];

// Only what a preset changes. Earlier tables win, so a preset can override.
const STANDARD: &[Binding] = &[
    b(key::BACKSPACE, 0, "up", "Backspace", "Up one folder", "Go"),
    b(key::LEFT, ALT, "back", "Alt+←", "Back", "Go"),
    b(key::RIGHT, ALT, "forward", "Alt+→", "Forward", "Go"),
    b(key::UP, ALT, "up", "Alt+↑", "Up one folder", "Go"),
    b(key::F2, 0, "rename", "F2", "Rename", "Files"),
    b(key::DELETE, 0, "trash", "Delete", "Move to Trash", "Files"),
];

const MAC: &[Binding] = &[
    // Omarchy's window manager owns Super, so Cmd is Ctrl here.
    b(key::RETURN, 0, "rename", "Enter", "Rename", "Files"),
    b(key::ENTER, 0, "rename", "Enter", "Rename", "Files"),
    b(key::DOWN, CTRL, "open", "Ctrl+↓", "Open", "Go"),
    b(key::UP, CTRL, "up", "Ctrl+↑", "Up one folder", "Go"),
    b(
        key::BACKSPACE,
        CTRL,
        "trash",
        "Ctrl+Backspace",
        "Move to Trash",
        "Files",
    ),
    b(key::LEFT, CTRL, "back", "Ctrl+[", "Back", "Go"),
    b(key::RIGHT, CTRL, "forward", "Ctrl+]", "Forward", "Go"),
    b(key::BACKSPACE, 0, "up", "Backspace", "Up one folder", "Go"),
];

const WINDOWS: &[Binding] = &[
    // Explorer: Backspace goes BACK in history, Alt+↑ goes up.
    b(key::BACKSPACE, 0, "back", "Backspace", "Back", "Go"),
    b(key::UP, ALT, "up", "Alt+↑", "Up one folder", "Go"),
    b(key::LEFT, ALT, "back", "Alt+←", "Back", "Go"),
    b(key::RIGHT, ALT, "forward", "Alt+→", "Forward", "Go"),
    b(key::F2, 0, "rename", "F2", "Rename", "Files"),
    b(key::DELETE, 0, "trash", "Delete", "Move to Trash", "Files"),
    b(
        key::N,
        CTRL | SHIFT,
        "new-folder",
        "Ctrl+Shift+N",
        "New folder",
        "Files",
    ),
];

const VIM: &[Binding] = &[
    b(
        key::BACKSPACE,
        0,
        "up",
        "Backspace / h",
        "Up one folder",
        "Go",
    ),
    b(key::LEFT, ALT, "back", "Alt+←", "Back", "Go"),
    b(key::RIGHT, ALT, "forward", "Alt+→", "Forward", "Go"),
];

fn preset_table(p: Preset) -> &'static [Binding] {
    match p {
        Preset::Standard => STANDARD,
        Preset::Vim => VIM,
        Preset::Mac => MAC,
        Preset::Windows => WINDOWS,
    }
}

/// Every binding in force for a preset, preset-specific first.
pub fn bindings(p: Preset) -> Vec<Binding> {
    let mut out: Vec<Binding> = preset_table(p).to_vec();
    for c in COMMON {
        if !out.iter().any(|o| o.key == c.key && o.mods == c.mods) {
            out.push(*c);
        }
    }
    out
}

/// What a key press means under `p`, or `None` when the table has no say
/// (the window's shared handler then deals with it).
///
/// Ctrl+1…9 is computed rather than tabled: tab N.
pub fn action_for(p: Preset, key: u32, mods: u32) -> Option<String> {
    let mods = mods & MOD_MASK & !KEYPAD;
    if mods == CTRL && (key::ONE..=key::NINE).contains(&key) {
        return Some(format!("tab-{}", key - key::ZERO));
    }
    bindings(p)
        .into_iter()
        .find(|b| b.key == key && b.mods == mods)
        .map(|b| b.action.to_string())
}

/// The key map as JSON, grouped, for the `?` overlay. Duplicate labels (the
/// three zoom-in chords) appear once.
pub fn keymap_json(p: Preset) -> String {
    // (group, keys, does), merged: two chords that do the same thing in the
    // same group are one line ("Backspace / Alt+↑"), and a chord listed
    // twice (the three zoom-in chords share one label) appears once.
    let mut rows: Vec<(&'static str, Vec<&'static str>, &'static str)> = Vec::new();
    let mut add = |group: &'static str, label: &'static str, does: &'static str| {
        if let Some(r) = rows.iter_mut().find(|r| r.0 == group && r.2 == does) {
            if !r.1.contains(&label) {
                r.1.push(label);
            }
            return;
        }
        rows.push((group, vec![label], does));
    };
    for b in bindings(p) {
        add(b.group, b.label, b.does);
    }
    // Shared keys the window handles itself, listed so the map is complete.
    for (g, l, d) in SHARED_DOCS {
        add(g, l, d);
    }
    if p == Preset::Vim {
        for (g, l, d) in VIM_DOCS {
            add(g, l, d);
        }
    }
    let out: Vec<String> = rows
        .iter()
        .map(|(g, ks, d)| {
            format!(
                "{{\"group\":{},\"keys\":{},\"does\":{}}}",
                json(g),
                json(&ks.join(" / ")),
                json(d)
            )
        })
        .collect();
    format!("[{}]", out.join(","))
}

const SHARED_DOCS: &[(&str, &str, &str)] = &[
    ("Go", "↑ ↓", "Move the cursor"),
    ("Go", "PgUp PgDn Home End", "Jump a page, or to an end"),
    ("Go", "Enter", "Open"),
    ("Go", "Ctrl+1–9", "Go to tab 1–9"),
    ("Go", "Ctrl+T / Ctrl+W", "New tab / close tab"),
    ("Go", "Ctrl+Tab", "Next tab"),
    ("Go", "Tab", "Other pane (when split)"),
    ("Find", "/", "Filter this folder"),
    ("Find", ":", "Command palette"),
    ("Find", "a–z", "Jump to a name"),
    ("Select", "Ctrl+Space", "Select / unselect"),
    ("Select", "Shift+↑ ↓", "Extend the selection"),
    ("Select", "Ctrl+A", "Select everything shown"),
    ("Files", "Space", "Quick Look"),
    ("Files", "Ctrl+C / X / V", "Copy / cut / paste"),
    ("Files", "Ctrl+Z", "Undo"),
    ("Files", "Ctrl+N", "New folder"),
    ("Files", "Ctrl+D", "Duplicate"),
    ("Files", "F5 / F6", "Copy / move to other pane"),
    ("View", "Ctrl+H", "Hidden files"),
    ("View", "Ctrl+,", "Settings"),
];

const VIM_DOCS: &[(&str, &str, &str)] = &[
    ("Go", "j / k", "Down / up"),
    ("Go", "l", "Open"),
    ("Go", "g", "First row"),
    ("Select", "x", "Select / unselect"),
    ("Files", "y / d / p", "Copy / cut / paste"),
    ("Files", "r / R", "Rename / bulk rename"),
    ("Files", "o", "Open with…"),
    ("View", "v", "Next view"),
];

fn json(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_digits_pick_a_tab_in_every_preset() {
        for p in Preset::ALL {
            assert_eq!(action_for(p, key::ONE, CTRL).as_deref(), Some("tab-1"));
            assert_eq!(action_for(p, key::NINE, CTRL).as_deref(), Some("tab-9"));
            // Ctrl+0 is zoom reset, not "tab 0".
            assert_eq!(
                action_for(p, key::ZERO, CTRL).as_deref(),
                Some("zoom-reset")
            );
            // A bare digit is type-ahead, never a tab switch.
            assert_eq!(action_for(p, key::ONE, 0), None);
        }
    }

    #[test]
    fn the_keypad_bit_does_not_change_the_meaning() {
        assert_eq!(
            action_for(Preset::Standard, key::MINUS, CTRL | KEYPAD).as_deref(),
            Some("zoom-out")
        );
    }

    #[test]
    fn backspace_means_what_each_preset_promises() {
        assert_eq!(
            action_for(Preset::Standard, key::BACKSPACE, 0).as_deref(),
            Some("up")
        );
        assert_eq!(
            action_for(Preset::Windows, key::BACKSPACE, 0).as_deref(),
            Some("back")
        );
        assert_eq!(
            action_for(Preset::Mac, key::BACKSPACE, 0).as_deref(),
            Some("up")
        );
        assert_eq!(
            action_for(Preset::Mac, key::BACKSPACE, CTRL).as_deref(),
            Some("trash")
        );
    }

    #[test]
    fn mac_enter_renames_and_ctrl_down_opens() {
        assert_eq!(
            action_for(Preset::Mac, key::RETURN, 0).as_deref(),
            Some("rename")
        );
        assert_eq!(
            action_for(Preset::Mac, key::DOWN, CTRL).as_deref(),
            Some("open")
        );
        // Everywhere else Enter is left to the shared handler (open).
        assert_eq!(action_for(Preset::Standard, key::RETURN, 0), None);
    }

    #[test]
    fn windows_new_folder_overrides_the_common_new_file() {
        assert_eq!(
            action_for(Preset::Windows, key::N, CTRL | SHIFT).as_deref(),
            Some("new-folder")
        );
        assert_eq!(
            action_for(Preset::Standard, key::N, CTRL | SHIFT).as_deref(),
            Some("new-file")
        );
    }

    #[test]
    fn no_preset_binds_one_chord_twice() {
        for p in Preset::ALL {
            let all = bindings(p);
            for (i, x) in all.iter().enumerate() {
                for y in &all[i + 1..] {
                    assert!(
                        !(x.key == y.key && x.mods == y.mods),
                        "{:?}: {} and {} share a chord",
                        p,
                        x.action,
                        y.action
                    );
                }
            }
        }
    }

    #[test]
    fn a_destructive_action_never_sits_on_a_bare_letter() {
        // Vim-style bare letters must never trash or delete: one stray key
        // would act on the cursor row.
        for p in Preset::ALL {
            for b in bindings(p) {
                if b.action == "trash" {
                    assert!(
                        b.mods != 0 || b.key == key::DELETE,
                        "{:?} puts trash on a bare key {}",
                        p,
                        b.label
                    );
                }
            }
        }
    }

    #[test]
    fn the_key_map_lists_every_bound_action_once() {
        for p in Preset::ALL {
            let j = keymap_json(p);
            let v: serde_json::Value = serde_json::from_str(&j).expect("valid JSON");
            let rows = v.as_array().unwrap();
            assert!(rows.len() > 20, "{p:?}: only {} rows", rows.len());
            let zoom_in = rows
                .iter()
                .filter(|r| r["does"] == "Bigger text and rows")
                .count();
            assert_eq!(zoom_in, 1, "three chords, one line in the map");
            assert!(rows.iter().any(|r| r["keys"] == "Ctrl+1–9"));
            // One line per thing it does, within a group.
            let mut seen = std::collections::HashSet::new();
            for r in rows {
                assert!(
                    seen.insert((r["group"].to_string(), r["does"].to_string())),
                    "{p:?}: {} listed twice",
                    r["does"]
                );
            }
        }
    }

    #[test]
    fn preset_names_round_trip() {
        for p in Preset::ALL {
            assert_eq!(Preset::parse(p.name()), Some(p));
        }
        assert_eq!(Preset::parse("emacs"), None);
    }
}
