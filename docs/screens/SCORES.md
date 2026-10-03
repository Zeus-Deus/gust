# Visual pass (v1 scope) — scores and pairs

Every surface the v1 scope lists, shot by `tests/visual-pass.sh` (real window,
real keys and pointer, each surface asserted open before its shot) in the
default theme (tokyo-night) and the white theme:

    tests/visual-pass-run.sh tokyo-night /tmp/vp-tokyo-night
    tests/visual-pass-run.sh white /tmp/vp-white

`after/<theme>/` is the full final set (19 shots per theme). `before/<theme>/`
holds the shots of the surfaces that changed in this pass. Each shot was scored
with vision against the goal contract's §5 rubric (0–2 per criterion).

## What the first pass found, and what was fixed

| Finding (before) | Fix | Proof |
|---|---|---|
| Trash confirmation named the cursor row ("notes.txt") while the selection ("dest") was what would be trashed | Dialogs name `target_name()`: the selection, or the cursor when nothing is selected; Permissions/Owner read the same target (D46) | qa §30i (failed 2 asserts before the fix, 8/8 after) |
| Selected text in Rename/Permissions unreadable: accent text on a pale fill, 1.2:1 in tokyo-night | Selection is the accent with background-coloured text (D47) | `after/*/vp-dialog-rename.png`, vision: readable in both themes |
| White theme: menus, prompts, dialogs and the transfer card on a mid-grey `#c0c0c0` slab | A stale shell.toml now falls back to the theme background for menus and tooltips, as Omarchy's own template does (D48) | unit test `a_stale_shell_gives_menus_the_theme_background_like_omarchy_does`; vision: "white surface, not a slab" |
| Quick Look's "Space or Escape to close" drawn below the panel | Preview height subtracts every line under it | probe `quickLookCloseInside`, asserted in the visual pass |
| Context menu opened from the keyboard showed no current entry | Shift+F10/Menu starts on the first entry; the current entry gets the list's accent bar | asserted (`menuCurrent == Open`) |
| Settings' focused row barely visible | Selection fill plus accent bar, like a list row | `after/*/vp-settings.png` |
| Colour-emoji gear; ≡ used for both the menu and Rows view | Text gear ⛭; Rows view ▤ | vision: "no colour emoji; distinct glyphs" |
| Transfer card buttons cramped against each other and the edge | Same padding as the dialog buttons, inset from the edge | `after/*/vp-transfer-running.png` |
| Columns preview "1 items"; type-ahead "Selected plan.md" outliving its folder | Singular; the type-ahead status clears on a folder change | asserted in the visual pass |
| Hints cut through their letters; hint strip pushed off-window with two tabs | (D44, D45, earlier in this batch) | qa §30g, §30h |

## Final scores (after), criteria still at 1, and why they stay

Most criteria now score 2 on every surface in both themes. The ones vision
still scores 1 fall into three groups, each a recorded decision rather than an
oversight:

1. **Secondary text contrast** (metadata columns, hints, notes): vision
   estimates "likely below 4.5:1". These are *secondary* tokens, held to
   ≥ 3:1 on every surface in all 22 themes by `crates/core/tests/theme_contrast.rs`
   (D20); primary text is ≥ 4.5:1. Raising secondary text to 4.5:1 would
   erase the hierarchy the same rubric asks for. Kept (D49).
2. **Dead zones** in the main views and Quick Look: the fixture folder has
   six items and a two-line text file. A real folder fills the space; the
   views do not stretch content to fill it. Kept (D49).
3. **Design asks beyond v1** (file-type icons, a filled primary button
   style, naming the files in the transfer card, a drop shadow on menus,
   rendered Markdown): recorded in `docs/HANDOFF.md` → After v1, not built
   (scope rule).

No surface scores 0 on any criterion after the fixes.
