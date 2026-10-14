# shellcheck shell=bash
# Colour tags: Alt+1–7 by keyboard, Tag… by mouse, dots on the row, stored
# where Dolphin reads them, carried by a copy, one Ctrl+Z (DESIGN §6.25).
say "47 · colour tags"
stop_gust
TG="$FIXTURE/tags"
rm -rf "$TG"; mkdir -p "$TG/into"
printf 'a\n' > "$TG/alpha.txt"
printf 'b\n' > "$TG/beta.txt"
printf 'c\n' > "$TG/gamma.txt"
setfattr -n user.xdg.tags -v work "$TG/beta.txt" 2>/dev/null
xt() { getfattr --only-values -n user.xdg.tags "$1" 2>/dev/null; }
if ! setfattr -n user.xdg.tags -v probe "$TG/gamma.txt" 2>/dev/null; then
  bad "the fixture filesystem has no user xattrs; tags cannot be tested"
  return 0 2>/dev/null || exit 0
fi
setfattr -x user.xdg.tags "$TG/gamma.txt"

start_gust "$TG"
wait_for "d['model']['settled']" "True" 15 >/dev/null
typ "alpha"; sleep 1.2
vm wtype -M shift -k Down -m shift >/dev/null 2>&1; sleep 0.4
expect "two rows selected" "d['selCount']" "2"
vm wtype -M alt -k 1 -m alt >/dev/null 2>&1; sleep 1
[[ "$(xt "$TG/alpha.txt")" == "Red" ]] && ok "Alt+1 stored Red where Dolphin reads it" || bad "alpha: '$(xt "$TG/alpha.txt")'"
[[ "$(xt "$TG/beta.txt")" == "work,Red" ]] && ok "a word tag Gust does not know is kept" || bad "beta: '$(xt "$TG/beta.txt")'"
expect_contains "the status says what happened" "d['hintStatus']" "Red added to 2"
expect_contains "the row shows a red dot" "d['rowTags']['alpha.txt']" "red"
shot 47-tags-rows

ctrl z; sleep 1.2
[[ -z "$(xt "$TG/alpha.txt")" && "$(xt "$TG/beta.txt")" == "work" ]] \
  && ok "one Ctrl+Z puts every file's tags back exactly" || bad "undo left alpha='$(xt "$TG/alpha.txt")' beta='$(xt "$TG/beta.txt")'"
expect "Undo says so" "d['hintStatus']" "Tags put back"

# By mouse: right-click gamma, Tag…, Blue.
if [[ "$MOUSE_OK" -eq 1 ]]; then
  read -r WX WY WW WH <<<"$(win_geo)"
  read -r gx gy <<<"$(state "' '.join(map(str, d['rowPoints']['gamma.txt']))")"
  mouse click $((WX + gx)) $((WY + gy)) right; sleep 0.6
  read -r tx ty <<<"$(state "' '.join(map(str, d['menuPoints']['Tag…']))")"
  mouse click $((WX + tx)) $((WY + ty)); sleep 0.6
  read -r bx by <<<"$(state "' '.join(map(str, d['menuPoints']['Blue']))")"
  mouse click $((WX + bx)) $((WY + by)); sleep 1
  [[ "$(xt "$TG/gamma.txt")" == "Blue" ]] && ok "Tag… then Blue with the mouse" || bad "gamma: '$(xt "$TG/gamma.txt")'"
else
  bad "Tag menu mouse journey did NOT run: no pointer injection"
  setfattr -n user.xdg.tags -v Blue "$TG/gamma.txt"
fi

# A copy keeps its colour.
typ "gamma"; sleep 1.2
ctrl c
goto "$TG/into"
wait_for "d['model']['location']" "$TG/into" 10 >/dev/null
ctrl v
wait_for "d['jobState']" "Confirmed" 20 >/dev/null
[[ "$(xt "$TG/into/gamma.txt")" == "Blue" ]] && ok "the copy keeps its tag" || bad "copy tag: '$(xt "$TG/into/gamma.txt")'"
expect_contains "and shows it" "d['rowTags']['gamma.txt']" "blue"

# Alt+0 removes the colours.
vm wtype -M alt -k 0 -m alt >/dev/null 2>&1; sleep 1
[[ -z "$(xt "$TG/into/gamma.txt")" ]] && ok "Alt+0 removes the colours" || bad "after Alt+0: '$(xt "$TG/into/gamma.txt")'"
stop_gust
start_gust "$FIXTURE"
