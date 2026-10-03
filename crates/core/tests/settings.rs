//! User settings: a small, flat file Gust owns (`~/.config/gust/settings.toml`).
//!
//! Every value that reaches the file is validated on the way IN and the way
//! OUT: an unknown or broken value falls back to the default for THAT key
//! only, so one bad line can never stop Gust from starting.

use gust_core::settings::{Density, KeyPreset, RendererChoice, Settings};

#[test]
fn defaults_are_the_safe_ones() {
    let s = Settings::default();
    assert_eq!(s.key_preset, KeyPreset::Standard);
    assert!(!s.show_hidden);
    assert!(s.sidebar);
    assert_eq!(s.density, Density::Normal);
    assert_eq!(s.renderer, RendererChoice::Auto);
}

#[test]
fn a_round_trip_keeps_every_value() {
    let s = Settings {
        key_preset: KeyPreset::Vim,
        show_hidden: true,
        sidebar: false,
        density: Density::Compact,
        renderer: RendererChoice::Software,
        restore_tabs: true,
    };
    assert_eq!(Settings::from_text(&s.to_text()), s);
}

#[test]
fn restoring_tabs_is_off_unless_chosen() {
    assert!(!Settings::default().restore_tabs);
    assert!(!Settings::from_text("restore_tabs = maybe\n").restore_tabs);
    assert!(Settings::from_text("restore_tabs = true\n").restore_tabs);
}

#[test]
fn one_bad_line_only_resets_that_line() {
    let text =
        "key_preset = \"vim\"\ndensity = \"gigantic\"\nshow_hidden = maybe\nsidebar = false\n";
    let s = Settings::from_text(text);
    assert_eq!(s.key_preset, KeyPreset::Vim);
    assert_eq!(s.density, Density::Normal);
    assert!(!s.show_hidden);
    assert!(!s.sidebar);
}

#[test]
fn garbage_is_just_defaults() {
    assert_eq!(
        Settings::from_text("\u{0}\u{1}[[[\n=\n==="),
        Settings::default()
    );
}

#[test]
fn density_scales_spacing_and_nothing_else() {
    assert!(Density::Compact.spacing() < Density::Normal.spacing());
    assert!(Density::Roomy.spacing() > Density::Normal.spacing());
    assert_eq!(Density::Normal.spacing(), 1.0);
}

#[test]
fn save_is_atomic_private_and_load_refuses_a_symlink() {
    use std::os::unix::fs::PermissionsExt;
    let d = std::env::temp_dir().join(format!("gust-settings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let f = d.join("gust/settings.toml");
    let s = Settings {
        sidebar: false,
        ..Settings::default()
    };
    s.save(&f).unwrap();
    assert_eq!(Settings::load(&f), s);
    assert_eq!(
        std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(std::fs::read_dir(f.parent().unwrap()).unwrap().count(), 1);
    // A symlink at the settings path is not followed.
    let other = d.join("elsewhere.toml");
    std::fs::write(&other, "sidebar = false\n").unwrap();
    std::fs::remove_file(&f).unwrap();
    std::os::unix::fs::symlink(&other, &f).unwrap();
    assert_eq!(Settings::load(&f), Settings::default());
}

#[test]
fn setting_one_value_by_name_validates_it() {
    let mut s = Settings::default();
    s.set("density", "roomy").unwrap();
    s.set("sidebar", "false").unwrap();
    assert_eq!((s.density, s.sidebar), (Density::Roomy, false));
    assert!(s.set("density", "huge").is_err());
    assert!(s.set("colour", "red").is_err());
    assert_eq!(
        s.density,
        Density::Roomy,
        "a refused change leaves the value alone"
    );
}

#[test]
fn all_four_key_presets_survive_a_save_and_load() {
    for name in ["standard", "vim", "mac", "windows"] {
        let mut s = Settings::default();
        s.set("key_preset", name).expect(name);
        let back = Settings::from_text(&s.to_text());
        assert_eq!(back.key_preset_name(), name);
    }
    let mut s = Settings::default();
    assert!(
        s.set("key_preset", "emacs").is_err(),
        "an unknown preset is refused"
    );
    assert_eq!(s.key_preset_name(), "standard");
}
