//! Omarchy theme token adapter.
//!
//! Reads `~/.local/state/omarchy/current/theme/{colors,shell}.toml` through
//! Gust's *own* parser. No imports from Omarchy's private shell tree.
//!
//! Rules (DESIGN.md §10):
//! - missing or malformed tokens fall back cleanly to a built-in palette
//! - `current` is watched at its PARENT so an atomic replacement is observed
//! - `shell.toml` values like `"hyprland.active-border"` are indirect
//!   references and must be resolved, not shown as literal text

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Resolved token set handed to QML. Every value is a concrete colour/number:
/// QML never contains a literal.
#[derive(Debug, Clone, PartialEq)]
pub struct Tokens {
    pub name: String,
    pub mode: String,
    pub colors: BTreeMap<String, String>,
    pub metrics: BTreeMap<String, f64>,
}

/// Every colour token the UI is allowed to ask for.
///
/// The fallback palette MUST define all of these: a missing token renders as
/// magenta, so an incomplete fallback means a theme without (say) a `[bar]`
/// section paints magenta chrome. `fallback_is_complete` enforces it.
pub const REQUIRED_COLOR_TOKENS: &[&str] = &[
    "background",
    "dark_background",
    "darker_background",
    "lighter_background",
    "foreground",
    "dark_foreground",
    "light_foreground",
    "bright_foreground",
    "accent",
    "selection",
    "muted",
    "red",
    "green",
    "yellow",
    "orange",
    "blue",
    "magenta",
    "cyan",
    "border",
    "bar_background",
    "bar_text",
    "menu_background",
    "menu_text",
    "selected_background",
    "selected_text",
    "tooltip_background",
    "tooltip_text",
];

/// Every metric token the UI is allowed to ask for.
pub const REQUIRED_METRIC_TOKENS: &[&str] = &[
    "font_base",
    "spacing_scale",
    "selected_background_alpha",
    "normal_border_alpha",
    "selection_fill_alpha",
    "border_width",
];

impl Tokens {
    /// Built-in palette used off-Omarchy or when tokens are unreadable.
    /// Values are deliberately neutral and WCAG-contrasting.
    pub fn fallback() -> Self {
        let mut colors = BTreeMap::new();
        for (k, v) in [
            ("background", "#17181e"),
            ("dark_background", "#101116"),
            ("darker_background", "#0b0c10"),
            ("lighter_background", "#20222b"),
            ("foreground", "#d4d7e0"),
            ("dark_foreground", "#8b90a3"),
            ("light_foreground", "#e2e5ec"),
            ("bright_foreground", "#f0f2f7"),
            ("accent", "#7aa2f7"),
            ("selection", "#2a2f44"),
            ("muted", "#414868"),
            ("red", "#f7768e"),
            ("green", "#9ece6a"),
            ("yellow", "#e0af68"),
            ("orange", "#eb927b"),
            ("blue", "#7aa2f7"),
            ("magenta", "#ad8ee6"),
            ("cyan", "#449dab"),
            ("border", "#7aa2f7"),
            // Surface tokens: these come from shell.toml on Omarchy, but a
            // theme may omit the section entirely, so the fallback defines
            // every one of them.
            ("bar_background", "#101116"),
            ("bar_text", "#d4d7e0"),
            ("menu_background", "#17181e"),
            ("menu_text", "#d4d7e0"),
            ("selected_background", "#d4d7e0"),
            ("selected_text", "#7aa2f7"),
            ("tooltip_background", "#17181e"),
            ("tooltip_text", "#d4d7e0"),
        ] {
            colors.insert(k.to_string(), v.to_string());
        }
        let mut metrics = BTreeMap::new();
        for (k, v) in [
            ("font_base", 12.0),
            ("spacing_scale", 1.0),
            ("selected_background_alpha", 0.08),
            ("normal_border_alpha", 0.4),
            ("selection_fill_alpha", 0.35),
            ("border_width", 1.0),
        ] {
            metrics.insert(k.to_string(), v);
        }
        Tokens {
            name: "fallback".into(),
            mode: "dark".into(),
            colors,
            metrics,
        }
    }

