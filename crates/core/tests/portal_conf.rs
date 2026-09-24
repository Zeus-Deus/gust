//! Opting in to Gust for other apps' Open/Save dialogs.
//!
//! xdg-desktop-portal picks a FileChooser backend from `portals.conf`. Gust
//! edits ONLY the FileChooser line in the user's own
//! `~/.config/xdg-desktop-portal/hyprland-portals.conf`, remembering what was
//! there, and puts it back exactly on opt-out.

use gust_core::portal_conf::{claim, release, same_config, CHOOSER_KEY};

const SYSTEM_DEFAULT: &str = "[preferred]\ndefault=hyprland;gtk\n";

#[test]
fn claiming_with_no_user_file_starts_from_the_system_default() {
    let (next, previous) = claim(None, SYSTEM_DEFAULT);
    assert!(next.contains("default=hyprland;gtk"), "{next}");
    assert!(next.contains(&format!("{CHOOSER_KEY}=gust")), "{next}");
    assert_eq!(previous, None);
}

#[test]
fn claiming_keeps_every_other_line_and_remembers_the_old_chooser() {
    let user = "[preferred]\ndefault=hyprland;gtk\norg.freedesktop.impl.portal.FileChooser=kde\norg.freedesktop.impl.portal.Screenshot=hyprland\n";
    let (next, previous) = claim(Some(user), SYSTEM_DEFAULT);
    assert_eq!(previous.as_deref(), Some("kde"));
    assert!(next.contains("org.freedesktop.impl.portal.Screenshot=hyprland"));
    assert!(next.contains("FileChooser=gust"));
    assert!(!next.contains("FileChooser=kde"));
}

#[test]
fn releasing_restores_the_previous_chooser_or_removes_our_line() {
    let user = "[preferred]\ndefault=hyprland;gtk\norg.freedesktop.impl.portal.FileChooser=kde\n";
    let (claimed, prev) = claim(Some(user), SYSTEM_DEFAULT);
    assert_eq!(release(&claimed, prev.as_deref()), user);

    let (claimed2, prev2) = claim(Some(SYSTEM_DEFAULT), SYSTEM_DEFAULT);
    assert_eq!(release(&claimed2, prev2.as_deref()), SYSTEM_DEFAULT);
}

#[test]
fn releasing_leaves_a_chooser_someone_else_set_later() {
    let later = "[preferred]\ndefault=hyprland;gtk\norg.freedesktop.impl.portal.FileChooser=termfilechooser\n";
    assert_eq!(release(later, None), later);
}

#[test]
fn a_file_without_a_preferred_section_gets_one() {
    let (next, _) = claim(Some("# nothing here\n"), SYSTEM_DEFAULT);
    assert!(next.contains("[preferred]\n"));
    assert!(next.contains("FileChooser=gust"));
}

#[test]
fn a_file_gust_created_is_recognised_as_unchanged_even_without_a_final_newline() {
    // The real /usr/share/xdg-desktop-portal/hyprland-portals.conf on
    // Omarchy 4.0.3 has no trailing newline. Found live: `--file-dialogs off`
    // left Gust's copy behind because the comparison was byte-exact.
    let system = "[preferred]\ndefault=hyprland;gtk";
    let (claimed, prev) = claim(None, system);
    let released = release(&claimed, prev.as_deref());
    assert!(same_config(&released, system));
    assert!(!same_config(&released, "[preferred]\ndefault=gtk"));
}
