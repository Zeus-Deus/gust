# shellcheck shell=bash
# v1 everyday features, each driven like a human (keys + real pointer) and
# asserted on the app's own state and on disk. Plus the four Flea 0.3.7
# bug checks.

pt_of() { # pt_of <probe-dict> <key>  -> "X Y" in window coordinates, or ""
  state "' '.join(map(str, d['$1'].get('''$2''', [])))"
}
wclick() { # wclick <button> "X Y"  (window coordinates)
  local x y; read -r x y <<<"$2"; read -r WX WY WW WH <<<"$(win_geo)"
  mouse click $((WX + x)) $((WY + y)) "$1"
}

say "30a · paste an image from the clipboard as a file, and Ctrl+Z removes it"
d30="$FIXTURE/paste30"; rm -rf "$d30"; mkdir -p "$d30"
python3 - "$OUTDIR/30-clip.png" <<'PY'
import sys, zlib, struct
w, h = 24, 16
rows = b""
for y in range(h):
    rows += b"\x00" + b"".join(bytes([(x * 10) % 256, (y * 15) % 256, 160]) for x in range(w))
def chunk(t, d): return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")
open(sys.argv[1], "wb").write(png)
PY
stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
goto "$d30" || bad "could not open $d30"
vm sh -c "wl-copy --type image/png < '$OUTDIR/30-clip.png'"; sleep 0.8
ctrl v
for _ in $(seq 1 25); do ls "$d30"/Pasted\ image*.png >/dev/null 2>&1 && break; sleep 0.3; done
got="$(ls "$d30" | head -1)"
[[ "$got" == Pasted\ image\ *.png ]] && ok "Ctrl+V made '$got'" || bad "no pasted image file (dir: $(ls "$d30"))"
cmp -s "$d30/$got" "$OUTDIR/30-clip.png" && ok "the file holds exactly the clipboard's bytes" || bad "bytes differ"
wait_for "d['model']['cursor_name']" "$got" 10 >/dev/null
expect "the cursor is on the new file" "d['model']['cursor_name']" "$got"
expect_contains "the status says how to undo it" "d['hintShown']" "Saved Pasted image"
expect_contains "and how to undo it" "d['hintShown']" "Ctrl+Z"
shot 30a-paste-image
ctrl z; sleep 1.0
[[ -z "$(ls "$d30")" ]] && ok "Ctrl+Z removed the pasted file" || bad "after undo: $(ls "$d30")"
# The palette entry does the same thing.
palette_run "Paste image as file"; sleep 1.5
[[ "$(ls "$d30" | wc -l)" == 1 ]] && ok "the palette's Paste image as file works too" || bad "palette paste: $(ls "$d30")"
vm wl-copy --clear >/dev/null 2>&1
# With no image (and no files) on the clipboard, nothing is written.
rm -f "$d30"/*; ctrl v; sleep 1.2
[[ -z "$(ls "$d30")" ]] && ok "an empty clipboard writes nothing" || bad "wrote $(ls "$d30")"

say "30b · Owner for several files, by mouse, and Ctrl+Z puts it back"
d30b="$FIXTURE/owner30"; rm -rf "$d30b"; mkdir -p "$d30b"
for f in o1.txt o2.txt keep.txt; do printf x > "$d30b/$f"; done
me_group="$(id -gn)"
if id -Gn | tr ' ' '\n' | grep -qx wheel; then
  goto "$d30b" || bad "could not open $d30b"
  wait_for "d['model']['count']" "3" 10 >/dev/null
  typ "o1"; sleep 1.2; ctrl space; key Down; ctrl space
  expect "two selected" "d['selCount']" "2"
  key Up
  if [[ "$MOUSE_OK" == 1 ]]; then
    wclick right "$(pt_of rowPoints o1.txt)"; sleep 0.6
    expect "a right-click inside the selection opens the menu" "d['menuOpen']" "True"
    wclick left "$(pt_of menuPoints 'Owner…')"; sleep 0.8
  else
    bad "no pointer injection: the Owner menu click did NOT run"; palette_run "Owner"
  fi
  expect "the Owner prompt opened" "d['promptMode']" "owner"
  shot 30b-owner-prompt
  ctrl a; typ ":wheel"; key Return; sleep 1.0
  [[ "$(stat -c %G "$d30b/o1.txt")" == wheel && "$(stat -c %G "$d30b/o2.txt")" == wheel ]] \
    && ok "both files now belong to group wheel" || bad "groups: $(stat -c %G "$d30b"/o1.txt "$d30b"/o2.txt)"
  [[ "$(stat -c %G "$d30b/keep.txt")" == "$me_group" ]] && ok "the unselected file was not touched" || bad "keep.txt changed"
  ctrl z; sleep 1.0
  [[ "$(stat -c %G "$d30b/o1.txt")" == "$me_group" && "$(stat -c %G "$d30b/o2.txt")" == "$me_group" ]] \
    && ok "Ctrl+Z gave both back to $me_group" || bad "after undo: $(stat -c %G "$d30b"/o1.txt "$d30b"/o2.txt)"
  # A new OWNER needs administrator rights: refused in words, nothing changes.
  palette_run "Owner"; sleep 0.8
  ctrl a; typ "root"; key Return; sleep 0.8
  expect "the prompt stays open" "d['promptOpen']" "True"
  expect_contains "and says it needs administrator rights" "d['promptError']" "administrator"
  key Escape
  [[ "$(stat -c %U "$d30b/o1.txt")" == "$(id -un)" ]] && ok "nothing changed on disk" || bad "owner changed"
else
  bad "the test user is in no second group (wheel): the group change did NOT run"
fi

say "30c · Link here: drop with the right button, choose Link here"
d30c="$FIXTURE/link30"; rm -rf "$d30c"; mkdir -p "$d30c/shelf"; printf 'note\n' > "$d30c/note.txt"
goto "$d30c" || bad "could not open $d30c"
wait_for "d['model']['count']" "2" 10 >/dev/null
key Home; key Down; key Up
if [[ "$MOUSE_OK" == 1 ]]; then
  read -r WX WY WW WH <<<"$(win_geo)"
  read -r ax ay <<<"$(pt_of rowPoints note.txt)"; read -r bx by <<<"$(pt_of rowPoints shelf)"
  mouse rdrag $((WX + ax)) $((WY + ay)) $((WX + bx)) $((WY + by)); sleep 0.8
  expect "releasing a right-button drag opens the drop menu" "d['menuOpen']" "True"
  expect_contains "it offers Link here" "' | '.join(d['menuItems'])" "Link here"
  expect_contains "the hint says what to do now" "d['hintShown']" "Esc cancels"
  shot 30c-drop-menu
  wclick left "$(pt_of menuPoints 'Link here')"; sleep 1.0
  [[ -L "$d30c/shelf/note.txt" && "$(readlink "$d30c/shelf/note.txt")" == "$d30c/note.txt" ]] \
    && ok "a link to note.txt landed in shelf" || bad "no link in shelf ($(ls -la "$d30c/shelf"))"
  [[ -f "$d30c/note.txt" && ! -L "$d30c/note.txt" ]] && ok "the original stayed where it was" || bad "the original moved"
  ctrl z; sleep 1.0
  [[ ! -e "$d30c/shelf/note.txt" && ! -L "$d30c/shelf/note.txt" ]] && ok "Ctrl+Z removed the link" || bad "the link is still there"
  [[ "$(cat "$d30c/note.txt")" == note ]] && ok "and never touched the target" || bad "target changed"
  # Escape on the drop menu does nothing at all.
  read -r ax ay <<<"$(pt_of rowPoints note.txt)"; read -r bx by <<<"$(pt_of rowPoints shelf)"
  mouse rdrag $((WX + ax)) $((WY + ay)) $((WX + bx)) $((WY + by)); sleep 0.8
  key Escape; sleep 0.5
  [[ -z "$(ls "$d30c/shelf")" ]] && ok "Escape on the drop menu changes nothing" || bad "something landed: $(ls "$d30c/shelf")"
  # A plain left drag on the same drive moves (D60).
  read -r ax ay <<<"$(pt_of rowPoints note.txt)"; read -r bx by <<<"$(pt_of rowPoints shelf)"
  mouse drag $((WX + ax)) $((WY + ay)) $((WX + bx)) $((WY + by)); sleep 1.5
  [[ -f "$d30c/shelf/note.txt" && ! -L "$d30c/shelf/note.txt" && ! -e "$d30c/note.txt" ]] \
    && ok "a left drag on one drive moves" || bad "left drag: $(ls -la "$d30c" "$d30c/shelf")"
else
  bad "no pointer injection: Link here did NOT run"
fi

say "30d · Pause and Resume a running copy from the transfer card"
d30d="/var/tmp/gust-qa-pause"; rm -rf "$d30d"; mkdir -p "$d30d/src" "$d30d/dest"
python3 -c "
import os
for i in range(2000): open('$d30d/src/f%05d' % i, 'wb').write(b'x' * 64)
"
goto "$d30d" || bad "could not open $d30d"
wait_for "d['model']['count']" "2" 10 >/dev/null
typ "src"; sleep 1.2; ctrl c; sleep 0.4
typ "dest"; sleep 1.2; key Return
wait_for "d['model']['location']" "$d30d/dest" 10 >/dev/null
ctrl v
wait_for "d['jobRunning']" "True" 20 >/dev/null
if [[ "$MOUSE_OK" == 1 && "$(state "d['jobRunning']")" == True ]]; then
  for _ in $(seq 1 20); do [[ "$(state "d['jobPausePoint'] is not None")" == True ]] && break; sleep 0.2; done
  sleep 1.0   # the card slides in; read the button where it settled
  wclick left "$(state "' '.join(map(str, d['jobPausePoint'] or []))")"
  if wait_for "d['jobPaused']" "True" 15; then
    ok "clicking Pause paused the copy"
    sleep 1.0; a="$(find "$d30d/dest" -type f | wc -l)"; sleep 1.5; b="$(find "$d30d/dest" -type f | wc -l)"
    [[ "$a" == "$b" ]] && ok "no file arrived while paused ($a of 2000)" || bad "files kept arriving while paused ($a → $b)"
    expect "the card says Paused, not finished" "d['jobState']" "Running"
    expect_contains "the hint strip says it is paused" "d['hintShown']" "Paused"
    shot 30d-paused-card
    sleep 0.4
    wclick left "$(state "' '.join(map(str, d['jobPausePoint'] or []))")"
    wait_for "d['jobPaused']" "False" 15 >/dev/null && ok "clicking Resume resumed it" || bad "Resume did not take"
    wait_for "d['jobRunning']" "False" 600 >/dev/null
    n="$(find "$d30d/dest/src" -type f | wc -l)"
    [[ "$n" == 2000 ]] && ok "the copy finished with all 2000 files" || bad "copied $n of 2000"
    [[ "$(find "$d30d/dest" -name '*staging*' | wc -l)" == 0 ]] && ok "no staging debris" || bad "staging debris left"
  else
    jr="$(state "d['jobRunning']")"
    [[ "$jr" == False ]] && bad "the copy finished before Pause could be clicked (make the fixture bigger)" \
      || bad "Pause did not take (jobPaused=$(state "d['jobPaused']"))"
  fi
else
  bad "the pause case did NOT run (mouse=$MOUSE_OK, running=$(state "d['jobRunning']"))"
fi
rm -rf "$d30d"

say "30e · copied and cut rows are marked in the listing"
d30e="$FIXTURE/marks30"; rm -rf "$d30e"; mkdir -p "$d30e"
for f in a.txt b.txt c.txt; do printf x > "$d30e/$f"; done
goto "$d30e" || bad "could not open $d30e"
wait_for "d['model']['count']" "3" 10 >/dev/null
typ "a.t"; sleep 1.2; ctrl c; sleep 0.6; key Down
expect "one row is marked after Ctrl+C" "d['model']['marked']" "1"
typ "b.t"; sleep 1.2; ctrl space; key Down; ctrl space; ctrl x; sleep 0.6; key Up
expect "a cut replaces the copy: two rows marked" "d['model']['marked']" "2"
shot 30e-marks
key Escape; key Escape; sleep 0.4
expect "Escape gives the clipboard up and the marks go" "d['model']['marked']" "0"

say "30f · bug checks from Flea 0.3.7"
# Folder sizes are worked out only where they are drawn.
d30f="$FIXTURE/sizes30"; rm -rf "$d30f"; mkdir -p "$d30f"
python3 -c "
import os
for i in range(400):
    os.makedirs('$d30f/d%03d' % i); open('$d30f/d%03d/x' % i, 'w').write('x')
"
goto "$d30f" || bad "could not open $d30f"
wait_for "d['model']['settled']" "True" 20 >/dev/null; sleep 2.5
key Down; key Up; sleep 0.5
sz="$(state "d['model']['sized']")"
[[ "$sz" =~ ^[0-9]+$ && "$sz" -gt 0 && "$sz" -lt 150 ]] \
  && ok "only folders near the screen were measured ($sz of 400)" || bad "measured $sz of 400 folders"
key End; sleep 2.5; key Up; key Down; sleep 0.5
sz2="$(state "d['model']['sized']")"
[[ "$sz2" =~ ^[0-9]+$ && "$sz2" -gt "$sz" ]] && ok "scrolling to the end measured those too ($sz2)" || bad "after scrolling: $sz2"
rm -rf "$d30f"
# A drive with no filesystem leaves no dead sidebar row (this VM has an empty DVD drive).
[[ "$(state "any('nothing in the drive' in x or 'no readable filesystem' in x for x in d['devices'])")" == False ]] \
  && ok "no unusable drive row in the sidebar" || bad "devices: $(state "d['devices']")"
if [[ "$MOUSE_OK" == 1 ]]; then
  # Arrow keys reach the list again after clicking a sidebar place.
  wclick left "$(pt_of placePoints Documents)"; sleep 0.8
  wait_for "d['model']['location']" "$FIXTURE/Documents" 10 >/dev/null
  key Down; key Down
  expect "after a sidebar click, arrows move the list cursor" "d['model']['cursor']" "2"
  # Clicking the folder already shown does not reload it.
  g1="$(state "d['model']['generation']")"
  wclick left "$(pt_of placePoints Documents)"; sleep 0.8
  key Up; key Down
  expect "clicking the folder on screen did not reload it" "d['model']['generation']" "$g1"
  expect "and the cursor stayed where it was" "d['model']['cursor']" "2"
  shot 30f-sidebar
else
  bad "no pointer injection: the sidebar bug checks did NOT run"
fi

say "30g · restore tabs on start (Settings → Tabs at start)"
rm -f "$FIXTURE/.local/state/gust/tabs.toml"
stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
ctrl comma; wait_for "d['settingsOpen']" "True" 8 >/dev/null
key Down; key Down; key Down; key Down; key Right; sleep 0.6
expect "Tabs at start: Reopen last" "str(d['settings'].get('restore_tabs'))" "True"
shot 30g-settings
key Escape; sleep 0.4
ctrl t; sleep 0.8
goto "$FIXTURE/Documents" || bad "could not open Documents in the new tab"
palette_run "View: Grid"; sleep 2.5   # the tab list is saved 0.6 s after a change
expect "two tabs open" "d['tabCount']" "2"
expect "with two tabs the hint strip is still inside the window" "d['hintsOverflow'] <= 0" "True"
stop_gust; start_gust "$FIXTURE"
wait_for "d['tabCount']" "2" 20 >/dev/null
expect "both tabs came back" "d['tabCount']" "2"
expect "the second tab was the one on screen" "d['activeTab']" "1"
wait_for "d['activeTabPath']" "$FIXTURE/Documents" 10 >/dev/null
expect "and it is in Documents again" "d['activeTabPath']" "$FIXTURE/Documents"
expect "in Grid, as it was left" "d['view']" "grid"
expect "the restored window still shows its hint strip" "d['hintsOverflow'] <= 0" "True"
m="$(stat -c %a "$FIXTURE/.local/state/gust/tabs.toml" 2>/dev/null)"
[[ "$m" == 600 ]] && ok "tabs.toml is owner-only" || bad "tabs.toml mode is '$m'"
shot 30g-restored-tabs
ctrl w; sleep 0.6
ctrl comma; wait_for "d['settingsOpen']" "True" 8 >/dev/null
key Down; key Down; key Down; key Down; key Left; sleep 0.6
expect "Tabs at start back to Fresh" "str(d['settings'].get('restore_tabs'))" "False"
key Escape
stop_gust; rm -f "$FIXTURE/.local/state/gust/tabs.toml"; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
expect "with Fresh, a start opens one tab" "d['tabCount']" "1"

say "30i · a dialog names what it will act on, not the row the cursor wandered to"
d30i="$FIXTURE/target30"; rm -rf "$d30i"; mkdir -p "$d30i/keep"; printf 'abc' > "$d30i/notes.txt"; chmod 600 "$d30i/notes.txt"
goto "$d30i" || bad "could not open $d30i"
wait_for "d['model']['count']" "2" 10 >/dev/null
key Home; ctrl space
expect "keep is selected" "d['selCount']" "1"
typ "notes"; sleep 1.2
expect "the cursor wandered to notes.txt" "d['model']['cursor_name']" "notes.txt"
key Delete; wait_for "d['confirmOpen']" "True" 10 >/dev/null
expect_contains "the Trash question names the selected folder" "d['confirmBody']" "\"keep\""
[[ "$(state "'notes.txt' in d['confirmBody']")" == False ]] && ok "and not the cursor row" || bad "it names notes.txt: $(state "d['confirmBody']")"
shot 30i-trash-names-selection
key Escape; sleep 0.5
[[ -d "$d30i/keep" && -f "$d30i/notes.txt" ]] && ok "Cancel changed nothing" || bad "something moved"
palette_run "Permissions…"; sleep 0.8
expect_contains "Permissions shows the selected folder's mode" "d['promptHint']" "Now: rwxr-xr-x"
key Escape; sleep 0.4
# Leave the folder before removing it: the next step acts on the folder shown.
goto "$FIXTURE" || bad "could not return to $FIXTURE"
rm -rf "$d30i"

say "30h · open as administrator: a separate, clearly marked window"
printf 'x\n' > "$FIXTURE/admin-note.txt"
ADMIN_STATE="${STATE%.json}-admin.json"; rm -f "$ADMIN_STATE"
palette_run "Open as administrator"
for _ in $(seq 1 40); do [[ -s "$ADMIN_STATE" ]] && break; sleep 0.3; done
astate() { python3 -c "
import json
try: d=json.load(open('$ADMIN_STATE'))
except Exception: print('NO_STATE'); raise SystemExit
print($1)"; }
if [[ -s "$ADMIN_STATE" ]]; then
  ok "polkit started the administrator window"
  [[ "$(astate "d['adminMode']")" == True ]] && ok "it knows it is the administrator window" || bad "adminMode is $(astate "d['adminMode']")"
  [[ "$(astate "d['model']['location']")" == "$FIXTURE" ]] && ok "it opened the folder that was on screen" || bad "admin location $(astate "d['model']['location']")"
  apid="$(pgrep -u root -x gust | head -1)"
  [[ -n "$apid" ]] && ok "it runs as root (pid $apid), the main window does not" || bad "no root gust process"
  pgrep -u "$(id -u)" -x gust >/dev/null && ok "the user's own window is still open" || bad "the main window went away"
  sleep 1.0
  vm hyprctl dispatch 'hl.dsp.focuswindow({ "title:^Administrator" })' >/dev/null 2>&1; sleep 0.5
  shot 30h-admin-window
  # It manages files but never opens them: Enter on a file is refused.
  typ "admin-n"; sleep 1.2; key Return; sleep 0.8
  [[ "$(astate "d['model']['cursor_name']")" == admin-note.txt ]] && ok "the keyboard reaches the administrator window" || bad "admin cursor: $(astate "d['model']['cursor_name']")"
  # Shown where every refusal to run a file is shown: the hint strip.
  [[ "$(astate "d['hintShown']")" == *"never opened or run"* ]] && ok "Enter on a file is refused there, in words" || bad "admin hint: $(astate "d['hintShown']")"
  [[ "$(astate "d['model']['location']")" == "$FIXTURE" ]] && ok "and nothing was opened" || bad "admin moved to $(astate "d['model']['location']")"
  [[ "$(astate "d['hintsCut']")" == 0 ]] && ok "no key hint is cut in half beside the refusal" || bad "cut hints: $(astate "d['hintsCut']")"
  [[ "$(state "d['hintsCut']")" == 0 ]] && ok "nor in the half-width main window" || bad "main window cut hints: $(state "d['hintsCut']")"
  shot 30h-admin-refusal
  sudo -n pkill -u root -x gust 2>/dev/null; sleep 0.8
  pgrep -u root -x gust >/dev/null && bad "the administrator window did not close" || ok "the administrator window closed"
  vm hyprctl dispatch 'hl.dsp.focuswindow({ "class:gust" })' >/dev/null 2>&1; sleep 0.5
else
  bad "no administrator window appeared: on a disposable VM run tests/admin-fixture.sh start (polkit rule), or approve the prompt"
fi
rm -f "$FIXTURE/admin-note.txt"
stop_gust; rm -rf "$d30" "$d30b" "$d30c" "$d30e"; start_gust "$FIXTURE"
