//! Omarchy key bindings: Super+Shift+F opens Gust instead of Nautilus.
//!
//! Omarchy's documented way to change a default binding is to unbind it and
//! bind it again in the user's own `~/.config/hypr/bindings.lua`. Gust adds
//! one clearly marked block there, only when asked, and removes exactly
//! that block again — every other line of the user's file is left alone.

use gust_core::omarchy_keys::{add_block, has_block, remove_block, BLOCK_BEGIN};

const USER_FILE: &str =
    "-- my stuff\no.bind(\"SUPER + SHIFT + R\", \"SSH\", \"alacritty -e ssh box\")\n";

#[test]
fn adding_appends_one_marked_block_with_both_bindings() {
    let out = add_block(USER_FILE, "/usr/bin/gust");
    assert!(
        out.starts_with(USER_FILE),
        "the user's lines come first, untouched"
    );
    assert!(has_block(&out));
    assert!(out.contains("hl.unbind(\"SUPER + SHIFT + F\")"));
    assert!(out.contains("hl.unbind(\"SUPER + ALT + SHIFT + F\")"));
    assert!(out.contains("o.bind(\"SUPER + SHIFT + F\", \"File manager\""));
    assert!(
        out.contains("omarchy-cmd-terminal-cwd"),
        "the cwd variant follows the terminal"
    );
}

#[test]
fn adding_twice_keeps_one_block() {
    let once = add_block(USER_FILE, "/usr/bin/gust");
    assert_eq!(add_block(&once, "/usr/bin/gust"), once);
}

#[test]
fn removing_gives_back_the_exact_original() {
    let with = add_block(USER_FILE, "/usr/bin/gust");
    assert_eq!(remove_block(&with), USER_FILE);
    assert_eq!(
        remove_block(USER_FILE),
        USER_FILE,
        "nothing of ours: nothing changes"
    );
}

#[test]
fn lines_the_user_added_inside_the_block_do_not_survive_but_nothing_outside_is_touched() {
    let with = add_block(USER_FILE, "/usr/bin/gust");
    let edited = format!("{with}-- after\n");
    assert_eq!(remove_block(&edited), format!("{USER_FILE}-- after\n"));
}

#[test]
fn a_file_without_a_trailing_newline_is_handled() {
    let f = "-- no newline at end";
    let with = add_block(f, "/usr/bin/gust");
    assert!(with.starts_with("-- no newline at end\n"));
    assert_eq!(remove_block(&with), "-- no newline at end\n");
}

#[test]
fn a_program_path_is_quoted_for_lua_and_the_shell() {
    let out = add_block("", "/opt/my \"odd\" apps/gust");
    assert!(
        !out.contains("/opt/my \"odd\""),
        "raw quotes would break the Lua string: {out}"
    );
    assert!(out.contains(BLOCK_BEGIN));
}