    pub fn color(&self, key: &str) -> &str {
        self.colors
            .get(key)
            .map(|s| s.as_str())
            .unwrap_or("#ff00ff") // magenta = missing token, visible in review
    }

    pub fn metric(&self, key: &str) -> f64 {
        *self.metrics.get(key).unwrap_or(&0.0)
    }

    pub fn is_light(&self) -> bool {
        self.mode == "light"
    }
}

/// Default state directory: `~/.local/state/omarchy`.
///
/// `GUST_OMARCHY_STATE` overrides it. That exists for tests and for running
/// against a fixture HOME: without it, pointing HOME at a fixture would make
/// Gust silently fall back to the built-in palette and hide real theming bugs.
pub fn default_state_dir() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("GUST_OMARCHY_STATE") {
        if !explicit.is_empty() {
            return Some(PathBuf::from(explicit));
        }
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local/state/omarchy"))
}

/// The directory to watch for theme changes.
///
/// Watching `current/theme` itself is wrong: Omarchy replaces the `current`
/// symlink atomically, so a watch on the old target never fires. Watch the
/// parent of `current`.
pub fn watch_dir(state_dir: &Path) -> PathBuf {
    state_dir.to_path_buf()
}

fn strip_quotes(s: &str) -> String {
    s.trim().trim_matches('"').trim_matches('\'').to_string()
}

/// Minimal TOML-ish reader for flat `key = value` files with `[section]`
/// headers. Deliberately tolerant: unknown keys are ignored, malformed lines
/// are skipped rather than failing the whole theme.
fn parse_flat(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut section = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_string();
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = if section.is_empty() {
            k.trim().to_string()
        } else {
            format!("{}.{}", section, k.trim())
        };
        // Strip trailing comments outside quotes.
        let mut val = v.trim().to_string();
        if !val.starts_with('"') {
            if let Some(p) = val.find(" #") {
                val.truncate(p);
            }
        }
        out.insert(key, strip_quotes(&val));
    }
    out
}

fn is_color(s: &str) -> bool {
    // #rgb / #rrggbb / #rrggbbaa
    if let Some(hex) = s.strip_prefix('#') {
        return matches!(hex.len(), 3 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit());
    }
    // Hyprland gradient / rgba() forms are passed through as-is.
    s.starts_with("rgba(") || s.starts_with("rgb(")
}

/// Perceived lightness of a hex colour, used to decide whether a surface
/// belongs to a light or a dark theme. Unparseable colours count as dark.
pub fn is_light_color(s: &str) -> bool {
    let Some(hex) = s.strip_prefix('#') else {
        return false;
    };
    let full = match hex.len() {
        3 => hex
            .chars()
            .flat_map(|c| std::iter::repeat_n(c, 2))
            .collect::<String>(),
        6 | 8 => hex[..6].to_string(),
        _ => return false,
    };
    let parse = |i: usize| u8::from_str_radix(&full[i..i + 2], 16).unwrap_or(0) as f64;
    let (r, g, b) = (parse(0), parse(2), parse(4));
    // Rec. 709 luma.
    (0.2126 * r + 0.7152 * g + 0.0722 * b) > 127.5
}

/// Resolve a shell.toml value that may reference another token,
/// e.g. `border = "hyprland.active-border"`.
fn resolve_ref(raw: &str, shell: &BTreeMap<String, String>, depth: usize) -> Option<String> {
    if depth > 4 {
        return None; // reference cycle
    }
    if is_color(raw) {
        return Some(raw.to_string());
    }
    if raw.contains('.') {
        if let Some(next) = shell.get(raw) {
            return resolve_ref(next, shell, depth + 1);
        }
    }
    None
}

