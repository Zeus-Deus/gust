//! User settings, in `~/.config/gust/settings.toml`.
//!
//! Deliberately small: only choices the UI actually honours are stored. The
//! parser is per-line and per-key, so one broken value resets that value
//! and nothing else — a settings file can never stop Gust from starting.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPreset {
    Standard,
    Vim,
    /// Finder habits: Enter renames, Ctrl+↓ opens (Cmd is Ctrl here: Omarchy
    /// gives Super to the window manager).
    Mac,
    /// Explorer habits: Backspace goes back, Alt+↑ goes up.
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    Compact,
    Normal,
    Roomy,
}

impl Density {
    /// Multiplier on the theme's spacing scale.
    pub fn spacing(self) -> f64 {
        match self {
            Density::Compact => 0.8,
            Density::Normal => 1.0,
            Density::Roomy => 1.25,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererChoice {
    /// Raster renderer when there is no GPU, the GPU otherwise.
    Auto,
    Software,
    Gpu,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub key_preset: KeyPreset,
    /// Show dotfiles when a folder opens (Ctrl+H still toggles per tab).
    pub show_hidden: bool,
    pub sidebar: bool,
    pub density: Density,
    /// Takes effect at the next start: the renderer is chosen before Qt.
    pub renderer: RendererChoice,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            key_preset: KeyPreset::Standard,
            show_hidden: false,
            sidebar: true,
            density: Density::Normal,
            renderer: RendererChoice::Auto,
        }
    }
}

fn unquote(v: &str) -> &str {
    v.trim().trim_matches('"')
}

impl Settings {
    pub fn default_path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|h| !h.is_empty())
                    .map(|h| PathBuf::from(h).join(".config"))
            })?;
        Some(base.join("gust/settings.toml"))
    }

    pub fn from_text(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let v = unquote(v);
            match k.trim() {
                "key_preset" => match v {
                    "standard" => s.key_preset = KeyPreset::Standard,
                    "vim" => s.key_preset = KeyPreset::Vim,
                    "mac" => s.key_preset = KeyPreset::Mac,
                    "windows" => s.key_preset = KeyPreset::Windows,
                    _ => {}
                },
                "show_hidden" => {
                    if let Ok(b) = v.parse() {
                        s.show_hidden = b
                    }
                }
                "sidebar" => {
                    if let Ok(b) = v.parse() {
                        s.sidebar = b
                    }
                }
                "density" => match v {
                    "compact" => s.density = Density::Compact,
                    "normal" => s.density = Density::Normal,
                    "roomy" => s.density = Density::Roomy,
                    _ => {}
                },
                "renderer" => match v {
                    "auto" => s.renderer = RendererChoice::Auto,
                    "software" => s.renderer = RendererChoice::Software,
                    "gpu" => s.renderer = RendererChoice::Gpu,
                    _ => {}
                },
                _ => {}
            }
        }
        s
    }

    /// Change one value by name, as the Settings screen does. Refuses an
    /// unknown key or value instead of silently ignoring it.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let line = format!("{key} = \"{}\"", value.replace('"', ""));
        let probe = Settings::from_text(&format!("{}\n{line}\n", self.to_text()));
        // Accepted iff the same line applied to a DIFFERENT starting value
        // also lands on `value`: from_text ignores what it rejects.
        let known = matches!(
            key,
            "key_preset" | "show_hidden" | "sidebar" | "density" | "renderer"
        );
        let echoed = probe.to_text().lines().any(|l| {
            l.split_once('=')
                .is_some_and(|(k, v)| k.trim() == key && v.trim().trim_matches('"') == value)
        });
        if !known || !echoed {
            return Err(format!("unknown setting {key} = {value}"));
        }
        *self = probe;
        Ok(())
    }

    pub fn to_text(&self) -> String {
        format!(
            "# Gust settings. Edited from Settings in the command palette (:settings).\n\
             key_preset = \"{}\"\nshow_hidden = {}\nsidebar = {}\ndensity = \"{}\"\nrenderer = \"{}\"\n",
            match self.key_preset {
                KeyPreset::Standard => "standard",
                KeyPreset::Vim => "vim",
                KeyPreset::Mac => "mac",
                KeyPreset::Windows => "windows",
            },
            self.show_hidden,
            self.sidebar,
            match self.density {
                Density::Compact => "compact",
                Density::Normal => "normal",
                Density::Roomy => "roomy",
            },
            match self.renderer {
                RendererChoice::Auto => "auto",
                RendererChoice::Software => "software",
                RendererChoice::Gpu => "gpu",
            },
        )
    }

    /// Bounded, no symlinks, and any failure is just the defaults.
    pub fn load(path: &Path) -> Settings {
        let Ok(md) = path.symlink_metadata() else {
            return Settings::default();
        };
        if !md.is_file() || md.len() > 64 * 1024 {
            return Settings::default();
        }
        std::fs::read_to_string(path)
            .map(|t| Settings::from_text(&t))
            .unwrap_or_default()
    }

    /// Stage, fsync, rename; owner-only.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let dir = path
            .parent()
            .ok_or_else(|| std::io::Error::other("no parent folder"))?;
        std::fs::create_dir_all(dir)?;
        let stage = dir.join(format!(".settings-{}.tmp", std::process::id()));
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
}

impl Settings {
    /// The value names, as the Settings screen shows and sends them.
    pub fn key_preset_name(&self) -> &'static str {
        match self.key_preset {
            KeyPreset::Standard => "standard",
            KeyPreset::Vim => "vim",
            KeyPreset::Mac => "mac",
            KeyPreset::Windows => "windows",
        }
    }
    pub fn density_name(&self) -> &'static str {
        match self.density {
            Density::Compact => "compact",
            Density::Normal => "normal",
            Density::Roomy => "roomy",
        }
    }
    pub fn renderer_name(&self) -> &'static str {
        match self.renderer {
            RendererChoice::Auto => "auto",
            RendererChoice::Software => "software",
            RendererChoice::Gpu => "gpu",
        }
    }
}
