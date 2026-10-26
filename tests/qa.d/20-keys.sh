# shellcheck shell=bash
# Keys that STATUS once claimed and the code did not have (Ctrl+1–9), and the
# gaps against Flea/Strata/Nautilus: invert selection, zoom, a key map, new
# file, middle-click to a new tab, F3 split, and the four key presets.

say "20a · Ctrl+1–9 goes to that tab"
stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
ctrl t; ctrl t
expect "three tabs open" "d['tabCount']" "3"
vm wtype -M ctrl -k 1 -m ctrl >/dev/null 2>&1; sleep 0.6
expect "Ctrl+1 goes to the first tab" "d['activeTab']" "0"
vm wtype -M ctrl -k 3 -m ctrl >/dev/null 2>&1; sleep 0.6
expect "Ctrl+3 goes to the third tab" "d['activeTab']" "2"
vm wtype -M ctrl -k 9 -m ctrl >/dev/null 2>&1; sleep 0.6
expect "Ctrl+9 with three tabs stays put" "d['activeTab']" "2"
expect_contains "and says there is no ninth tab" "d['hintShown']" "no tab 9"
ctrl w; ctrl w

say "20b · invert selection, and Ctrl+Shift+A selects nothing"
wait_for "d['model']['settled']" "True" 20 >/dev/null
total="$(state "d['model']['count']")"
key Home; ctrl space
expect "one selected" "d['selCount']" "1"
ctrl i
expect "Ctrl+I inverts: everything else is selected" "d['selCount']" "$((total - 1))"
vm wtype -M ctrl -M shift -k a -m shift -m ctrl >/dev/null 2>&1; sleep 0.6
expect "Ctrl+Shift+A selects nothing" "d['selCount']" "0"
# The filter hides rows: inverting must never select what is not shown.
typ "/"; typ "Doc"; key Return; sleep 0.5
shown="$(state "d['model']['count']")"
ctrl i
expect "inverting under a filter selects only the rows shown" "d['selCount']" "$shown"
key Escape; key Escape; key Escape; sleep 0.4

say "20c · Ctrl+= / Ctrl+- / Ctrl+0 zoom like the rest of Omarchy"
vm wtype -M ctrl -k equal -m ctrl >/dev/null 2>&1; sleep 0.5
expect "Ctrl+= zooms in" "d['zoom']" "1.1"
vm wtype -M ctrl -k minus -m ctrl >/dev/null 2>&1; sleep 0.4
vm wtype -M ctrl -k minus -m ctrl >/dev/null 2>&1; sleep 0.5
expect "Ctrl+- zooms out" "d['zoom']" "0.9"
vm wtype -M ctrl -k 0 -m ctrl >/dev/null 2>&1; sleep 0.5
expect "Ctrl+0 resets" "d['zoom']" "1"

say "20d · ? opens a key map built from the same table the keys use"
vm wtype -M shift -k question -m shift >/dev/null 2>&1; sleep 0.8
expect "? opens the key map" "d['keyMapOpen']" "True"
rows="$(state "d['keyMapRows']")"
[[ "$rows" =~ ^[0-9]+$ && "$rows" -ge 30 ]] && ok "the key map lists $rows keys" || bad "the key map lists only '$rows' keys"
shot 20d-keymap
key Escape
expect "Escape closes it" "d['keyMapOpen']" "False"
key Down
expect "and the list has the keyboard again" "d['model']['cursor']" "1"

say "20e · Ctrl+Shift+N makes an empty file, never over an existing one"
rm -f "$FIXTURE/fresh-notes.txt"
vm wtype -M ctrl -M shift -k n -m shift -m ctrl >/dev/null 2>&1; sleep 0.8
expect "a New file prompt opens" "d['promptMode']" "newfile"
ctrl a; typ "fresh-notes.txt"; key Return; sleep 1.0
[[ -f "$FIXTURE/fresh-notes.txt" && ! -s "$FIXTURE/fresh-notes.txt" ]] \
  && ok "an empty file landed on disk" || bad "no empty fresh-notes.txt on disk"