/// Load tokens from a theme directory. Never fails: on any problem the
/// fallback palette is returned with whatever was readable merged in.
pub fn load_from_theme_dir(theme_dir: &Path, name: &str) -> Tokens {
    let mut t = Tokens::fallback();
    t.name = name.to_string();

    let colors_text = std::fs::read_to_string(theme_dir.join("colors.toml")).unwrap_or_default();
    let colors = parse_flat(&colors_text);
    if colors.is_empty() {
        return t; // unreadable/absent → clean fallback
    }

    if let Some(m) = colors.get("mode") {
        if m == "light" || m == "dark" {
            t.mode = m.clone();
        }
    }
    for (k, v) in &colors {
        if k != "mode" && is_color(v) {
            t.colors.insert(k.clone(), v.clone());
        }
    }

    let shell_text = std::fs::read_to_string(theme_dir.join("shell.toml")).unwrap_or_default();
    let shell = parse_flat(&shell_text);

    // Border: resolve the indirect reference, else fall back to accent.
    let border = shell
        .get("popups.border")
        .and_then(|r| resolve_ref(r, &shell, 0))
        .or_else(|| shell.get("hyprland.active-border").map(|s| s.to_string()))
        .filter(|s| is_color(s))
        .unwrap_or_else(|| t.color("accent").to_string());
    t.colors.insert("border".into(), border);

    for (dst, src) in [
        ("menu_background", "menu.background"),
        ("menu_text", "menu.text"),
        ("selected_background", "menu.selected-background"),
        ("selected_text", "menu.selected-text"),
        ("bar_background", "bar.background"),
        ("bar_text", "bar.text"),
        ("tooltip_background", "tooltip.background"),
        ("tooltip_text", "tooltip.text"),
    ] {
        if let Some(v) = shell.get(src).and_then(|r| resolve_ref(r, &shell, 0)) {
            t.colors.insert(dst.into(), v);
        }
    }

    // shell.toml is GENERATED per install and can lag behind colors.toml — a
    // light theme then inherits a dark bar and the window looks half-themed.
    // colors.toml is authoritative, so any surface colour whose lightness
    // disagrees with the theme's mode is replaced by a derived one.
    {
        let bg = t.color("background").to_string();
        let fg = t.color("foreground").to_string();
        let want_light = t.mode == "light";
        let surfaces = [
            ("bar_background", "dark_background", &bg),
            // Omarchy's template (default/themed/shell.toml.tpl) makes menus
            // and tooltips the theme background; lighter_background can be a
            // mid grey (white: #c0c0c0) that reads as an unthemed panel.
            ("menu_background", "background", &bg),
            ("tooltip_background", "background", &bg),
        ];
        for (key, preferred, fallback) in surfaces {
            let current = t.color(key).to_string();
            if is_light_color(&current) != want_light {
                let replacement = {
                    let p = t.color(preferred).to_string();
                    if is_light_color(&p) == want_light {
                        p
                    } else {
                        fallback.to_string()
                    }
                };
                t.colors.insert(key.into(), replacement);
            }
        }
        for key in ["bar_text", "menu_text", "tooltip_text"] {
            let current = t.color(key).to_string();
            // Text must contrast with the surface, i.e. be the opposite
            // lightness to the background.
            if is_light_color(&current) == want_light {
                t.colors.insert(key.into(), fg.clone());
            }
        }
    }

    for (dst, src) in [
        ("font_base", "font.base-size"),
        ("spacing_scale", "spacing.scale"),
        (
            "selected_background_alpha",
            "menu.selected-background-alpha",
        ),
        ("normal_border_alpha", "controls.normal-border-alpha"),
        ("selection_fill_alpha", "controls.selection-fill-alpha"),
        ("border_width", "controls.normal-border-width"),
    ] {
        if let Some(v) = shell.get(src).and_then(|s| s.parse::<f64>().ok()) {
            if v.is_finite() && v >= 0.0 {
                t.metrics.insert(dst.into(), v);
            }
        }
    }
    // A theme must never shrink text to nothing.
    if t.metric("font_base") < 6.0 {
        t.metrics.insert("font_base".into(), 12.0);
    }
    enforce_contrast(&mut t);
    t
}

/// WCAG 2 minimums Gust holds every theme to, against `background`.
pub const MIN_PRIMARY_CONTRAST: f64 = 4.5;
pub const MIN_SECONDARY_CONTRAST: f64 = 3.0;

