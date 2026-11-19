# shellcheck shell=bash
# Shared with the focused fixture-navigation desktop regression.
say "01 · window opens, lists the fixture, uses the real Omarchy theme"
if ! start_gust; then
  printf '\ncannot continue without a window\n'; return 1
fi
ok "window present"
wait_for "d['model']['settled']" "True"
expect "listing loaded"          "d['model']['count']" "11"
expect_location "starts in the fixture" "$FIXTURE"
expect "rows view by default"    "d['view']" "rows"
expect "standard preset default" "d['preset']" "standard"
expect "hidden files off"        "d['model']['show_hidden']" "False"
expect "name sort settled"       "d['model']['settled']" "True"
expect "dirs first, then files"  "d['model']['visible'][0]" "big"
shot 01-startup
python3 "$HERE/assert_shot.py" "$OUTDIR/01-startup.png" Projects Documents items \
  --absent "This folder is empty" > "$OUTDIR/01-startup.txt" 2>&1 \
  && ok "startup screen shows the listing" || bad "startup screen (see $OUTDIR/01-startup.txt)"

say "02 · real key events move the keyboard cursor"
key Down; key Down
expect "cursor moved with Down" "d['model']['cursor']" "2"
expect_contains "cursor is on a real entry" "d['model']['cursor_name']" "Documents"

say "03 · type-ahead SELECTS in this folder and never starts a search"
# The Nautilus #1157 regression: typing must not change what the folder shows.
count_before="$(state "d['model']['count']")"
loc_before="$(state "d['model']['location']")"
typ "Pro"
expect "type-ahead selected Projects" "d['model']['cursor_name']" "Projects"
expect "listing was NOT replaced"     "d['model']['count']" "$count_before"
expect "location unchanged"           "d['model']['location']" "$loc_before"
expect "no filter was applied"        "d['model']['filter']" ""
shot 02-type-ahead

say "04 · Enter opens a folder, Backspace goes back up"
key Return
wait_location "$FIXTURE/Projects" >/dev/null
expect_location "entered Projects" "$FIXTURE/Projects"
key Return
wait_location "$FIXTURE/Projects/omarchy"
expect_location "entered a nested folder" "$FIXTURE/Projects/omarchy"
shot 03-nested
key BackSpace
wait_location "$FIXTURE/Projects" >/dev/null
expect_location "Backspace went up" "$FIXTURE/Projects"
key BackSpace
wait_location "$FIXTURE" >/dev/null
expect_location "Backspace again" "$FIXTURE"
