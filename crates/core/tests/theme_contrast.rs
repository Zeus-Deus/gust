//! Every installed Omarchy theme, as Gust loads it, meets the contrast
//! minimums: primary text >= 4.5:1 and secondary text >= 3:1 on the
//! background. Also the fallback palette, which needs no Omarchy.

use gust_core::theme::{
    contrast_ratio, installed_themes, load_from_theme_dir, Tokens, MIN_PRIMARY_CONTRAST,
    MIN_SECONDARY_CONTRAST, PRIMARY_TEXT_TOKENS, SECONDARY_TEXT_TOKENS, TEXT_SURFACES,
};

/// Every text token against every surface text is drawn on.
fn check(t: &Tokens) -> Vec<String> {
    let mut bad = Vec::new();
    for (tokens, min) in [
        (PRIMARY_TEXT_TOKENS, MIN_PRIMARY_CONTRAST),
        (SECONDARY_TEXT_TOKENS, MIN_SECONDARY_CONTRAST),
    ] {
        for k in tokens {
            for surface in TEXT_SURFACES {
                let r = contrast_ratio(t.color(k), t.color(surface));
                if r < min {
                    bad.push(format!(
                        "{}: {k} {} on {surface} {} is {r:.2}:1 (< {min})",
                        t.name,
                        t.color(k),
                        t.color(surface)
                    ));
                }
            }
        }
    }
    bad
}

#[test]
fn the_fallback_palette_meets_the_minimums() {
    let bad = check(&Tokens::fallback());
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn every_installed_theme_meets_the_minimums() {
    let themes = installed_themes();
    gust_testkit::skip_unless!(
        !themes.is_empty(),
        "THEMES",
        "no Omarchy themes installed in /usr/share/omarchy/themes"
    );
    let mut bad = Vec::new();
    for (name, dir) in &themes {
        bad.extend(check(&load_from_theme_dir(dir, name)));
    }
    assert!(bad.is_empty(), "{} themes checked:\n{bad:#?}", themes.len());
    eprintln!("contrast: {} themes checked", themes.len());
}