/// Tokens used for secondary text (dates, sizes, hints, dimmed hidden
/// files). Omarchy's `dark_foreground` is a terminal colour, not a text
/// colour: in 15 of the 22 stock themes it is under 3:1 on the background.
pub const SECONDARY_TEXT_TOKENS: &[&str] = &["dark_foreground"];
pub const PRIMARY_TEXT_TOKENS: &[&str] = &["foreground", "bright_foreground"];
/// Every surface text is drawn on: the list, the sidebar and hint strip,
/// menus / banners / dialogs, and the bar. Text must pass on the WORST of
/// them, not only on `background` (the white theme's banner hint line was
/// 1.9:1 on its menu surface while passing on the list).
pub const TEXT_SURFACES: &[&str] = &[
    "background",
    "dark_background",
    "menu_background",
    "bar_background",
];

/// The lowest contrast of `c` over every text surface.
pub fn worst_contrast(t: &Tokens, c: &str) -> f64 {
    TEXT_SURFACES
        .iter()
        .map(|s| contrast_ratio(c, t.color(s)))
        .fold(f64::INFINITY, f64::min)
}

fn rgb(s: &str) -> Option<(f64, f64, f64)> {
    let hex = s.strip_prefix('#')?;
    let full: String = match hex.len() {
        3 => hex
            .chars()
            .flat_map(|c| std::iter::repeat_n(c, 2))
            .collect(),
        6 | 8 => hex[..6].to_string(),
        _ => return None,
    };
    let p = |i: usize| {
        u8::from_str_radix(&full[i..i + 2], 16)
            .ok()
            .map(|v| v as f64)
    };
    Some((p(0)?, p(2)?, p(4)?))
}

fn luminance((r, g, b): (f64, f64, f64)) -> f64 {
    let f = |c: f64| {
        let c = c / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
}

/// WCAG contrast ratio of two colours, 1.0 … 21.0. Unparseable → 1.0.
pub fn contrast_ratio(a: &str, b: &str) -> f64 {
    match (rgb(a), rgb(b)) {
        (Some(x), Some(y)) => {
            let (la, lb) = (luminance(x), luminance(y));
            (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
        }
        _ => 1.0,
    }
}

fn hex_of((r, g, b): (f64, f64, f64)) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        r.round().clamp(0.0, 255.0) as u8,
        g.round().clamp(0.0, 255.0) as u8,
        b.round().clamp(0.0, 255.0) as u8
    )
}

/// Move `c` toward `toward` in small steps until it reaches `min` contrast
/// on `bg`. Keeps the theme's hue as long as possible: the smallest change
/// that passes, not a jump to the foreground.
fn lift(t: &Tokens, c: &str, toward: &str, min: f64) -> Option<String> {
    if worst_contrast(t, c) >= min {
        return None;
    }
    let (a, b) = (rgb(c)?, rgb(toward)?);
    for step in 1..=20 {
        let f = step as f64 / 20.0;
        let m = (
            a.0 + (b.0 - a.0) * f,
            a.1 + (b.1 - a.1) * f,
            a.2 + (b.2 - a.2) * f,
        );
        let h = hex_of(m);
        if worst_contrast(t, &h) >= min {
            return Some(h);
        }
    }
    Some(toward.to_string())
}

/// Hold text tokens to the WCAG minimums against the background. Secondary
/// text is lifted toward the foreground; primary text (already ≥ 6.6:1 in
/// every stock theme) toward black or white only if a custom theme needs it.
pub fn enforce_contrast(t: &mut Tokens) {
    let bg = t.color("background").to_string();
    let extreme = if is_light_color(&bg) {
        "#000000"
    } else {
        "#ffffff"
    };
    for k in PRIMARY_TEXT_TOKENS {
        let c = t.color(k).to_string();
        if let Some(new) = lift(t, &c, extreme, MIN_PRIMARY_CONTRAST) {
            t.colors.insert((*k).into(), new);
        }
    }
    let fg = t.color("foreground").to_string();
    for k in SECONDARY_TEXT_TOKENS {
        let c = t.color(k).to_string();
        if let Some(new) = lift(t, &c, &fg, MIN_SECONDARY_CONTRAST) {
            t.colors.insert((*k).into(), new);
        }
    }
}

