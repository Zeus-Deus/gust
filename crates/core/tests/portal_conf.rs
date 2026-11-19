//! Opting in to Gust for other apps' Open/Save dialogs.
//!
//! xdg-desktop-portal picks a FileChooser backend from `portals.conf`. Gust
//! edits ONLY the FileChooser line in the user's own
//! `~/.config/xdg-desktop-portal/hyprland-portals.conf`, remembering what was
//! there, and puts it back exactly on opt-out.

use gust_core::portal_conf::{
    add_rule, claim, has_rule, release, remove_rule, same_config, CHOOSER_KEY, RULE_BEGIN,
};

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

#[test]
fn the_float_rule_is_one_marked_block_that_comes_off_exactly() {
    let user = "-- my rules\no.window(\"mpv\", { float = true })";
    let on = add_rule(user);
    assert!(has_rule(&on));
    assert!(on.contains("gust-chooser"));
    assert!(on.contains("floating-window"));
    // Adding twice changes nothing.
    assert_eq!(add_rule(&on), on);
    assert_eq!(remove_rule(&on), format!("{user}\n"));
}

#[test]
fn removing_the_float_rule_leaves_a_file_without_one_alone() {
    let user = "o.window(\"mpv\", { float = true })\n";
    assert_eq!(remove_rule(user), user);
}

#[test]
fn a_block_without_its_end_marker_keeps_everything_after_it() {
    let text = format!("before\n{RULE_BEGIN}\no.window(\"gust-chooser\", {{}})\nmine = true\n");
    assert_eq!(remove_rule(&text), text);
    assert!(has_rule(&remove_rule(&text)));
}

#[test]
fn lines_after_the_block_survive_removal() {
    let on = add_rule("before\n") + "after\n";
    assert_eq!(remove_rule(&on), "before\nafter\n");
}