wait_for "d['model']['cursor_name']" "fresh-notes.txt" 10 >/dev/null
expect "the cursor is on the new file" "d['model']['cursor_name']" "fresh-notes.txt"
echo "keep me" > "$FIXTURE/fresh-notes.txt"
vm wtype -M ctrl -M shift -k n -m shift -m ctrl >/dev/null 2>&1; sleep 0.8
ctrl a; typ "fresh-notes.txt"; key Return; sleep 0.8
expect "a clash keeps the prompt open" "d['promptOpen']" "True"
expect_contains "and says why" "d['promptError']" "already exists"
[[ "$(cat "$FIXTURE/fresh-notes.txt")" == "keep me" ]] && ok "the existing file is untouched" || bad "an existing file was overwritten"
key Escape; rm -f "$FIXTURE/fresh-notes.txt"; sleep 0.6

say "20f · F3 turns two panes on and off"
key F3; sleep 0.8
expect "F3 splits" "d['paneCount']" "2"
key F3; sleep 0.8
expect "F3 again unsplits" "d['paneCount']" "1"
expect "and leaves no stray tab" "d['tabCount']" "1"
key F3; sleep 0.8
typ "Pro"; key Return
wait_for "d['paneLocations'][1]" "$FIXTURE/Projects" 10 >/dev/null
key F3; sleep 0.8
expect "a split tab the user moved stays as a tab" "d['tabCount']" "2"
ctrl w; key BackSpace
wait_for "d['model']['location']" "$FIXTURE" 10 >/dev/null

say "20g · middle-click a folder opens it in a new tab (real mouse)"
if [[ "$MOUSE_OK" == 1 ]]; then
  # Enough folders that jumping to Projects scrolls the list: the click
  # point must be read after the scroll, not before (it once was not).
  for i in $(seq -w 1 40); do mkdir -p "$FIXTURE/aa-pad-$i"; done
  stop_gust; start_gust "$FIXTURE"
  wait_for "d['model']['settled']" "True" 20 >/dev/null
  typ "Proj"; sleep 1.2
  expect "cursor on Projects" "d['model']['cursor_name']" "Projects"
  read -r WX WY WW WH <<<"$(win_geo)"
  pt="$(state "' '.join(map(str, d['cursorPoint']))")"
  read -r PX PY <<<"$pt"
  mouse click $((WX + PX)) $((WY + PY)) middle; sleep 1.0
  expect "middle-click opened a tab" "d['tabCount']" "2"
  expect "and stayed on this one" "d['activeTab']" "0"
  shot 20g-middle-click
  # Background tabs load when first shown; switching to it must land there.
  vm wtype -M ctrl -k 2 -m ctrl >/dev/null 2>&1
  wait_for "d['model']['location']" "$FIXTURE/Projects" 15 >/dev/null
  expect "the new tab opens on Projects" "d['model']['location']" "$FIXTURE/Projects"
  ctrl w; sleep 0.4
  rm -rf "$FIXTURE"/aa-pad-*
else
  bad "no pointer injection: the middle-click case did NOT run"
fi

say "20h · the Windows preset: Backspace goes back, Alt+↑ goes up"
stop_gust
mkdir -p "$FIXTURE/.config/gust"
printf 'key_preset = "windows"\n' > "$FIXTURE/.config/gust/settings.toml"
start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
expect "the preset is read" "d['preset']" "windows"
typ "Proj"; sleep 1.2; key Return
wait_for "d['model']['location']" "$FIXTURE/Projects" >/dev/null
key BackSpace; sleep 0.8
expect "Backspace goes BACK in Windows" "d['model']['location']" "$FIXTURE"
vm wtype -M alt -k Right -m alt >/dev/null 2>&1; sleep 0.8
expect "Alt+→ goes forward" "d['model']['location']" "$FIXTURE/Projects"
vm wtype -M alt -k Up -m alt >/dev/null 2>&1; sleep 0.8
expect "Alt+↑ goes up" "d['model']['location']" "$FIXTURE"
stop_gust
printf 'key_preset = "mac"\n' > "$FIXTURE/.config/gust/settings.toml"
start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
typ "Proj"; sleep 1.2; key Return; sleep 0.8
expect "Enter renames in the Mac preset" "d['promptMode']" "rename"
key Escape; sleep 0.4
vm wtype -M ctrl -k Down -m ctrl >/dev/null 2>&1; sleep 0.8
expect "Ctrl+↓ opens in the Mac preset" "d['model']['location']" "$FIXTURE/Projects"
stop_gust
rm -f "$FIXTURE/.config/gust/settings.toml"
start_gust "$FIXTURE"