/// Load the currently active Omarchy theme, or the fallback off-Omarchy.
/// Folders to watch so a theme switch is noticed.
///
/// `omarchy-theme-set` replaces `current/theme` with `rm -rf` + `mv`, which a
/// watch on `current/` sees. Older layouts swap a `current` SYMLINK
/// atomically, which only a watch on the state folder itself sees. Both are
/// watched; each event just triggers one cheap reload.
pub fn theme_watch_dirs(state: &Path) -> Vec<PathBuf> {
    vec![state.to_path_buf(), state.join("current")]
}

/// Whether a complete theme is in place. False in the gap between removing
/// the old theme and moving the new one in, where reloading would paint the
/// fallback palette for a frame.
pub fn theme_ready(state: &Path) -> bool {
    state.join("current/theme/colors.toml").is_file()
}

pub fn load_current() -> Tokens {
    let Some(state) = default_state_dir() else {
        return Tokens::fallback();
    };
    let theme_dir = state.join("current/theme");
    if !theme_dir.is_dir() {
        return Tokens::fallback();
    }
    let name = std::fs::read_to_string(state.join("current/theme.name"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "current".into());
    load_from_theme_dir(&theme_dir, &name)
}

/// Enumerate installed stock themes. Never a hardcoded count.
pub fn installed_themes() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for base in [
        PathBuf::from("/usr/share/omarchy/themes"),
        default_state_dir()
            .map(|d| d.join("themes"))
            .unwrap_or_default(),
    ] {
        let Ok(rd) = std::fs::read_dir(&base) else {
            continue;
        };
        for e in rd.flatten() {
            if e.path().is_dir() && e.path().join("colors.toml").exists() {
                let n = e.file_name().to_string_lossy().to_string();
                if !out.iter().any(|(x, _): &(String, PathBuf)| *x == n) {
                    out.push((n, e.path()));
                }
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_theme(dir: &Path, colors: &str, shell: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("colors.toml"), colors).unwrap();
        fs::write(dir.join("shell.toml"), shell).unwrap();
    }

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("gust-theme-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn fallback_defines_every_required_token() {
        // A token the UI asks for but the palette lacks renders as magenta.
        // This test is why the header bar is not magenta.
        let f = Tokens::fallback();
        let missing: Vec<_> = REQUIRED_COLOR_TOKENS
            .iter()
            .filter(|k| !f.colors.contains_key(**k))
            .collect();
        assert!(
            missing.is_empty(),
            "fallback is missing colours: {missing:?}"
        );

        let missing_m: Vec<_> = REQUIRED_METRIC_TOKENS
            .iter()
            .filter(|k| !f.metrics.contains_key(**k))
            .collect();
        assert!(
            missing_m.is_empty(),
            "fallback is missing metrics: {missing_m:?}"
        );

        // And nothing resolves to the sentinel.
        for k in REQUIRED_COLOR_TOKENS {
            assert_ne!(
                f.color(k),
                "#ff00ff",
                "{k} resolved to the missing sentinel"
            );
        }
    }

    #[test]
    fn a_theme_without_a_shell_file_still_resolves_every_token() {
        // colors.toml only — no shell.toml at all.
        let d = tmp("noshell");
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("colors.toml"),
            "mode = \"dark\"\naccent = \"#123456\"\n",
        )
        .unwrap();
        let t = load_from_theme_dir(&d, "noshell");
        for k in REQUIRED_COLOR_TOKENS {
            assert_ne!(
                t.color(k),
                "#ff00ff",
                "{k} would render as magenta for a theme with no shell.toml"
            );
        }
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn every_installed_theme_resolves_every_token() {
        // Enumerated, never a hardcoded count (plan §9).
        let themes = installed_themes();
        if themes.is_empty() {
            eprintln!("no installed Omarchy themes here; skipping matrix check");
            return;
        }
        let mut problems = Vec::new();
        for (name, dir) in &themes {
            let t = load_from_theme_dir(dir, name);
            for k in REQUIRED_COLOR_TOKENS {
                if t.color(k) == "#ff00ff" {
                    problems.push(format!("{name}: {k}"));
                }
            }
            if t.metric("font_base") < 6.0 {
                problems.push(format!("{name}: font_base too small"));
            }
        }
        assert!(
            problems.is_empty(),
            "{} themes checked, unresolved tokens: {problems:?}",
            themes.len()
        );
    }

    #[test]
    fn a_light_theme_never_gets_a_dark_bar_from_a_stale_shell_file() {
        // Omarchy generates shell.toml per install, so it can still carry the
        // PREVIOUS theme's surface colours. colors.toml is authoritative: a
        // light theme must not render a dark header bar.
        let d = tmp("stalebar");
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("colors.toml"),
            "mode = \"light\"\nbackground = \"#faf4ed\"\nforeground = \"#575279\"\n\
             dark_background = \"#f2e9e1\"\nlighter_background = \"#fffaf3\"\naccent = \"#907aa9\"\n",
        )
        .unwrap();
        // Stale dark values left over from a dark theme.
        fs::write(
            d.join("shell.toml"),
            "[bar]\nbackground = \"#1a1b26\"\ntext = \"#a9b1d6\"\n\
             \n[menu]\nbackground = \"#1a1b26\"\ntext = \"#a9b1d6\"\n",
        )
        .unwrap();

        let t = load_from_theme_dir(&d, "rose-pine");
        assert!(t.is_light());
        assert!(
            is_light_color(t.color("bar_background")),
            "light theme got a dark bar: {}",
            t.color("bar_background")
        );
        assert!(
            !is_light_color(t.color("bar_text")),
            "bar text must contrast with a light bar: {}",
            t.color("bar_text")
        );
        assert!(
            is_light_color(t.color("menu_background")),
            "light theme got a dark menu surface"
        );
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_stale_shell_gives_menus_the_theme_background_like_omarchy_does() {
        let d = tmp("whitemenu");
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("colors.toml"),
            "mode = \"light\"\nbackground = \"#ffffff\"\nforeground = \"#000000\"\n\
             lighter_background = \"#c0c0c0\"\naccent = \"#6e6e6e\"\n",
        )
        .unwrap();
        fs::write(
            d.join("shell.toml"),
            "[menu]\nbackground = \"#1a1b26\"\ntext = \"#a9b1d6\"\n",
        )
        .unwrap();
        let t = load_from_theme_dir(&d, "white");
        assert_eq!(t.color("menu_background"), "#ffffff");
        assert_eq!(t.color("tooltip_background"), "#ffffff");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_dark_theme_keeps_its_dark_surfaces() {
        let d = tmp("darkkeep");
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("colors.toml"),
            "mode = \"dark\"\nbackground = \"#1a1b26\"\nforeground = \"#a9b1d6\"\n\
             dark_background = \"#13141c\"\naccent = \"#7aa2f7\"\n",
        )
        .unwrap();
        fs::write(
            d.join("shell.toml"),
            "[bar]\nbackground = \"#1a1b26\"\ntext = \"#a9b1d6\"\n",
        )
        .unwrap();
        let t = load_from_theme_dir(&d, "tokyo-night");
        assert_eq!(t.color("bar_background"), "#1a1b26");
        assert_eq!(t.color("bar_text"), "#a9b1d6");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn lightness_detection_is_sane() {
        assert!(is_light_color("#ffffff"));
        assert!(is_light_color("#faf4ed"));
        assert!(!is_light_color("#000000"));
        assert!(!is_light_color("#1a1b26"));
        assert!(!is_light_color("not-a-colour"));
    }

    #[test]
    fn every_installed_theme_has_surfaces_matching_its_mode() {
        // The real matrix: no installed theme may render a mismatched bar.
        let themes = installed_themes();
        if themes.is_empty() {
            eprintln!("no installed Omarchy themes here; skipping");
            return;
        }
        let mut problems = Vec::new();
        for (name, dir) in &themes {
            let t = load_from_theme_dir(dir, name);
            let want_light = t.is_light();
            for key in ["bar_background", "menu_background", "tooltip_background"] {
                if is_light_color(t.color(key)) != want_light {
                    problems.push(format!("{name}: {key}={} mode={}", t.color(key), t.mode));
                }
            }
        }
        assert!(
            problems.is_empty(),
            "{} themes checked, mismatched surfaces: {problems:?}",
            themes.len()
        );
    }

    #[test]
    fn reads_colors_and_mode() {
        let d = tmp("basic");
        write_theme(
            &d,
            "mode = \"light\"\naccent = \"#123456\"\nbackground = \"#ffffff\"\n",
            "[font]\nbase-size = 14\n",
        );
        let t = load_from_theme_dir(&d, "x");
        assert_eq!(t.mode, "light");
        assert!(t.is_light());
        assert_eq!(t.color("accent"), "#123456");
        assert_eq!(t.metric("font_base"), 14.0);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn resolves_indirect_border_reference() {
        // shell.toml says border = "hyprland.active-border" — a reference,
        // not a colour. Showing the literal string would be a bug.
        let d = tmp("ref");
        write_theme(
            &d,
            "accent = \"#7aa2f7\"\n",
            "[hyprland]\nactive-border = \"#abcdef\"\n\n[popups]\nborder = \"hyprland.active-border\"\n",
        );
        let t = load_from_theme_dir(&d, "x");
        assert_eq!(t.color("border"), "#abcdef");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn unresolvable_reference_falls_back_to_accent() {
        let d = tmp("badref");
        write_theme(
            &d,
            "accent = \"#00ff00\"\n",
            "[popups]\nborder = \"nope.missing\"\n",
        );
        let t = load_from_theme_dir(&d, "x");
        assert_eq!(t.color("border"), "#00ff00");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn malformed_theme_falls_back_cleanly() {
        let d = tmp("malformed");
        write_theme(&d, "this is not toml at all %%%%\n", "garbage\n");
        let t = load_from_theme_dir(&d, "x");
        assert_eq!(
            t.color("background"),
            Tokens::fallback().color("background")
        );
        assert_eq!(t.metric("font_base"), 12.0);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn missing_theme_dir_returns_fallback() {
        let t = load_from_theme_dir(Path::new("/gust/nope/missing"), "x");
        assert_eq!(t.colors, Tokens::fallback().colors);
    }

    #[test]
    fn absurd_font_size_is_clamped() {
        let d = tmp("font");
        write_theme(&d, "accent = \"#fff\"\n", "[font]\nbase-size = 0\n");
        assert_eq!(load_from_theme_dir(&d, "x").metric("font_base"), 12.0);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn non_color_values_are_rejected() {
        let d = tmp("notcolor");
        write_theme(&d, "accent = \"not-a-color\"\n", "");
        let t = load_from_theme_dir(&d, "x");
        assert_eq!(t.color("accent"), Tokens::fallback().color("accent"));
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn watch_target_is_parent_of_current_symlink() {
        // Watching current/theme misses an atomic symlink swap.
        let state = Path::new("/home/u/.local/state/omarchy");
        assert_eq!(watch_dir(state), state);
    }

    #[test]
    fn comments_do_not_leak_into_values() {
        let d = tmp("comment");
        write_theme(
            &d,
            "accent = \"#111111\"\n",
            "[spacing]\nscale = 2.0 # doubled\n",
        );
        assert_eq!(load_from_theme_dir(&d, "x").metric("spacing_scale"), 2.0);
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn contrast_ratio_matches_wcag_reference_values() {
        assert!((contrast_ratio("#000000", "#ffffff") - 21.0).abs() < 0.01);
        assert!((contrast_ratio("#777777", "#ffffff") - 4.48).abs() < 0.01);
        assert!((contrast_ratio("#abc", "#abc") - 1.0).abs() < 1e-9);
    }

    #[test]
    fn secondary_text_is_lifted_just_enough() {
        let mut t = Tokens::fallback();
        t.colors.insert("background".into(), "#2d353b".into());
        t.colors.insert("foreground".into(), "#d3c6aa".into());
        // everforest's dark_foreground: 1.72:1 on its background.
        t.colors.insert("dark_foreground".into(), "#475258".into());
        enforce_contrast(&mut t);
        let r = worst_contrast(&t, t.color("dark_foreground"));
        assert!(r >= MIN_SECONDARY_CONTRAST, "{r}");
        assert!(r < 4.5, "lifted to {r}: secondary text must stay secondary");
    }

    #[test]
    fn a_passing_colour_is_left_exactly_as_the_theme_set_it() {
        let mut t = Tokens::fallback();
        let before = t.color("dark_foreground").to_string();
        enforce_contrast(&mut t);
        assert_eq!(t.color("dark_foreground"), before);
    }
}
