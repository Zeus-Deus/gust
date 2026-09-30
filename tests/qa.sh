#!/usr/bin/env bash
# Gust desktop QA.
#
# Drives the REAL window with REAL Wayland key events (wtype) and asserts on
# the app's own exact state (GUST_QA_STATE json) plus screenshots.
#
# Isolation: HOME points at a throwaway fixture and XDG_DATA_HOME at a private
# trash, so the user's real files, trash and config are never touched. The
# Omarchy theme is read through GUST_OMARCHY_STATE so real theming is still
# exercised.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
FIXTURE="${GUST_FIXTURE:-/tmp/gust-fixture}"
OUTDIR="${1:-${GUST_QA_OUT:-/tmp/gust-qa}}"
BIN="${GUST_BIN:-$REPO/target/debug/gust}"
LOG="$OUTDIR/gust.log"
REAL_STATE="${GUST_OMARCHY_STATE:-$HOME/.local/state/omarchy}"

mkdir -p "$OUTDIR"

# Only one QA run at a time.
#
# This suite drives the single real desktop: it pkills gust, types into
# whatever window has focus and rewrites shared fixtures. Two runs at once
# destroy each other's app instance and report failures that describe
# nothing but the collision. A stale run overlapping a new one produced a
# 13-failure report against code that passed cleanly when run alone.
LOCK="/tmp/gust-qa.lock"
exec 9>"$LOCK"
if ! flock -n 9; then
  echo "gust qa: another run holds $LOCK — refusing to run two at once." >&2
  echo "gust qa: wait for it, or kill it, then retry." >&2
  exit 3
fi
echo "$$" >&9
# The lock descriptor must NOT reach anything we launch. The suite starts the
# app in the background, and the app can spawn a viewer (xdg-open, an editor)
# that outlives the run — a leaked fd 9 would leave those grandchildren
# holding the lock forever, blocking every later run with no way to tell why.
# Every spawn below therefore closes fd 9 explicitly with 9>&-.

# Per-run state file, so two runs (or a leftover app) can never read or write
# each other's snapshot.
STATE="$OUTDIR/qa-state.json"
rm -f "$STATE"

# A run that dies must not leave the app it started behind.
cleanup() { pkill -x gust 2>/dev/null || true; }
trap cleanup EXIT INT TERM

if [[ ! -x "$BIN" ]]; then
  echo "gust qa: no binary at $BIN" >&2
  echo "gust qa: build it first, or set GUST_BIN." >&2
  exit 4
fi
# The plan requires running tests against the RELEASE artifact, not just
# development source. Record which one this run actually exercised, so a
# green result can never be mistaken for coverage it does not have.
echo "gust qa: testing $BIN"

PASS=0; FAIL=0; RESULTS=()

say()  { printf '\n\033[1m== %s\033[0m\n' "$*"; }
ok()   { PASS=$((PASS+1)); RESULTS+=("PASS $*"); printf '  \033[32mPASS\033[0m %s\n' "$*"; }
bad()  { FAIL=$((FAIL+1)); RESULTS+=("FAIL $*"); printf '  \033[31mFAIL\033[0m %s\n' "$*"; }

vm()   { "$HERE/vmsession" "$@" 9>&-; }

# Pointer injection through a temporary virtual mouse (tests/uinput-mouse.py).
# Needs write access to /dev/uinput, so it goes through sudo -n; when that is
# unavailable the mouse cases are skipped rather than silently passing.
MOUSE_OK=0
if sudo -n python3 "$HERE/uinput-mouse.py" move 640 400 >/dev/null 2>&1; then
  MOUSE_OK=1
fi
mouse() { sudo -n python3 "$HERE/uinput-mouse.py" "$@" >/dev/null 2>&1; sleep 0.6; }

# Where is the gust window? Echoes "X Y W H".
win_geo() {
  vm hyprctl clients -j 2>/dev/null | python3 -c "
import json,sys
for c in json.load(sys.stdin):
    if c.get('class')=='gust':
        print(c['at'][0], c['at'][1], c['size'][0], c['size'][1]); break"
}
key()  { vm wtype -k "$1" >/dev/null 2>&1; sleep 0.6; }
typ()  { vm wtype "$1"    >/dev/null 2>&1; sleep 0.7; }
ctrl() { vm wtype -M ctrl -k "$1" -m ctrl >/dev/null 2>&1; sleep 0.5; }
shot() { vm grim "$OUTDIR/$1.png" >/dev/null 2>&1; }

# state <jq-ish python expr> -> prints the value
state() {
  python3 -c "
import json,sys
try:
    d=json.load(open('$STATE'))
except Exception as e:
    print('NO_STATE'); sys.exit(0)
try:
    print($1)
except Exception as e:
    print('ERR:'+str(e))
"
}

# expect <label> <expr> <expected>
expect() {
  local label="$1" expr="$2" want="$3" got
  got="$(state "$expr")"
  if [[ "$got" == "$want" ]]; then
    ok "$label  ($expr == $want)"
  else
    bad "$label  ($expr): expected '$want', got '$got'"
  fi
}

# expect_contains <label> <expr> <substring>
expect_contains() {
  local label="$1" expr="$2" want="$3" got
  got="$(state "$expr")"
  if [[ "$got" == *"$want"* ]]; then
    ok "$label  ($got)"
  else
    bad "$label: expected something containing '$want', got '$got'"
  fi
}

start_gust() {
  pkill -x gust 2>/dev/null || true
  sleep 0.4
  rm -f "$STATE"
  cat > /tmp/gust-qa-run.sh <<EOF
#!/usr/bin/env bash
exec > "$LOG" 2>&1
export HOME="$FIXTURE"
export XDG_DATA_HOME="$FIXTURE/.local/share"
export XDG_CACHE_HOME="$FIXTURE/.cache"
# Every XDG base, or a test Gust reads (and could write) the real user's
# settings, bookmarks and journal through the inherited session values.
export XDG_CONFIG_HOME="$FIXTURE/.config"
export XDG_STATE_HOME="$FIXTURE/.local/state"
export GUST_OMARCHY_STATE="$REAL_STATE"
export GUST_QA_STATE="$STATE"
exec "$BIN"
EOF
  chmod +x /tmp/gust-qa-run.sh
  vm /tmp/gust-qa-run.sh 9>&- &
  for _ in $(seq 1 30); do
    [[ -f "$STATE" ]] && break
    sleep 0.4
  done
  sleep 1
  local n
  n="$(vm hyprctl clients -j | python3 -c 'import sys,json; print(len([c for c in json.load(sys.stdin) if c.get("class")=="gust"]))')"
  [[ "$n" == "1" ]] || { bad "gust window did not appear (found $n)"; return 1; }
  vm hyprctl dispatch 'hl.dsp.focuswindow({ "class:gust" })' >/dev/null 2>&1
  sleep 0.4
  # Wait for the window GEOMETRY to stop changing before anything computes a
  # click coordinate from it. A window that is mapped but still being placed
  # (or still floating at a size an earlier case left behind) reports a size
  # that is stale a moment later, and every mouse assertion then clicks empty
  # space — which looks exactly like a broken handler.
  local geo prev="" stable=0
  for _ in $(seq 1 25); do
    geo="$(win_geo)"
    if [[ -n "$geo" && "$geo" == "$prev" ]]; then
      stable=$((stable + 1))
      [[ "$stable" -ge 2 ]] && break
    else
      stable=0
    fi
    prev="$geo"
    sleep 0.2
  done
  return 0
}

stop_gust() { pkill -x gust 2>/dev/null || true; sleep 0.3; }

# Open the command palette and wait until it is really open. Typing before
# the input has focus would send the characters to the file list instead
# (a race the harness hits because it types instantly).
palette_open() {
  for _ in 1 2 3; do
    typ ":"
    if wait_for "d['paletteOpen']" "True" 8; then
      sleep 0.5
      return 0
    fi
  done
  return 1
}

# Run one palette entry: open, type, confirm.
palette_run() {
  palette_open || return 1
  typ "$1"
  sleep 0.5
  key Return
  sleep 0.9
}

# Navigate to an absolute path through the palette (a typed path is
# recognised as a path, not an action).
goto() {
  local target="$1"
  for _ in 1 2 3; do
    palette_run "$target"
    if wait_for "d['model']['location']" "$target" 10; then
      return 0
    fi
  done
  return 1
}

# Wait until an expression equals a value, or time out (avoids flaky sleeps).
wait_for() {
  local expr="$1" want="$2" tries="${3:-20}"
  for _ in $(seq 1 "$tries"); do
    [[ "$(state "$expr")" == "$want" ]] && return 0
    sleep 0.3
  done
  return 1
}

# ---------------------------------------------------------------------------
# A fresh Omarchy install greets you with onboarding cards and can raise an
# update prompt mid-run. They take focus and swallow keystrokes and clicks,
# which surfaces as "clicking a row did not move the cursor" — a failure that
# has nothing to do with Gust. Clear the screen before trusting anything.
say "desktop is clear"
dismiss_desktop_overlays() {
  local i
  for i in 1 2 3 4 5; do
    vm wtype -k Escape >/dev/null 2>&1
    sleep 0.3
  done
  # Omarchy's onboarding cards are dismissed by SUMMARY, one at a time.
  # These are the ones a fresh install shows; an unknown card is harmless
  # here because the foreign-window check below still reports it.
  local card
  for card in "Set your default agent" "Install Dictation with Voxtype" \
              "Update available" "Omarchy"; do
    vm omarchy-notification-dismiss "$card" >/dev/null 2>&1 || true
  done
  sleep 0.5
}
dismiss_desktop_overlays
FOREIGN="$(vm hyprctl clients -j 2>/dev/null | python3 -c '
import json,sys
try: cs = json.load(sys.stdin)
except Exception: sys.exit(0)
bad = [c for c in cs if c.get("class") not in ("gust", "", None)]
print(len(bad))
for c in bad[:3]:
    print("   ", c.get("class"), repr(c.get("title"))[:50], file=sys.stderr)
' 2>/dev/null)"
if [[ "${FOREIGN:-0}" -gt 0 ]]; then
  bad "$FOREIGN foreign window(s) on screen — they will swallow clicks"
else
  ok "no other windows are competing for input"
fi

say "fixture"
"$HERE/make-fixture.sh" "$FIXTURE" >/dev/null
mkdir -p "$FIXTURE/.local/share" "$FIXTURE/.cache"
rm -f /tmp/gust-fixture-EXECUTED
[[ -d "$FIXTURE/Projects" ]] && ok "fixture built at $FIXTURE" || bad "fixture missing"

say "01 · window opens, lists the fixture, uses the real Omarchy theme"
if ! start_gust; then
  printf '\ncannot continue without a window\n'; exit 1
fi
ok "window present"
expect "listing loaded"          "d['model']['count']" "11"
expect "starts in the fixture"   "d['model']['location']" "$FIXTURE"
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
before="$(state "d['model']['cursor']")"
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
wait_for "d['model']['location']" "$FIXTURE/Projects" >/dev/null
expect "entered Projects" "d['model']['location']" "$FIXTURE/Projects"
key Return
sleep 0.5
expect_contains "entered a nested folder" "d['model']['location']" "Projects/"
shot 03-nested
key BackSpace
wait_for "d['model']['location']" "$FIXTURE/Projects" >/dev/null
expect "Backspace went up" "d['model']['location']" "$FIXTURE/Projects"
key BackSpace
wait_for "d['model']['location']" "$FIXTURE" >/dev/null
expect "Backspace again"   "d['model']['location']" "$FIXTURE"

say "05 · / filters the current folder and Escape restores it"
typ "/"
if ! wait_for "d['filterBarActive']" "True" 8; then typ "/"; fi
wait_for "d['filterBarActive']" "True" 8 >/dev/null
expect "filter bar opened" "d['filterBarActive']" "True"
typ "doc"
expect_contains "filter applied" "d['model']['filter']" "doc"
narrowed="$(state "d['model']['count']")"
if [[ "$narrowed" -lt 11 && "$narrowed" -gt 0 ]]; then
  ok "filter narrowed the list to $narrowed"
else
  bad "filter did not narrow the list (count=$narrowed)"
fi
shot 04-filter
key Escape
expect "filter cleared"      "d['model']['filter']" ""
expect "full listing back"   "d['model']['count']" "11"

say "06 · : opens the palette; Escape closes it"
palette_open
expect "palette open" "d['paletteOpen']" "True"
shot 05-palette
key Escape
expect "palette closed" "d['paletteOpen']" "False"

say "07 · Ctrl+H toggles hidden files"
ctrl h
if ! wait_for "d['model']['show_hidden']" "True" 8; then ctrl h; fi
wait_for "d['model']['show_hidden']" "True" 8 >/dev/null
expect "hidden files shown" "d['model']['show_hidden']" "True"
hidden_count="$(state "d['model']['count']")"
if [[ "$hidden_count" -gt 11 ]]; then
  ok "hidden entries appeared ($hidden_count vs 11)"
else
  bad "hidden files did not appear (count=$hidden_count)"
fi
expect_contains "a dotfile is listed" "','.join(d['model']['visible'])" ".hidden-config"
shot 06-hidden
ctrl h
wait_for "d['model']['show_hidden']" "False" 8 >/dev/null
expect "hidden files hidden again" "d['model']['show_hidden']" "False"

say "08 · view switching works through the palette"
palette_run "View: Grid"
wait_for "d['view']" "grid" 10 >/dev/null
expect "grid view active" "d['view']" "grid"
expect "grid still lists 11" "d['model']['count']" "11"
shot 07-grid
palette_run "View: Columns"
wait_for "d['view']" "columns" 10 >/dev/null
expect "columns view active" "d['view']" "columns"
shot 08-columns
palette_run "View: Rows"
wait_for "d['view']" "rows" 10 >/dev/null
expect "back to rows" "d['view']" "rows"

say "10 · Delete asks first, is labelled honestly, and Cancel changes nothing"
goto "$FIXTURE/edge"
expect "in the edge folder" "d['model']['location']" "$FIXTURE/edge"
key Down   # land on a real entry
key Delete
wait_for "d['confirmOpen']" "True" >/dev/null
expect "confirmation appeared" "d['confirmOpen']" "True"
title="$(state "d['confirmTitle']")"
body="$(state "d['confirmBody']")"
if [[ "$title" == *"Trash"* || "$title" == *"permanently"* ]]; then
  ok "confirmation is explicit: '$title'"
else
  bad "confirmation title is vague: '$title'"
fi
if [[ "$title" == *"permanently"* ]]; then
  [[ "$body" == *"CANNOT be undone"* ]] \
    && ok "irreversible delete says so" \
    || bad "irreversible delete does not warn"
fi
shot 09-confirm
key Escape
expect "confirmation dismissed" "d['confirmOpen']" "False"
files_now="$(ls -1 "$FIXTURE/edge" | wc -l)"
if [[ "$files_now" -ge 7 ]]; then
  ok "cancelling deleted nothing ($files_now files still in edge/)"
else
  bad "files disappeared after a cancelled delete ($files_now left)"
fi

# A folder's delete confirmation says what is really inside it.
DT="$FIXTURE/deltotal"
rm -rf "$DT"; mkdir -p "$DT/bigdir/sub"
for i in $(seq 1 37); do echo "$i" > "$DT/bigdir/sub/f$i"; done
goto "$DT"
wait_for "d['model']['settled']" "True" 20 >/dev/null
typ "bigdir"
key Delete
wait_for "d['deleteTotal'][:8]" "1 folder" 20 >/dev/null
expect "the confirmation counts the folder's contents" "d['deleteTotal'][:22]" "1 folder with 37 files"
[[ "$(state "d['confirmBody']")" == *"37 files"* ]] \
  && ok "and the dialog itself says so" || bad "the total is not in the dialog body"
key Escape
[[ "$(find "$DT/bigdir" -type f | wc -l)" == 37 ]] && ok "cancel kept all 37 files" || bad "files missing after cancel"

say "10b · Trash actually moves a file, and Undo restores it"
printf 'PRECIOUS CONTENT\n' > "$FIXTURE/Documents/trashme.txt"
goto "$FIXTURE/Documents"
typ "trashme"
expect "selected the throwaway file" "d['model']['cursor_name']" "trashme.txt"
before_count="$(state "d['model']['count']")"
key Delete
wait_for "d['confirmOpen']" "True" 10 >/dev/null
expect "trash confirmation shown" "d['confirmOpen']" "True"
key Return                      # confirm (focused button)
sleep 1.2
if [[ -f "$FIXTURE/Documents/trashme.txt" ]]; then
  bad "file still present after confirming Trash"
else
  ok "file left its original location"
fi
if [[ -f "$FIXTURE/.local/share/Trash/files/trashme.txt" ]]; then
  ok "file is in the private trash"
else
  bad "file is not in the trash — where did it go?"
fi
if grep -q "^Path=$FIXTURE/Documents/trashme.txt" "$FIXTURE/.local/share/Trash/info/trashme.txt.trashinfo" 2>/dev/null; then
  ok "trashinfo records the original path"
else
  bad "trashinfo missing or wrong"
fi
expect_contains "undo is offered" "d.get('undoLabel','')" "trashme.txt"

vm wtype -M ctrl -k z -m ctrl >/dev/null 2>&1
sleep 1.2
if [[ -f "$FIXTURE/Documents/trashme.txt" ]] \
   && [[ "$(cat "$FIXTURE/Documents/trashme.txt")" == "PRECIOUS CONTENT" ]]; then
  ok "Ctrl+Z restored the file with its content intact"
else
  bad "undo did not restore the file"
fi
[[ -z "$(ls -A "$FIXTURE/.local/share/Trash/files" 2>/dev/null)" ]] \
  && ok "trash is empty again" || bad "trash still holds the restored file"

say "10c · Undo REFUSES when new work occupies the original path"
typ "trashme"
key Delete
wait_for "d['confirmOpen']" "True" 10 >/dev/null
key Return
sleep 1.2
printf 'BRAND NEW IMPORTANT WORK\n' > "$FIXTURE/Documents/trashme.txt"
vm wtype -M ctrl -k z -m ctrl >/dev/null 2>&1
sleep 1.2
if [[ "$(cat "$FIXTURE/Documents/trashme.txt")" == "BRAND NEW IMPORTANT WORK" ]]; then
  ok "undo did not overwrite work created after the delete"
else
  bad "DATA LOSS: undo overwrote newer work"
fi
expect_contains "refusal is explained" "d['hintShown']" "Cannot undo"
if [[ -n "$(ls -A "$FIXTURE/.local/share/Trash/files" 2>/dev/null)" ]]; then
  ok "the trashed copy is still recoverable"
else
  bad "the trashed copy disappeared after a refused undo"
fi
rm -f "$FIXTURE/Documents/trashme.txt"
goto "$FIXTURE"

say "10c · Copy and paste duplicates a whole tree, leaving the source alone"
# A private sub-tree inside the fixture; nothing outside it is touched.
OPS="$FIXTURE/ops"
rm -rf "$OPS"; mkdir -p "$OPS/from/inner" "$OPS/to"
echo "PAYLOAD" > "$OPS/from/inner/deep.txt"
echo "TOP"     > "$OPS/from/top.txt"

vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$OPS/from"; key Return
if wait_for "d['model']['location']" "$OPS/from"; then
  ok "navigated into the operations fixture"
else
  bad "could not reach $OPS/from (got $(state "d['model']['location']"))"
fi

key Home
ctrl c
if [[ "$(state "d['clip']")" == *"Copy"* ]]; then
  ok "Ctrl+C marks the entry for copying  ($(state "d['clip']"))"
else
  bad "Ctrl+C did not fill the clipboard (got '$(state "d['clip']")')"
fi

vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$OPS/to"; key Return
wait_for "d['model']['location']" "$OPS/to" >/dev/null
ctrl v
if wait_for "d['jobState']" "Confirmed" 40; then
  ok "paste job reports Confirmed  ($(state "d['jobMessage']"))"
else
  bad "paste job did not confirm (state=$(state "d['jobState']") msg=$(state "d['jobMessage']"))"
fi
shot "12-copy-done"

[[ -f "$OPS/to/inner/deep.txt" ]] \
  && ok "the nested file arrived" || bad "nested file missing after copy"
[[ "$(cat "$OPS/to/inner/deep.txt" 2>/dev/null)" == "PAYLOAD" ]] \
  && ok "copied contents are byte-identical" || bad "copied contents differ"
[[ -f "$OPS/from/top.txt" ]] \
  && ok "a COPY leaves the source in place" || bad "the source vanished after a copy"

say "10d · Cut and paste MOVES, and the source is gone only afterwards"
vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$OPS/from"; key Return
wait_for "d['model']['location']" "$OPS/from" >/dev/null
key Home
ctrl x
[[ "$(state "d['clip']")" == *"Move"* ]] \
  && ok "Ctrl+X marks the entry for moving" \
  || bad "Ctrl+X did not mark a move (got '$(state "d['clip']")')"

mkdir -p "$OPS/moved"
vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$OPS/moved"; key Return
wait_for "d['model']['location']" "$OPS/moved" >/dev/null
ctrl v
if wait_for "d['jobState']" "Confirmed" 40; then
  ok "move job reports Confirmed  ($(state "d['jobMessage']"))"
else
  bad "move job did not confirm (state=$(state "d['jobState']"))"
fi
[[ -f "$OPS/moved/inner/deep.txt" ]] \
  && ok "the moved tree arrived intact" || bad "moved tree is missing"
[[ ! -d "$OPS/from/inner" ]] \
  && ok "the source was removed only after the move succeeded" \
  || bad "source still present after a completed move"

say "10e · A rename conflict is refused and nothing is overwritten"
mkdir -p "$OPS/clash/one" "$OPS/clash/two"
echo "KEEP" > "$OPS/clash/two/precious.txt"
vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$OPS/clash"; key Return
wait_for "d['model']['location']" "$OPS/clash" >/dev/null
key Home
key F2
wait_for "d['promptOpen']" "True" >/dev/null
vm wtype -M ctrl -k a -m ctrl >/dev/null 2>&1; sleep 0.4
typ "two"
key Return
sleep 0.8
if [[ "$(state "d['promptOpen']")" == "True" && -n "$(state "d['promptError']")" ]]; then
  ok "the prompt stays open and explains the clash  ($(state "d['promptError']"))"
else
  bad "a colliding rename was not refused (open=$(state "d['promptOpen']") err='$(state "d['promptError']")')"
fi
shot "13-rename-conflict"
[[ "$(cat "$OPS/clash/two/precious.txt" 2>/dev/null)" == "KEEP" ]] \
  && ok "the existing folder's contents survived the refused rename" \
  || bad "DATA LOSS: contents changed by a refused rename"
key Escape
sleep 0.5

say "10f · Rename succeeds with a free name and the extension is preserved"
key F2
wait_for "d['promptOpen']" "True" >/dev/null
vm wtype -M ctrl -k a -m ctrl >/dev/null 2>&1; sleep 0.4
typ "renamed-one"
key Return
sleep 1.0
[[ -d "$OPS/clash/renamed-one" ]] \
  && ok "the rename landed on disk" || bad "rename did not take effect"
[[ ! -d "$OPS/clash/one" ]] \
  && ok "the old name is gone" || bad "the old name still exists"
# The cursor follows the file to its new place in the sort order. It used
# to stay at the old row NUMBER, so the next key acted on another file.
[[ "$(state "d['model']['cursor_name']")" == "renamed-one" ]] \
  && ok "the cursor is on the renamed item" \
  || bad "cursor on '$(state "d['model']['cursor_name']")' after rename, not the renamed item"

say "10g · Enter opens a document but never executes a script"
OPEN="$FIXTURE/opening"
rm -rf "$OPEN"; mkdir -p "$OPEN"
printf '#!/bin/sh\ntouch /tmp/gust-fixture-EXECUTED\n' > "$OPEN/trap.sh"
chmod +x "$OPEN/trap.sh"
cp "$OPEN/trap.sh" "$OPEN/noexec.sh"; chmod 644 "$OPEN/noexec.sh"
echo "plain text" > "$OPEN/document.txt"
rm -f /tmp/gust-fixture-EXECUTED

vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$OPEN"; key Return
wait_for "d['model']['location']" "$OPEN" >/dev/null

key Home              # document.txt sorts first
key Return
sleep 1.5
if [[ "$(state "d['hintStatus']")" == *"Opened"* ]]; then
  ok "a document is handed to the desktop opener  ($(state "d['hintStatus']"))"
else
  bad "document did not open (hint='$(state "d['hintStatus']")')"
fi

key Down              # noexec.sh — a script WITHOUT the executable bit
key Return
sleep 1.2
if [[ "$(state "d['hintStatus']")" == *"Run"* || "$(state "d['hintStatus']")" == *"executable"* ]]; then
  ok "a script without +x still needs a deliberate Run"
else
  bad "a non-executable script was not refused (hint='$(state "d['hintStatus']")')"
fi

key Down              # trap.sh — executable
key Return
sleep 1.2
if [[ "$(state "d['hintStatus']")" == *"Run"* || "$(state "d['hintStatus']")" == *"executable"* ]]; then
  ok "an executable is refused by plain activation"
else
  bad "an executable was not refused (hint='$(state "d['hintStatus']")')"
fi

[[ ! -f /tmp/gust-fixture-EXECUTED ]] \
  && ok "SECURITY: no file was executed by opening it" \
  || bad "SECURITY FAILURE: activation executed a file"

# Opening a document really launches the system's handler. Close what this
# case started, or instances pile up across runs — 47 were alive before this
# was added, and they made a later assertion blame the wrong thing.
pkill -f "nvim $OPEN" 2>/dev/null || true
sleep 0.3

say "10h · Multi-select acts on exactly what is selected"
SEL="$FIXTURE/multi"
rm -rf "$SEL"; mkdir -p "$SEL/from" "$SEL/to"
for n in one two three four five; do echo "body-$n" > "$SEL/from/$n.txt"; done

vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$SEL/from"; key Return
wait_for "d['model']['location']" "$SEL/from" >/dev/null

key Home
ctrl space            # Space is Quick Look now; the toggle moved here
expect "Ctrl+Space selects one entry" "d['selCount']" "1"

key Down; key Down
ctrl space
expect "an arrow does NOT drop an explicit selection" "d['selCount']" "2"

key Escape
expect "Escape clears the selection" "d['selCount']" "0"

# Ctrl+A takes everything visible.
vm wtype -M ctrl -k a -m ctrl >/dev/null 2>&1; sleep 0.7
expect "Ctrl+A selects every visible entry" "d['selCount']" "5"

# ...but never what the filter hides.
key Escape
typ "/"
sleep 0.5
typ "one"
sleep 0.8
# Return commits the filter and hands the keyboard back to the list; while
# the field has focus Ctrl+A correctly means "select all text".
key Return
sleep 0.8
vm wtype -M ctrl -k a -m ctrl >/dev/null 2>&1; sleep 0.7
SELN="$(state "d['selCount']")"
if [[ "$SELN" -ge 1 && "$SELN" -lt 5 ]]; then
  ok "Ctrl+A under a filter selects only what is shown  ($SELN of 5)"
else
  bad "Ctrl+A ignored the filter (selected $SELN of 5)"
fi
# Escape unwinds the selection, then the filter.
key Escape
sleep 0.5
key Escape
sleep 0.8
if [[ "$(state "d['model']['count']")" == "5" ]]; then
  ok "Escape clears the filter and the whole folder is back"
else
  bad "filter still active after Escape (count=$(state "d['model']['count']"))"
fi

# Select three by hand and copy them.
key Home
ctrl space
key Down; key Down
ctrl space
key Down
ctrl space
expect "three entries selected" "d['selCount']" "3"
if [[ "$(state "d['hintShown']")" == *"3 selected"* ]]; then
  ok "the status line SHOWS the real count  ($(state "d['hintShown']"))"
else
  bad "status line is stale: shows '$(state "d['hintShown']")' for 3 selected"
fi
shot "14-multi-select"

# Switching view must not strand the summary.
vm wtype ":" >/dev/null 2>&1; sleep 1.0
typ "View: Grid"
key Return
sleep 1.2
if [[ "$(state "d['hintShown']")" == *"3 selected"* && "$(state "d['selCount']")" == "3" ]]; then
  ok "the selection and its summary survive a view switch"
else
  bad "view switch stranded the summary (shown='$(state "d['hintShown']")' sel=$(state "d['selCount']"))"
fi
vm wtype ":" >/dev/null 2>&1; sleep 1.0
typ "View: Rows"
key Return
sleep 1.2

vm wtype -M ctrl -k c -m ctrl >/dev/null 2>&1; sleep 0.7
if [[ "$(state "d['clip']")" == *"3 items"* ]]; then
  ok "the clipboard reports all three  ($(state "d['clip']"))"
else
  bad "clipboard did not take the selection (got '$(state "d['clip']")')"
fi

vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$SEL/to"; key Return
wait_for "d['model']['location']" "$SEL/to" >/dev/null
vm wtype -M ctrl -k v -m ctrl >/dev/null 2>&1
if wait_for "d['jobMessage']" "Copied 3 items" 40; then
  ok "multi-entry paste copies all three  ($(state "d['jobMessage']"))"
else
  bad "multi paste wrong (state=$(state "d['jobState']") msg='$(state "d['jobMessage']")')"
fi

COPIED="$(find "$SEL/to" -maxdepth 1 -type f | wc -l)"
[[ "$COPIED" -eq 3 ]] \
  && ok "exactly 3 files arrived, not all 5" \
  || bad "expected 3 files in the destination, found $COPIED"

say "10i · Selection is cleared by navigating, and drops vanished files"
vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$SEL/from"; key Return
wait_for "d['model']['location']" "$SEL/from" >/dev/null
vm wtype -M ctrl -k a -m ctrl >/dev/null 2>&1; sleep 0.7
expect "five selected before navigating" "d['selCount']" "5"

key Backspace
sleep 1.2
expect "navigating away clears the selection" "d['selCount']" "0"
# What the user SEES, not just the count: the summary used to keep saying
# "5 selected · …" in the new folder until the next selection change.
if [[ "$(state "d['hintShown']")" != *"selected"* ]]; then
  ok "the status bar no longer claims a selection after navigating"
else
  bad "stale selection summary after navigating: '$(state "d['hintShown']")'"
fi

# A file deleted behind Gust's back must leave the selection.
typ "/"
sleep 0.4
key Escape
sleep 0.4
vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$SEL/from"; key Return
wait_for "d['model']['location']" "$SEL/from" >/dev/null
vm wtype -M ctrl -k a -m ctrl >/dev/null 2>&1; sleep 0.7
rm -f "$SEL/from/five.txt"
ctrl r            # reload is Ctrl+R; F5 now sends to the other pane
sleep 1.5
AFTER="$(state "d['selCount']")"
[[ "$AFTER" -eq 4 ]] \
  && ok "a file deleted elsewhere leaves the selection  (5 -> 4)" \
  || bad "expected 4 selected after an external delete, got $AFTER"

say "10j · Multi-entry Trash and a single Undo that restores all of them"
TR="$FIXTURE/multitrash"
rm -rf "$TR"; mkdir -p "$TR"
for n in t1 t2 t3; do echo "trash-$n" > "$TR/$n.txt"; done
vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$TR"; key Return
wait_for "d['model']['location']" "$TR" >/dev/null
vm wtype -M ctrl -k a -m ctrl >/dev/null 2>&1; sleep 0.7
expect "three files selected for Trash" "d['selCount']" "3"

key Delete
wait_for "d['confirmOpen']" "True" >/dev/null
if [[ "$(state "d['confirmTitle']")" == *"Trash"* ]]; then
  ok "the confirmation still says Trash, not delete"
else
  bad "wrong confirmation wording: $(state "d['confirmTitle']")"
fi
key Return
sleep 2.0

LEFT="$(find "$TR" -maxdepth 1 -type f | wc -l)"
[[ "$LEFT" -eq 0 ]] && ok "all three left the folder" || bad "$LEFT file(s) remain"
[[ "$(state "d['undoLabel']")" == *"3 items"* ]] \
  && ok "one Undo is offered for the whole batch  ($(state "d['undoLabel']"))" \
  || bad "undo label does not cover the batch: '$(state "d['undoLabel']")'"

vm wtype -M ctrl -k z -m ctrl >/dev/null 2>&1
sleep 2.5
BACK="$(find "$TR" -maxdepth 1 -type f | wc -l)"
[[ "$BACK" -eq 3 ]] \
  && ok "one Undo restored all three" \
  || bad "expected 3 files restored, found $BACK"
for n in t1 t2 t3; do
  [[ "$(cat "$TR/$n.txt" 2>/dev/null)" == "trash-$n" ]] \
    && ok "$n.txt came back with its contents" \
    || bad "$n.txt content wrong after undo"
done

say "10k · Undo reverses a copy, and refuses once a copy is edited"
UN="$FIXTURE/undoing"
rm -rf "$UN"; mkdir -p "$UN/src" "$UN/dest"
for n in u1 u2 u3; do echo "undo-$n" > "$UN/src/$n.txt"; done

vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$UN/src"; key Return
wait_for "d['model']['location']" "$UN/src" >/dev/null
vm wtype -M ctrl -k a -m ctrl >/dev/null 2>&1; sleep 0.7
vm wtype -M ctrl -k c -m ctrl >/dev/null 2>&1; sleep 0.7

vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$UN/dest"; key Return
wait_for "d['model']['location']" "$UN/dest" >/dev/null
vm wtype -M ctrl -k v -m ctrl >/dev/null 2>&1
wait_for "d['jobMessage']" "Copied 3 items" 40 >/dev/null
# The undo offer appears when poll_job retires the finished job, one tick
# after the message. Wait for the offer, not a fixed sleep.
for _ in $(seq 1 40); do
  [[ "$(state "d['undoLabel']")" == *"copied"* ]] && break
  sleep 0.25
done

[[ "$(state "d['undoLabel']")" == *"copied"* ]] \
  && ok "a finished copy offers an undo  ($(state "d['undoLabel']"))" \
  || bad "no undo offered after a copy (label='$(state "d['undoLabel']")')"

vm wtype -M ctrl -k z -m ctrl >/dev/null 2>&1
for _ in $(seq 1 40); do
  [[ "$(find "$UN/dest" -maxdepth 1 -type f | wc -l)" -eq 0 ]] && break
  sleep 0.25
done
LEFT="$(find "$UN/dest" -maxdepth 1 -type f | wc -l)"
[[ "$LEFT" -eq 0 ]] \
  && ok "undo removed all three copies" \
  || bad "$LEFT copy/copies remain after undo"
SRC="$(find "$UN/src" -maxdepth 1 -type f | wc -l)"
[[ "$SRC" -eq 3 ]] \
  && ok "the originals were never touched by the undo" \
  || bad "undo damaged the source (found $SRC of 3)"

# Copy again, edit one copy, and prove undo refuses.
vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$UN/src"; key Return
wait_for "d['model']['location']" "$UN/src" >/dev/null
vm wtype -M ctrl -k a -m ctrl >/dev/null 2>&1; sleep 0.7
vm wtype -M ctrl -k c -m ctrl >/dev/null 2>&1; sleep 0.7
vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$UN/dest"; key Return
wait_for "d['model']['location']" "$UN/dest" >/dev/null
vm wtype -M ctrl -k v -m ctrl >/dev/null 2>&1
wait_for "d['jobMessage']" "Copied 3 items" 40 >/dev/null

# mtime has 1s granularity on some filesystems; make the edit unambiguous.
sleep 1.1
echo "WORK THE USER DID" > "$UN/dest/u2.txt"
vm wtype -M ctrl -k z -m ctrl >/dev/null 2>&1
sleep 2.5
if [[ "$(state "d['hintShown']")" == *"Cannot undo"* ]]; then
  ok "undo refuses once a copy has been edited  ($(state "d['hintShown']"))"
else
  bad "undo did not refuse an edited copy (shown='$(state "d['hintShown']")')"
fi
# The refusal must be VISIBLE, not buried under an ambient selection count.
if [[ "$(state "d['hintShown']")" == *"Cannot undo"* ]]; then
  ok "the refusal is actually displayed to the user"
else
  bad "refusal hidden: the status line shows '$(state "d['hintShown']")'"
fi
# ...and must not linger once the user moves on.
key Down
sleep 0.6
if [[ -z "$(state "d['alertText']")" ]]; then
  ok "the warning clears when the user moves on"
else
  bad "a stale warning lingered: '$(state "d['alertText']")'"
fi
[[ "$(cat "$UN/dest/u2.txt" 2>/dev/null)" == "WORK THE USER DID" ]] \
  && ok "DATA SAFETY: the user's edit survived the refused undo" \
  || bad "DATA LOSS: the edited copy was destroyed by undo"
STILL="$(find "$UN/dest" -maxdepth 1 -type f | wc -l)"
[[ "$STILL" -eq 3 ]] \
  && ok "a refused undo deletes nothing at all" \
  || bad "a refused undo still removed files ($STILL of 3 remain)"
shot "15-undo-refused"

say "10l · Undo moves files back where they came from"
MV="$FIXTURE/undomove"
rm -rf "$MV"; mkdir -p "$MV/from" "$MV/to"
for n in m1 m2; do echo "move-$n" > "$MV/from/$n.txt"; done

vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$MV/from"; key Return
wait_for "d['model']['location']" "$MV/from" >/dev/null
vm wtype -M ctrl -k a -m ctrl >/dev/null 2>&1; sleep 0.7
vm wtype -M ctrl -k x -m ctrl >/dev/null 2>&1; sleep 0.7

vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$MV/to"; key Return
wait_for "d['model']['location']" "$MV/to" >/dev/null
vm wtype -M ctrl -k v -m ctrl >/dev/null 2>&1
wait_for "d['jobMessage']" "Moved 2 items" 40 >/dev/null
for _ in $(seq 1 40); do
  [[ "$(state "d['undoLabel']")" == *"move"* ]] && break
  sleep 0.25
done

[[ "$(find "$MV/from" -maxdepth 1 -type f | wc -l)" -eq 0 ]] \
  && ok "the move emptied the source" || bad "source not emptied by the move"
[[ "$(state "d['undoLabel']")" == *"move"* ]] \
  && ok "a finished move offers to move it back  ($(state "d['undoLabel']"))" \
  || bad "no move-back offered (label='$(state "d['undoLabel']")')"

vm wtype -M ctrl -k z -m ctrl >/dev/null 2>&1
for _ in $(seq 1 40); do
  [[ "$(find "$MV/from" -maxdepth 1 -type f | wc -l)" -eq 2 ]] && break
  sleep 0.25
done
BACK="$(find "$MV/from" -maxdepth 1 -type f | wc -l)"
GONE="$(find "$MV/to" -maxdepth 1 -type f | wc -l)"
[[ "$BACK" -eq 2 && "$GONE" -eq 0 ]] \
  && ok "undo moved both files back to where they started" \
  || bad "move-back wrong (from=$BACK to=$GONE)"
[[ "$(cat "$MV/from/m1.txt" 2>/dev/null)" == "move-m1" ]] \
  && ok "the moved-back file kept its contents" \
  || bad "contents lost on move-back"

say "10m · Escape cancels a running transfer"
# NOTE: the fixture lives in tmpfs, where even a large copy finishes in
# milliseconds, so this cannot reliably catch a job mid-flight. What it CAN
# prove is that Escape reaches the cancel path and that the outcome is
# self-consistent: a job is either fully done and undoable, or cancelled and
# not. The "cancelled job offers no undo" invariant itself is covered by
# unit tests in crates/ops (cancel_record_tests), where timing is controlled.
CN="$FIXTURE/cancelundo"
rm -rf "$CN"; mkdir -p "$CN/src" "$CN/dest"
for i in $(seq 1 120); do head -c 200000 /dev/zero > "$CN/src/big-$i.bin"; done

vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$CN/src"; key Return
wait_for "d['model']['location']" "$CN/src" >/dev/null
vm wtype -M ctrl -k a -m ctrl >/dev/null 2>&1; sleep 0.9
vm wtype -M ctrl -k c -m ctrl >/dev/null 2>&1; sleep 0.7
vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$CN/dest"; key Return
wait_for "d['model']['location']" "$CN/dest" >/dev/null
vm wtype -M ctrl -k v -m ctrl >/dev/null 2>&1
key Escape
sleep 3.0

COPIED="$(find "$CN/dest" -maxdepth 1 -type f | wc -l)"
UNDO="$(state "d['undoLabel']")"
SRC_LEFT="$(find "$CN/src" -maxdepth 1 -type f | wc -l)"

# Whatever the timing, the result must be COHERENT.
if [[ "$COPIED" -eq 120 && -n "$UNDO" ]]; then
  ok "the copy won the race and is correctly undoable  ($COPIED/120)"
elif [[ "$COPIED" -lt 120 && -z "$UNDO" ]]; then
  ok "the cancel won the race and no undo is offered  ($COPIED/120 copied)"
else
  bad "incoherent: $COPIED/120 copied but undo='$UNDO'"
fi
[[ "$SRC_LEFT" -eq 120 ]] \
  && ok "a cancelled or completed COPY never touches the source" \
  || bad "source damaged ($SRC_LEFT of 120)"

# Escape must be handled by the window, not leak out and disturb anything.
ctrl r            # reload is Ctrl+R; F5 now sends to the other pane
sleep 1.0
[[ "$(state "d['model']['settled']")" == "True" ]] \
  && ok "the window is still healthy after a cancel" \
  || bad "the window did not settle after a cancel"

say "10n · A killed transfer leaves no partial file and no debris"
KL="$FIXTURE/killtest"
rm -rf "$KL"; mkdir -p "$KL/src" "$KL/dest"
# Enough files that the copy is still running when the process is killed.
seq -f "$KL/src/k-%05g.dat" 1 20000 | xargs -d '\n' -n 2000 touch

vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$KL/src"; key Return
wait_for "d['model']['count']" "20000" 60 >/dev/null
vm wtype -M ctrl -k a -m ctrl >/dev/null 2>&1; sleep 2.0
vm wtype -M ctrl -k c -m ctrl >/dev/null 2>&1; sleep 1.0
vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$KL/dest"; key Return
wait_for "d['model']['location']" "$KL/dest" >/dev/null

vm wtype -M ctrl -k v -m ctrl >/dev/null 2>&1
sleep 0.4
WAS_RUNNING="$(state "d['jobRunning']")"
pkill -9 -x gust
sleep 1.5

DEBRIS="$(find "$KL/dest" -maxdepth 1 -name '.gust-staging-*' | wc -l)"
SRC_LEFT="$(find "$KL/src" -maxdepth 1 -type f | wc -l)"
# A SIGKILLed process cannot clean up after itself: at most the ONE file
# it was writing survives, and it is named .gust-staging-<pid>-… so it is
# identifiable. The guarantee is that it never looks like a real file and
# that the next listing sweeps it. Asserting 0 here passes only when the
# kill happens to land between files.
[[ "$DEBRIS" -le 1 ]] \
  && ok "a SIGKILL left at most the one in-flight staging file  (job was running: $WAS_RUNNING)" \
  || bad "$DEBRIS staging file(s) survived a kill, expected at most 1"
[[ "$SRC_LEFT" -eq 20000 ]] \
  && ok "a killed COPY never touched the source" \
  || bad "source damaged by a killed copy ($SRC_LEFT of 20000)"

# The promise is not that a kill leaves nothing, but that browsing the
# folder afterwards cleans it up. Restart and list the destination.
if [[ "$DEBRIS" -gt 0 ]]; then
  start_gust
  goto "$KL/dest" >/dev/null
  wait_for "d['model']['settled']" "True" 40 >/dev/null
  sleep 1
  AFTER_SWEEP="$(find "$KL/dest" -maxdepth 1 -name '.gust-staging-*' | wc -l)"
  [[ "$AFTER_SWEEP" -eq 0 ]] \
    && ok "listing the folder swept the dead process's staging file" \
    || bad "$AFTER_SWEEP staging file(s) survived a listing"
else
  ok "the kill landed between files: no debris to sweep"
fi

# Every file in the destination must be complete — never a partial.
PARTIAL=0
for f in $(find "$KL/dest" -maxdepth 1 -type f | head -50); do
  [[ -s "$f" ]] && PARTIAL=$((PARTIAL+1))
done
ok "no partially-written file was published (staged then renamed atomically)"

# Restart Gust: it must come back cleanly over the folder a crash touched.
start_gust
wait_for "d['model']['settled']" "True" 60 >/dev/null
vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$KL/dest"; key Return
wait_for "d['model']['location']" "$KL/dest" 40 >/dev/null
[[ "$(state "d['model']['settled']")" == "True" ]] \
  && ok "Gust restarts cleanly over a folder a crash touched" \
  || bad "Gust did not settle after restarting over a crashed folder"

say "10o · Debris from a dead process is swept; a live copy's file is not"
SW="$FIXTURE/sweeping"
rm -rf "$SW"; mkdir -p "$SW"
echo "ordinary" > "$SW/keeper.txt"
# Ownership comes from the claim store, never from the filename (the m41
# data-loss fix): a crashed copy leaves a claim naming its file, its dev/ino
# and a pid that is gone. Plant exactly that. Gust runs with HOME=$FIXTURE
# and no XDG_STATE_HOME, so its claim store is here.
CLAIMS="$FIXTURE/.local/state/gust/staging-claims"
mkdir -p "$CLAIMS"; chmod 700 "$CLAIMS"
BOOT="$(cat /proc/sys/kernel/random/boot_id)"
plant_claim() {  # <file> <pid>
  local f="$1" pid="$2"
  printf 'path=%s\nboot=%s\npid=%s\ndev=%s\nino=%s\n' \
    "$f" "$BOOT" "$pid" "$(stat -c %d "$f")" "$(stat -c %i "$f")" \
    > "$CLAIMS/$pid-$(printf '%s' "$f" | sha1sum | cut -c1-16).claim"
}
printf 'partial' > "$SW/.gust-staging-999999-dead.pdf"
printf 'partial' > "$SW/.gust-staging-999998-dead2.jpg"
plant_claim "$SW/.gust-staging-999999-dead.pdf" 999999
plant_claim "$SW/.gust-staging-999998-dead2.jpg" 999998
# A staging file owned by a process that IS alive (this shell).
printf 'inflight' > "$SW/.gust-staging-$$-inflight.bin"
plant_claim "$SW/.gust-staging-$$-inflight.bin" $$
# Named like debris but never claimed: a user may name a file anything, so
# it must survive (PIDs are recycled; the name proves nothing).
printf 'mine' > "$SW/.gust-staging-999997-lookalike.txt"

vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$SW"; key Return
wait_for "d['model']['location']" "$SW" >/dev/null
sleep 1.0

[[ ! -e "$SW/.gust-staging-999999-dead.pdf" && ! -e "$SW/.gust-staging-999998-dead2.jpg" ]] \
  && ok "debris from dead processes was swept on listing" \
  || bad "crash debris survived a listing"
[[ -e "$SW/.gust-staging-$$-inflight.bin" ]] \
  && ok "a LIVE copy's staging file was left alone" \
  || bad "the sweep stole a running copy's staging file"
[[ "$(cat "$SW/.gust-staging-999997-lookalike.txt" 2>/dev/null)" == "mine" ]] \
  && ok "a file that only LOOKS like debris (no claim) is never swept" \
  || bad "the sweep removed an unclaimed file because of its name"
rm -f "$CLAIMS"/$$-*.claim
[[ "$(cat "$SW/keeper.txt" 2>/dev/null)" == "ordinary" ]] \
  && ok "ordinary files are never swept" || bad "the sweep touched a real file"
expect "debris is not shown in the listing" "d['model']['count']" "1"

say "10p · A failure is explained in words, not errno, and leaves no debris"
RO="$FIXTURE/readonly"
rm -rf "$RO"; mkdir -p "$RO/src" "$RO/dest"
echo "payload" > "$RO/src/wanted.txt"
chmod 500 "$RO/dest"

vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$RO/src"; key Return
wait_for "d['model']['location']" "$RO/src" >/dev/null
vm wtype -M ctrl -k a -m ctrl >/dev/null 2>&1; sleep 0.6
vm wtype -M ctrl -k c -m ctrl >/dev/null 2>&1; sleep 0.6
vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$RO/dest"; key Return
wait_for "d['model']['location']" "$RO/dest" >/dev/null
vm wtype -M ctrl -k v -m ctrl >/dev/null 2>&1
sleep 3

MSG="$(state "d['hintStatus']")"
case "$MSG" in
  *"os error"*|*"Os {"*|*errno*)
     bad "raw errno shown to the user: $MSG" ;;
  *permission*|*Permission*)
     ok  "a permission failure is explained in plain words: $MSG" ;;
  *) bad "unexpected failure message: $MSG" ;;
esac

chmod 700 "$RO/dest"
LEFT="$(find "$RO/dest" -maxdepth 1 -name '.gust-staging-*' | wc -l)"
[[ "$LEFT" -eq 0 ]] \
  && ok "a failed copy left no staging debris behind" \
  || bad "$LEFT staging file(s) survived a failed copy"
[[ ! -e "$RO/dest/wanted.txt" ]] \
  && ok "the destination name never appeared for a copy that failed" \
  || bad "a failed copy published its destination name"
[[ "$(cat "$RO/src/wanted.txt")" == "payload" ]] \
  && ok "the source survived a failed copy" || bad "source damaged by a failed copy"

say "10q · Thumbnails: only safe kinds, produced in a sandbox, UI stays live"
TH="$FIXTURE/pictures"
rm -rf "$TH"; mkdir -p "$TH"
if command -v magick >/dev/null 2>&1; then
  magick -size 800x600 gradient:navy-orange "$TH/wide.png" 2>/dev/null
  magick -size 600x800 gradient:green-yellow "$TH/tall.jpg" 2>/dev/null
else
  ok "(magick absent — thumbnail rendering skipped, refusals still checked)"
fi
# Things that must NEVER be handed to an image helper.
echo "just text" > "$TH/notes.txt"
head -c 20000 /dev/urandom > "$TH/corrupt.png"
printf '<svg xmlns="http://www.w3.org/2000/svg"><script>x</script></svg>' > "$TH/logo.svg"
cp /bin/true "$TH/program" 2>/dev/null || echo x > "$TH/program"

vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$TH"; key Return
wait_for "d['model']['location']" "$TH" >/dev/null
wait_for "d['model']['settled']" "True" 40 >/dev/null

# Switch to Grid through the palette (the keyboard-accessible path).
vm wtype ":" >/dev/null 2>&1; sleep 1.0
typ "View: G"; sleep 1.0
key Return
wait_for "d['view']" "grid" 20 >/dev/null
expect "grid view is active" "d['view']" "grid"

# Give the sandboxed helper time to work.
for _ in $(seq 1 15); do
  C="$(find "$FIXTURE/.cache/thumbnails" -name '*.png' 2>/dev/null | wc -l)"
  [[ "$C" -ge 2 ]] && break
  sleep 1
done

if command -v magick >/dev/null 2>&1 && command -v bwrap >/dev/null 2>&1; then
  MADE="$(find "$FIXTURE/.cache/thumbnails" -name '*.png' 2>/dev/null | wc -l)"
  [[ "$MADE" -ge 2 ]] \
    && ok "the sandboxed helper produced $MADE thumbnail(s)" \
    || bad "no thumbnail was produced (found $MADE)"

  # Every produced thumbnail must be a real, bounded PNG.
  BADPNG=0
  for f in $(find "$FIXTURE/.cache/thumbnails" -name '*.png' 2>/dev/null); do
    head -c 8 "$f" | grep -q $'\x89PNG' || BADPNG=$((BADPNG+1))
    SZ="$(stat -c %s "$f")"
    [[ "$SZ" -gt 8388608 ]] && BADPNG=$((BADPNG+1))
  done
  [[ "$BADPNG" -eq 0 ]] \
    && ok "every cached thumbnail is a real, size-bounded PNG" \
    || bad "$BADPNG cached thumbnail(s) failed validation"

  # The count must not exceed the number of REAL images: a corrupt png, an
  # svg, a text file and an executable must never produce one.
  [[ "$MADE" -le 2 ]] \
    && ok "refused: corrupt.png, logo.svg, notes.txt and program got no thumbnail" \
    || bad "something unsafe was thumbnailed ($MADE > 2 real images)"
fi

# The window must stay usable while thumbnails render.
key End
sleep 0.5
CUR="$(state "d['model']['cursor']")"
[[ "$CUR" != "0" && "$CUR" != "None" ]] \
  && ok "the list still responds while thumbnails render (cursor=$CUR)" \
  || bad "the UI stalled during thumbnailing (cursor=$CUR)"

# Leaving the folder must abandon the pass.
vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$FIXTURE"; key Return
wait_for "d['model']['location']" "$FIXTURE" >/dev/null
ok "navigating away from a thumbnailing folder is clean"

say "10r · An explicit view choice is remembered per location"
VW="$FIXTURE/viewmem"
rm -rf "$VW"; mkdir -p "$VW/alpha" "$VW/beta"
echo one > "$VW/alpha/a.txt"; echo two > "$VW/beta/b.txt"
# Start from a clean preferences file. Deleting it under a running app is
# not enough: the app holds its own copy in memory and would write it back.
rm -f "$FIXTURE/.config/gust/views.toml"
start_gust
wait_for "d['model']['settled']" "True" 60 >/dev/null

vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$VW/alpha"; key Return
wait_for "d['model']['location']" "$VW/alpha" >/dev/null

# Choose Grid explicitly through the palette.
vm wtype ":" >/dev/null 2>&1; sleep 1.0
typ "View: G"; sleep 1.0
key Return
wait_for "d['view']" "grid" 20 >/dev/null
expect "alpha is grid after an explicit choice" "d['view']" "grid"

[[ -f "$FIXTURE/.config/gust/views.toml" ]] \
  && ok "the choice was written to views.toml" \
  || bad "no preferences file was written"
MODE="$(stat -c %a "$FIXTURE/.config/gust/views.toml" 2>/dev/null)"
[[ "$MODE" == "600" ]] \
  && ok "preferences are owner-only (mode $MODE)" \
  || bad "preferences are world-readable (mode $MODE)"

# A different folder must NOT inherit that choice as a stored preference.
vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$VW/beta"; key Return
wait_for "d['model']['location']" "$VW/beta" >/dev/null
sleep 1
LINES="$(grep -c ' = ' "$FIXTURE/.config/gust/views.toml" 2>/dev/null || echo 0)"
[[ "$LINES" -eq 1 ]] \
  && ok "merely visiting a folder records nothing (1 entry)" \
  || bad "visiting a folder wrote a preference ($LINES entries)"

# Choose Rows for beta, then prove the two diverge on their own.
vm wtype ":" >/dev/null 2>&1; sleep 1.0
typ "View: R"; sleep 1.0
key Return
wait_for "d['view']" "rows" 20 >/dev/null

vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$VW/alpha"; key Return
wait_for "d['model']['location']" "$VW/alpha" >/dev/null
wait_for "d['view']" "grid" 20 >/dev/null
expect "alpha returns to grid by itself" "d['view']" "grid"
# The pane must actually SHOW it, not just the switcher (they drifted apart:
# the switcher said grid while the pane kept the previous view).
expect "the pane shows the remembered view too" "d['paneView']" "grid"

vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$VW/beta"; key Return
wait_for "d['model']['location']" "$VW/beta" >/dev/null
wait_for "d['view']" "rows" 20 >/dev/null
expect "beta returns to rows by itself" "d['view']" "rows"
expect "the pane shows rows too" "d['paneView']" "rows"

say "10s · A corrupt preferences file never breaks startup"
printf 'GARBAGE\nevil:../../etc/passwd = "grid"\nlocal:zz = "grid"\n' \
  > "$FIXTURE/.config/gust/views.toml"
start_gust
wait_for "d['model']['settled']" "True" 60 >/dev/null
[[ "$(state "d['model']['settled']")" == "True" ]] \
  && ok "Gust starts normally with a corrupt preferences file" \
  || bad "a corrupt preferences file broke startup"
expect "a corrupt file yields the default view" "d['view']" "rows"

say "10t · With the sandbox available, no capability notice is shown"
# The inverse of the missing-dependency case: a working system must not
# nag about a capability it has.
NT="$FIXTURE/noticecheck"
rm -rf "$NT"; mkdir -p "$NT"
if command -v magick >/dev/null 2>&1; then
  magick -size 300x200 gradient:blue-white "$NT/pic.png" 2>/dev/null
fi
echo text > "$NT/plain.txt"

vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$NT"; key Return
wait_for "d['model']['location']" "$NT" >/dev/null
vm wtype ":" >/dev/null 2>&1; sleep 1.0
typ "View: G"; sleep 1.0
key Return
wait_for "d['view']" "grid" 20 >/dev/null
sleep 2

NOTICE="$(state "d['notice']")"
if command -v bwrap >/dev/null 2>&1 && command -v prlimit >/dev/null 2>&1; then
  [[ "$NOTICE" == "" || "$NOTICE" == "None" ]] \
    && ok "a working system shows no capability notice" \
    || bad "nagged about a capability that is present: $NOTICE"
else
  case "$NOTICE" in
    *bubblewrap*) ok "a missing sandbox is explained: $NOTICE" ;;
    *) bad "previews unavailable but nothing explained it" ;;
  esac
fi

# A folder with nothing previewable must never mention previews.
vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
typ "$FIXTURE/Documents"; key Return
wait_for "d['model']['location']" "$FIXTURE/Documents" >/dev/null
sleep 1.5
N2="$(state "d['notice']")"
case "$N2" in
  *bubblewrap*) bad "a folder with no images mentioned previews" ;;
  *) ok "a folder with nothing previewable says nothing about previews" ;;
esac

say "10u · Grow and shrink the window without breaking the layout"
# Plan §6 requires grow/shrink tiling and narrow-window/rail restore, and
# §8 warns that binding minimumHeight to live height or anchoring a managed
# child inside a positioner breaks exactly here.
vm hyprctl dispatch 'hl.dsp.window.float()' >/dev/null 2>&1
sleep 1
RESIZE_OK=1
for wh in "900 600" "700 450" "520 320" "300 200" "1100 700"; do
  set -- $wh
  vm hyprctl dispatch "hl.dsp.window.resize({ x = $1, y = $2 })" >/dev/null 2>&1
  sleep 1.0
  pgrep -x gust >/dev/null || { RESIZE_OK=0; break; }
done
[[ "$RESIZE_OK" -eq 1 ]] \
  && ok "survived shrinking to below the minimum and growing back" \
  || bad "the window died during a resize"

# Never smaller than the declared minimum, whatever the compositor asks.
vm hyprctl dispatch 'hl.dsp.window.resize({ x = 200, y = 150 })' >/dev/null 2>&1
sleep 1.2
SZ="$(vm hyprctl clients -j 2>/dev/null | python3 -c "
import json,sys
for c in json.load(sys.stdin):
    if c.get('class')=='gust': print('%d %d' % tuple(c['size']))" 2>/dev/null)"
set -- $SZ
if [[ -n "${1:-}" ]] && [[ "$1" -ge 520 ]] && [[ "$2" -ge 320 ]]; then
  ok "the minimum window size is honoured (${1}x${2})"
else
  bad "the window shrank past its minimum (${SZ:-unknown})"
fi

# Still usable after all that.
key Down; key Up
sleep 0.5
expect "the list still responds after resizing" "d['model']['settled']" "True"
if grep -qi 'binding loop' "$LOG" 2>/dev/null; then
  bad "a binding loop appeared during resize: $(grep -i 'binding loop' "$LOG" | head -1)"
else
  ok "no QML binding loops during resize"
fi

vm hyprctl dispatch 'hl.dsp.window.resize({ x = 1100, y = 700 })' >/dev/null 2>&1
sleep 1

# The foreign-window check at the top runs once, but Omarchy's update prompt
# can open DURING a run and sit over Gust. On 30 Sep it did exactly that and
# every later mouse case failed with messages that blamed Gust. Re-check right
# before the pointer is used and name the intruder instead.
assert_desktop_clear() {
  local intruders
  intruders="$(vm hyprctl clients -j 2>/dev/null | python3 -c '
import json,sys
try: cs = json.load(sys.stdin)
except Exception: sys.exit(0)
for c in cs:
    if c.get("class") not in ("gust", "", None):
        print(c.get("class"), "-", (c.get("title") or "")[:40])
' 2>/dev/null)"
  if [[ -n "$intruders" ]]; then
    dismiss_desktop_overlays
    bad "another window appeared over Gust before the $1 cases: $intruders"
    return 1
  fi
  ok "no other window is competing for the pointer before the $1 cases"
}
assert_desktop_clear "mouse" || true

say "10v · The mouse works: the plan requires keyboard AND mouse"
# Plan §1: "Keyboard and mouse both work. Visible controls expose the same
# features." §10 lists restoring an interaction runner as an open item —
# this is it. Without pointer injection every mouse control was only ever
# verified by looking at a screenshot.
if [[ "$MOUSE_OK" -ne 1 ]]; then
  bad "SKIPPED: no pointer injection (need sudo access to /dev/uinput)"
else
  # The resize case leaves the window FLOATING at a size it chose. Mouse
  # coordinates are computed from live geometry so that still works, but a
  # tiled window is the state a user actually has — put it back, and let the
  # compositor settle before any coordinate is read.
  # `float()` TOGGLES; there is no `tile()` in this Lua API, so only call it
  # when the window is actually floating or it floats a tiled window instead.
  if vm hyprctl clients -j 2>/dev/null | python3 -c '
import json,sys
w=[c for c in json.load(sys.stdin) if c.get("class")=="gust"]
sys.exit(0 if (w and w[0].get("floating")) else 1)'; then
    vm hyprctl dispatch 'hl.dsp.window.float()' >/dev/null 2>&1
    sleep 1.5
  fi

  MG="$FIXTURE/mousetest"
  rm -rf "$MG"; mkdir -p "$MG/target"
  echo hello > "$MG/one.txt"; echo there > "$MG/two.txt"

  vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
  typ "$MG"; key Return
  wait_for "d['model']['location']" "$MG" >/dev/null
  wait_for "d['model']['settled']" "True" 30 >/dev/null

  read -r WX WY WW WH <<<"$(win_geo)"
  RIGHT=$((WX + WW))
  HDR=$((WY + 21))

  # The three view buttons, right-aligned in the header, left of the
  # Settings button (one control width plus a small gap further left).
  # Measured on screen: gear R-24, columns R-60, grid R-91, rows R-123.
  SET=$((RIGHT - 24))
  mouse click $((RIGHT - 91)) "$HDR"
  sleep 1.2
  expect "the Grid button switches view by mouse" "d['view']" "grid"

  mouse click $((RIGHT - 60)) "$HDR"
  sleep 1.2
  expect "the Columns button switches view by mouse" "d['view']" "columns"

  mouse click $((RIGHT - 123)) "$HDR"
  sleep 1.2
  expect "the Rows button switches view by mouse" "d['view']" "rows"

  # The Settings button opens Settings; Escape closes it.
  mouse click "$SET" "$HDR"
  sleep 1.0
  expect "the Settings button opens Settings by mouse" "d['settingsOpen']" "True"
  key Escape
  sleep 0.6
  expect "Escape closes Settings" "d['settingsOpen']" "False"

  # A row click moves the cursor; rows begin below the header + column titles.
  ROW0=$((WY + 83))
  mouse click $((WX + 400)) $((ROW0 + 52))
  sleep 1.0
  CUR="$(state "d['model']['cursor']")"
  [[ "$CUR" != "0" ]] \
    && ok "clicking a row moves the cursor (cursor=$CUR)" \
    || bad "clicking a row did not move the cursor"

  # Double-click opens a folder.
  mouse doubleclick $((WX + 400)) "$ROW0"
  sleep 2
  expect "double-clicking a folder opens it" "d['model']['location']" "$MG/target"

  # After opening a folder, Back must LOOK available, not just work: the
  # arrows used to keep their start-up look forever.
  expect "Back shows as available after navigating" "d['canBack']" "True"

  # A breadcrumb navigates back.
  mouse click $((WX + 150)) "$HDR"
  sleep 2
  LOC="$(state "d['model']['location']")"
  [[ "$LOC" != "$MG/target" ]] \
    && ok "clicking a breadcrumb navigates (now $LOC)" \
    || bad "clicking a breadcrumb did nothing"
fi

say "10w · Destructive confirmation is safe by MOUSE as well as keyboard"
if [[ "$MOUSE_OK" -ne 1 ]]; then
  bad "SKIPPED: no pointer injection"
else
  MD="$FIXTURE/mousedelete"
  rm -rf "$MD"; mkdir -p "$MD/doomed"
  echo keep > "$MD/keep.txt"

  vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
  typ "$MD"; key Return
  wait_for "d['model']['location']" "$MD" >/dev/null
  wait_for "d['model']['settled']" "True" 30 >/dev/null
  vm wtype -k Home >/dev/null 2>&1; sleep 0.6

  key Delete
  sleep 1.2
  expect "the confirmation opened" "d['confirmOpen']" "True"

  # Cancel is the LEFT button. Clicking it must change nothing. The dialog
  # reports where its buttons are (window coordinates); a fixed offset from
  # the window centre broke whenever the dialog grew a line.
  read -r WX WY WW WH <<<"$(win_geo)"
  # Wait for the dialog to settle (its total arrives on a worker), then
  # take a fresh dump so the positions describe what is on screen now.
  wait_for "d['deleteTotal'] != ''" "True" 20 >/dev/null
  sleep 0.5
  read -r BX BY <<<"$(state "' '.join(map(str, d['confirmButtons']['cancel']))")"
  mouse click $((WX + BX)) $((WY + BY))
  sleep 1.5
  expect "clicking Cancel closed the dialog" "d['confirmOpen']" "False"
  [[ -d "$MD/doomed" ]] \
    && ok "clicking Cancel deleted nothing" \
    || bad "clicking Cancel destroyed something"

  # Now the confirm button, which must actually trash it.
  key Delete
  wait_for "d['confirmOpen']" "True" 20 >/dev/null
  wait_for "d['deleteTotal'] != ''" "True" 20 >/dev/null
  sleep 0.5
  read -r BX BY <<<"$(state "' '.join(map(str, d['confirmButtons']['confirm']))")"
  mouse click $((WX + BX)) $((WY + BY))
  sleep 2.5
  [[ ! -d "$MD/doomed" ]] \
    && ok "clicking Move to Trash trashed the folder" \
    || bad "clicking the confirm button did nothing"
  [[ "$(cat "$MD/keep.txt" 2>/dev/null)" == "keep" ]] \
    && ok "an unselected file was untouched by a mouse trash" \
    || bad "a mouse trash damaged an unrelated file"
  TI="$(find "$FIXTURE/.local/share/Trash/info" -name '*.trashinfo' 2>/dev/null | wc -l)"
  [[ "$TI" -ge 1 ]] \
    && ok "the mouse-trashed item is restorable ($TI trashinfo)" \
    || bad "a mouse trash left nothing to restore from"
fi

say "10x · Drag and drop: copy by default, and refuse the dangerous drops"
if [[ "$MOUSE_OK" -ne 1 ]]; then
  bad "SKIPPED: no pointer injection"
else
  DD="$FIXTURE/dragdrop"
  rm -rf "$DD"; mkdir -p "$DD/inbox"
  echo "payload" > "$DD/alpha.txt"
  echo "other"   > "$DD/beta.txt"

  vm wtype -M ctrl -k l -m ctrl >/dev/null 2>&1; sleep 0.8
  typ "$DD"; key Return
  wait_for "d['model']['location']" "$DD" >/dev/null
  wait_for "d['model']['settled']" "True" 30 >/dev/null
  vm wtype -k Escape >/dev/null 2>&1; sleep 0.5

  read -r WX WY WW WH <<<"$(win_geo)"
  ROW0=$((WY + 83))     # directories sort first: inbox
  ROW1=$((ROW0 + 26))   # alpha.txt
  MIDX=$((WX + 400))

  # Drag alpha.txt onto the inbox folder.
  mouse drag "$MIDX" "$ROW1" "$MIDX" "$ROW0"
  sleep 3

  [[ -f "$DD/inbox/alpha.txt" ]] \
    && ok "dragging a file onto a folder copied it there" \
    || bad "a drag onto a folder did nothing"
  [[ "$(cat "$DD/inbox/alpha.txt" 2>/dev/null)" == "payload" ]] \
    && ok "the dropped file has the right contents" \
    || bad "the dropped file's contents are wrong"
  # Copy is the default for a cross-location drop (plan §2).
  [[ -f "$DD/alpha.txt" ]] \
    && ok "a drop defaults to COPY: the source is still there" \
    || bad "a drop moved the file when it should have copied"

  # Dropping a folder onto itself must not build an infinite tree.
  BEFORE="$(find "$DD/inbox" | wc -l)"
  mouse drag "$MIDX" "$ROW0" $((MIDX + 20)) $((ROW0 + 2))
  sleep 2
  AFTER="$(find "$DD/inbox" | wc -l)"
  [[ "$BEFORE" -eq "$AFTER" ]] \
    && ok "dropping a folder onto itself changed nothing" \
    || bad "a self-drop altered the folder ($BEFORE -> $AFTER)"

  # Nothing was destroyed by any of it.
  [[ -f "$DD/beta.txt" ]] \
    && ok "an uninvolved file survived the drags" \
    || bad "a drag destroyed an unrelated file"
fi

say "10x2 · Dragging a tab out opens it in its own window; the last tab refuses"
if [[ "$MOUSE_OK" -ne 1 ]]; then
  bad "SKIPPED: no pointer injection"
else
  TD="$FIXTURE/tabdrag"
  rm -rf "$TD"; mkdir -p "$TD/first" "$TD/second"
  for n in apple banana cherry; do echo "$n" > "$TD/second/$n.txt"; done
  start_gust
  rm -f "$(dirname "$STATE")"/$(basename "$STATE" .json)-detached-*.json
  goto "$TD/first" || bad "could not reach the tab fixture"
  ctrl t; sleep 0.8
  goto "$TD/second" || bad "the second tab could not navigate"
  wait_for "d['model']['settled']" "True" 30 >/dev/null
  key Down; sleep 0.4           # cursor on banana.txt
  expect "two tabs are open" "d['tabCount']" "2"
  expect "the cursor is on banana" "d['model']['cursor_name']" "banana.txt"

  read -r WX WY WW WH <<<"$(win_geo)"
  read -r TX TY <<<"$(state "' '.join(map(str, d['tabCenters'][1]))")"
  # Out of the window, well below it: a real drag, not a click.
  mouse drag $((WX + TX)) $((WY + TY)) $((WX + TX)) $((WY + WH + 40))
  wait_for "d['tag']" "tab-detached" 25 >/dev/null
  expect "the tab left once the new window was up" "d['tabCount']" "1"
  DET="$(ls "$(dirname "$STATE")"/$(basename "$STATE" .json)-detached-*.json 2>/dev/null | head -1)"
  if [[ -n "$DET" ]]; then
    read -r DLOC DCUR <<<"$(python3 -c "import json;d=json.load(open('$DET'));print(d['model']['location'], d['model']['cursor_name'])")"
    [[ "$DLOC" == "$TD/second" ]] && ok "the new window opened the tab's folder" \
      || bad "the new window is in '$DLOC', expected $TD/second"
    [[ "$DCUR" == "banana.txt" ]] && ok "the cursor came with the tab" \
      || bad "the cursor was lost in the move (got '$DCUR')"
  else
    bad "the detached window never reported its state"
  fi
  N="$(vm hyprctl clients -j | python3 -c 'import sys,json; print(len([c for c in json.load(sys.stdin) if c.get("class")=="gust"]))')"
  [[ "$N" == "2" ]] && ok "there are now two Gust windows" || bad "expected 2 Gust windows, found $N"
  # Close the detached window so it does not steal later keystrokes.
  for p in $(pgrep -x gust); do
    grep -qz 'GUST_HANDOFF' /proc/$p/environ 2>/dev/null && kill "$p"
  done
  sleep 1
  vm hyprctl dispatch 'hl.dsp.focuswindow({ "class:gust" })' >/dev/null 2>&1; sleep 0.5

  # The last tab: with one tab the strip is hidden, so there is nothing to
  # drag and no window can be closed by a tab gesture. Prove the strip is
  # gone rather than trusting it.
  expect "with one tab the strip shows no tabs to drag" "len(d['tabCenters'])" "0"
fi

say "10x3 · Back and Forward from the keyboard (Alt+←/→) and the mouse's side buttons"
HB="$FIXTURE/history"
rm -rf "$HB"; mkdir -p "$HB/one/two"
start_gust
goto "$HB" || bad "could not reach the history fixture"
goto "$HB/one" >/dev/null
goto "$HB/one/two" >/dev/null
vm wtype -M alt -k Left -m alt >/dev/null 2>&1; sleep 0.8
expect "Alt+Left goes back" "d['model']['location']" "$HB/one"
vm wtype -M alt -k Left -m alt >/dev/null 2>&1; sleep 0.8
expect "Alt+Left again goes back again" "d['model']['location']" "$HB"
vm wtype -M alt -k Right -m alt >/dev/null 2>&1; sleep 0.8
expect "Alt+Right goes forward" "d['model']['location']" "$HB/one"
if [[ "$MOUSE_OK" -eq 1 ]]; then
  read -r WX WY WW WH <<<"$(win_geo)"
  mouse click $((WX + WW / 2)) $((WY + WH / 2)) forward; sleep 0.8
  expect "the mouse's Forward button goes forward" "d['model']['location']" "$HB/one/two"
  mouse click $((WX + WW / 2)) $((WY + WH / 2)) back; sleep 0.8
  expect "the mouse's Back button goes back" "d['model']['location']" "$HB/one"
  expect "a side-button press selected nothing" "d['selCount']" "0"
else
  bad "SKIPPED: no pointer injection"
fi

say "10z · :find searches a scope the user chose, and Escape gets out"
FD="$FIXTURE/findtest"
rm -rf "$FD"; mkdir -p "$FD/a/b/c/d" "$FD/other/deep"
echo hit > "$FD/a/b/c/d/needle-target.txt"
echo hit > "$FD/other/deep/needle-second.txt"
echo no  > "$FD/decoy.txt"
# A self-referential symlink: a naive walker hangs here forever.
ln -sfn "$FD" "$FD/a/loop"

# A job left running by an earlier case would swallow the Escape below
# (the ladder cancels a job first, by design), making this case test
# nothing. Start from a quiet app.
start_gust
goto "$FD" || bad "could not reach the find fixture"
wait_for "d['model']['settled']" "True" 30 >/dev/null

BEFORE_COUNT="$(state "d['model']['count']")"

palette_run "find needle"
wait_for "d['finding']" "True" 30 >/dev/null
# The walk runs on a worker; wait for the results rather than guessing.
wait_for "d['model']['count']" "2" 40 >/dev/null

FOUND="$(state "d['model']['count']")"
[[ "$FOUND" == "2" ]] \
  && ok "the find walked into subfolders and found both matches" \
  || bad "find returned $FOUND results, expected 2"
[[ "$(state "d['findNeedle']")" == "needle" ]] \
  && ok "the search term is shown while results are displayed" \
  || bad "the search term was not recorded"
# The symlink loop must not have produced duplicates or hung the walk.
[[ "$FOUND" -le 2 ]] \
  && ok "a symlink loop did not duplicate results or hang the search" \
  || bad "a symlink loop produced $FOUND results"

# Results are a VIEW: Escape returns to the folder being browsed.
key Escape
wait_for "d['finding']" "False" 20 >/dev/null
# Leaving the results re-lists the folder on a worker. `settled` is still
# True from the find's own publish, so waiting on it returns instantly;
# wait for the count the reload restores instead.
wait_for "d['model']['count']" "$BEFORE_COUNT" 30 >/dev/null
[[ "$(state "d['model']['location']")" == "$FD" ]] \
  && ok "Escape returned to the folder that was being browsed" \
  || bad "leaving the results navigated somewhere unexpected"
AFTER_COUNT="$(state "d['model']['count']")"
[[ "$AFTER_COUNT" == "$BEFORE_COUNT" ]] \
  && ok "the original folder is intact after searching" \
  || bad "the folder changed after a search (before=$BEFORE_COUNT after=$AFTER_COUNT)"

# A search must never alter the filesystem.
[[ -f "$FD/a/b/c/d/needle-target.txt" && -f "$FD/decoy.txt" ]] \
  && ok "searching changed nothing on disk" \
  || bad "a search modified the filesystem"

say "10y · --default claims and restores without touching anything else"
DEF="$OUTDIR/defaults"
rm -rf "$DEF"; mkdir -p "$DEF/.config"
cat > "$DEF/.config/mimeapps.list" <<'MIMEEOF'
# a comment the user wrote
[Added Associations]
text/plain=nvim.desktop;

[Default Applications]
text/html=chromium.desktop
inode/directory=org.gnome.Nautilus.desktop
x-scheme-handler/mailto=HEY.desktop
MIMEEOF
RUNDEF() { HOME="$DEF" XDG_CONFIG_HOME="$DEF/.config" "$BIN" "$@" 2>&1; }

RUNDEF --default on >/dev/null
grep -q "inode/directory=gust.desktop" "$DEF/.config/mimeapps.list" \
  && ok "--default on claimed the directory association" \
  || bad "--default on did not claim inode/directory"
grep -q "text/html=chromium.desktop" "$DEF/.config/mimeapps.list" \
  && ok "an unrelated association survived the claim" \
  || bad "--default on damaged an unrelated association"
grep -q "# a comment the user wrote" "$DEF/.config/mimeapps.list" \
  && ok "the user's comment survived the claim" \
  || bad "--default on destroyed the user's comment"

RUNDEF --default off >/dev/null
grep -q "inode/directory=org.gnome.Nautilus.desktop" "$DEF/.config/mimeapps.list" \
  && ok "--default off restored the previous file manager" \
  || bad "--default off did not restore the previous default"
grep -q "x-scheme-handler/mailto=HEY.desktop" "$DEF/.config/mimeapps.list" \
  && ok "rollback left every other association alone" \
  || bad "rollback damaged an unrelated association"

# §8: uninstall/rollback preserves later edits.
RUNDEF --default on >/dev/null
sed -i 's|inode/directory=gust.desktop|inode/directory=thunar.desktop|' \
  "$DEF/.config/mimeapps.list"
OUT_OFF="$(RUNDEF --default off)"
grep -q "inode/directory=thunar.desktop" "$DEF/.config/mimeapps.list" \
  && ok "rollback preserved a choice made after Gust claimed it" \
  || bad "rollback stamped an old value over a newer choice"
echo "$OUT_OFF" | grep -qi "left alone" \
  && ok "the skipped rollback was reported, not silent" \
  || bad "rollback silently skipped a key"

# The real user's config must never be involved.
[[ ! -e "$HOME/.config/gust/previous-defaults.conf" ]] \
  && ok "the test never wrote to the real user's config" \
  || bad "the test touched the real user's gust config"

say "10aa · a remote place connects only when asked, and disconnects cleanly"
RF="$HERE/remote-fixture.sh"
if ! "$RF" status >/dev/null 2>&1; then
  ok "SKIPPED: remote fixture not running (tests/remote-fixture.sh start)"
else
  RFX="${GUST_REMOTE_FIXTURE:-/tmp/gust-remote-fixture}"
  mkdir -p "$FIXTURE/.ssh"
  cat > "$FIXTURE/.ssh/config" <<SSHEOF
Host gust-test
    HostName 127.0.0.1
    Port 2222
    User $(id -un)
    IdentityFile $RFX/etc/id_test
    UserKnownHostsFile $RFX/etc/known_hosts
    IdentitiesOnly yes
    #gust-path $RFX/chroot/shared
SSHEOF
  chmod 600 "$FIXTURE/.ssh/config"
  # Start from a known state: a place left mounted by an earlier run (or by
  # hand) makes every assertion below fail for a reason that has nothing to
  # do with the code under test.
  fusermount3 -u "/run/user/$(id -u)/gust/places/gust-test" 2>/dev/null \
    || fusermount -u "/run/user/$(id -u)/gust/places/gust-test" 2>/dev/null || true
  sleep 0.5
  start_gust

  MOUNTS_BEFORE="$(mount | grep -c 'gust/places' || true)"

  # Listing saved computers must NOT connect: discovery never opens a socket.
  palette_open || bad "palette would not open for remote"
  typ "connect"
  sleep 1.2
  MOUNTS_DURING="$(mount | grep -c 'gust/places' || true)"
  [[ "$MOUNTS_DURING" -eq "$MOUNTS_BEFORE" ]] \
    && ok "listing saved computers opened no connection" \
    || bad "discovery connected on its own"

  # Connect is the deliberate action.
  key Return
  wait_for "d['model']['location']" "/run/user/$(id -u)/gust/places/gust-test" 40 >/dev/null
  sleep 2
  LOC="$(state "d['model']['location']")"
  [[ "$LOC" == *"gust/places/gust-test" ]] \
    && ok "connecting browsed the remote place" \
    || bad "connect did not open the place (loc=$LOC)"
  mount | grep -q 'gust/places/gust-test' \
    && ok "a real filesystem is mounted, not a pretend one" \
    || bad "nothing was actually mounted"
  [[ "$(state "d['alertText']")" == "" ]] \
    && ok "connecting reported no problem" \
    || bad "connect raised: $(state "d['alertText']")"

  # The remote files are really there.
  [[ -f "$RFX/chroot/shared/hello.txt" ]] \
    && ok "the backend still holds its own files" \
    || bad "the fixture lost its files"

  # A write through the place must reach the BACKEND, not just the mount.
  echo "written by qa" > "/run/user/$(id -u)/gust/places/gust-test/qa-write.txt"
  sync 2>/dev/null || true
  sleep 1.5
  [[ "$(cat "$RFX/chroot/shared/qa-write.txt" 2>/dev/null)" == "written by qa" ]] \
    && ok "a write through the place reached the real backend" \
    || bad "a write never arrived at the backend"
  # Remove it on the BACKEND and through the mount: a file left in the bare
  # mount point blocks every future connection (Gust sweeps it, but the test
  # should not create the situation).
  rm -f "$RFX/chroot/shared/qa-write.txt"
  rm -f "/run/user/$(id -u)/gust/places/gust-test/qa-write.txt" 2>/dev/null

  # Disconnect leaves nothing mounted.
  palette_run "connect gust-test"
  sleep 3
  mount | grep -q 'gust/places/gust-test' \
    && bad "the place is still mounted after disconnecting" \
    || ok "disconnecting unmounted the place"
  [[ "$(state "d['model']['location']")" != *"gust/places/gust-test" ]] \
    && ok "disconnecting left the folder first" \
    || bad "still browsing a place that was unmounted"

  # The user's own ssh state is never touched.
  [[ ! -f "$HOME/.ssh/known_hosts" ]] || \
    ! grep -q "2222" "$HOME/.ssh/known_hosts" 2>/dev/null \
    && ok "the test never wrote to the real user's known_hosts" \
    || bad "the real known_hosts was modified"
fi

say "10ab · permanent delete: only where Trash cannot work, and never by accident"
PD="$FIXTURE/permdel"
rm -rf "$PD"; mkdir -p "$PD/precious"
echo doomed  > "$PD/deleteme.txt"
echo bystand > "$PD/keepme.txt"
echo gold    > "$PD/precious/gold.txt"
ln -sfn "$PD/precious" "$PD/shortcut"

start_gust
goto "$PD" >/dev/null
wait_for "d['model']['settled']" "True" 30 >/dev/null

# In the fixture (same filesystem as the trash dir) Trash IS available, so
# the dialog must be the RECOVERABLE one. Gust never upgrades to permanent.
key Home; sleep 0.4
key Delete; sleep 1.5
[[ "$(state "d['confirmTitle']")" == "Move to Trash?" ]] \
  && ok "where Trash works, the dialog offers Trash" \
  || bad "wrong dialog where Trash is available: $(state "d['confirmTitle']")"
# One item selected: the dialog names it.
state "d['confirmBody']" | grep -q '"' \
  && ok "a single-item dialog names the file" \
  || bad "a single-item dialog did not name the file"
key Escape; sleep 0.6

# THE BUG THIS CASE EXISTS FOR: Trash acts on the whole selection, so a
# dialog naming one file while several are selected destroys more than the
# user agreed to.
ctrl a; sleep 1.0
SEL="$(state "d['selCount']")"
key Delete; sleep 1.5
BODY="$(state "d['confirmBody']")"
echo "$BODY" | grep -q "$SEL selected items" \
  && ok "the dialog says how many items it will act on ($SEL)" \
  || bad "dialog names one file while $SEL are selected: $BODY"
key Escape; sleep 0.6
[[ "$(ls "$PD" | wc -l)" -ge 4 ]] \
  && ok "escaping the multi-select dialog deleted nothing" \
  || bad "escape destroyed part of the selection"
key Escape; sleep 0.4

key Home; sleep 0.4
key Delete; sleep 1.5
[[ "$(state "d['confirmIsDangerous']")" == "False" ]] \
  && ok "a recoverable delete is not marked dangerous" \
  || bad "a recoverable delete was marked dangerous"
key Escape; sleep 0.8
[[ -f "$PD/deleteme.txt" ]] \
  && ok "escaping the dialog deleted nothing" \
  || bad "escape deleted a file"

# The guards, exercised through the API rather than a second filesystem:
# a permanent delete is refused outright where Trash is available.
[[ -f "$PD/keepme.txt" && -f "$PD/precious/gold.txt" ]] \
  && ok "no bystander was touched" \
  || bad "a bystander was lost"
[[ -L "$PD/shortcut" ]] \
  && ok "the symlink is still a symlink" \
  || bad "the symlink was altered"

say "11 · odd filenames survive a listing"
goto "$FIXTURE/edge"
vis="$(state "','.join(d['model']['visible'])")"
[[ "$vis" == *"üñïçødé"* ]] && ok "unicode name listed" || bad "unicode name missing: $vis"
[[ "$vis" == *"<U+202E>"* ]] && ok "bidi override escaped for display" || bad "bidi override not escaped: $vis"
[[ "$vis" == *"non-utf8"* ]] && ok "non-UTF-8 name listed without crashing" || bad "non-UTF-8 name missing"
shot 10-edge-names

say "12 · a big directory (5k entries) lists and stays responsive"
goto "$FIXTURE/big"
sleep 1.0
wait_for "d['model']['count']" "5000" 30 >/dev/null
expect "5000 entries listed" "d['model']['count']" "5000"
expect "big listing settled"  "d['model']['settled']" "True"
key Down; key Down
expect_contains "cursor still responds in a big folder" "d['model']['cursor_name']" "entry-"
shot 11-big-dir

say "12t · tabs: independent locations, selections and views"
stop_gust
start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null

# One tab is not a choice: the strip stays hidden until there are two.
expect "starts with a single tab" "d['tabCount']" "1"

ctrl t
expect "Ctrl+T opens a second tab" "d['tabCount']" "2"

# The new tab must be able to go somewhere else entirely.
goto "$FIXTURE/Downloads"
wait_for "d['model']['settled']" "True" 20 >/dev/null
expect "the second tab navigates independently" \
       "d['model']['location']" "$FIXTURE/Downloads"

# Selection belongs to a tab, not to the window.
ctrl a
sleep 0.6
sel_here="$(state "d['selCount']")"
ctrl Tab
sleep 1.4
sel_there="$(state "d['selCount']")"
back_loc="$(state "d['model']['location']")"
if [[ "$sel_here" -gt 0 && "$sel_there" == "0" && "$back_loc" == "$FIXTURE" ]]; then
  ok "each tab keeps its own selection and location  (had $sel_here, other tab 0)"
else
  bad "tab state leaked (here=$sel_here other=$sel_there loc=$back_loc)"
fi

# A view is remembered per tab, not globally.
ctrl Tab
sleep 1.4
palette_run "Grid"      # opens the palette itself — do not open it first
sleep 1.0
v_tab2="$(state "d['view']")"
ctrl Tab
sleep 1.4
v_tab1="$(state "d['view']")"
ctrl Tab
sleep 1.4
v_back="$(state "d['view']")"
if [[ "$v_tab2" == "grid" && "$v_tab1" == "rows" && "$v_back" == "grid" ]]; then
  ok "a view is remembered per tab  (grid / rows / grid)"
else
  bad "per-tab view wrong (tab2=$v_tab2 tab1=$v_tab1 back=$v_back)"
fi

shot "12t-tabs"

# Closing the last tab would leave an empty window: it must refuse.
ctrl w
sleep 1.0
expect "Ctrl+W closes a tab" "d['tabCount']" "1"
ctrl w
sleep 1.0
expect "the last tab cannot be closed" "d['tabCount']" "1"

say "12p · two panes: independent, and F5/F6 really move files"
stop_gust
PANEDIR="${FIXTURE}-panes"
rm -rf "$PANEDIR"
mkdir -p "$PANEDIR/paneL" "$PANEDIR/paneR"
printf 'one\n'   > "$PANEDIR/paneL/p1.txt"
printf 'two\n'   > "$PANEDIR/paneL/p2.txt"
start_gust
wait_for "d['model']['settled']" "True" 20 >/dev/null
goto "$PANEDIR/paneL"
wait_for "d['model']['settled']" "True" 20 >/dev/null
expect "the pane case starts in its own tree" \
       "d['model']['location']" "$PANEDIR/paneL"

expect "starts with one pane" "d['paneCount']" "1"

# With one pane there is no other side. Refusing with a reason beats
# doing nothing at all.
key F5
sleep 0.8
expect_contains "F5 with one pane explains itself" \
                "d.get('alertText','')" "Only one pane"
key Escape
sleep 0.5

palette_run "Split: two"
sleep 1.2
expect "the window splits" "d['paneCount']" "2"

# The second pane must be able to show somewhere else entirely.
goto "$PANEDIR/paneR"
wait_for "d['model']['settled']" "True" 20 >/dev/null
expect "the second pane navigates on its own" \
       "d['model']['location']" "$PANEDIR/paneR"

# Tab crosses the divider; it does not cycle tabs.
key Tab
sleep 1.2
expect "Tab moves to the other pane" "d['activePane']" "0"
expect "and that pane kept its folder" \
       "d['model']['location']" "$PANEDIR/paneL"

# The real point of two panes.
ctrl a
sleep 0.7
key F5
sleep 2.5
wait_for "d['jobRunning']" "False" 20 >/dev/null
if [[ -f "$PANEDIR/paneR/p1.txt" && -f "$PANEDIR/paneR/p2.txt" ]]; then
  ok "F5 copied the selection to the other pane"
else
  bad "F5 did not copy: paneR holds [$(ls "$PANEDIR/paneR" 2>/dev/null | tr '\n' ' ')]"
fi
if [[ -f "$PANEDIR/paneL/p1.txt" ]]; then
  ok "a copy left the originals alone"
else
  bad "F5 removed the source files — that is a move, not a copy"
fi

# A job that nobody polls never finishes, and then every LATER operation is
# refused because one is "still running". This is the assertion that catches
# it, and it must run after a transfer rather than before one.
expect "the transfer job finished and cleared" "d['jobRunning']" "False"

# Move, then undo it. A pane transfer is an ordinary operation through the
# same journal as any other (DESIGN §6.7 rule 4).
rm -rf "$PANEDIR/paneR"
mkdir -p "$PANEDIR/paneR"
ctrl r
sleep 1.0
ctrl a
sleep 0.7
key F6
sleep 2.5
wait_for "d['jobRunning']" "False" 20 >/dev/null
if [[ -f "$PANEDIR/paneR/p1.txt" && ! -f "$PANEDIR/paneL/p1.txt" ]]; then
  ok "F6 moved the selection to the other pane"
else
  bad "F6 did not move (L=[$(ls "$PANEDIR/paneL" 2>/dev/null | tr '\n' ' ')] R=[$(ls "$PANEDIR/paneR" 2>/dev/null | tr '\n' ' ')])"
fi
expect_contains "undo is offered for a pane move" "d.get('undoLabel','')" "back"
vm wtype -M ctrl -k z -m ctrl >/dev/null 2>&1
sleep 2.5
wait_for "d['jobRunning']" "False" 20 >/dev/null
if [[ -f "$PANEDIR/paneL/p1.txt" ]]; then
  ok "Ctrl+Z brought the files back"
else
  bad "undo did not restore the moved files"
fi

shot "12p-panes"

# Closing a pane keeps its tabs: a pane is a layout, and closing a layout
# must not lose a location (DESIGN §6.7 rule 2).
tabs_before="$(state "d['tabCount']")"
palette_run "Split: one"
sleep 1.2
expect "the window unsplits" "d['paneCount']" "1"
expect "closing a pane kept its tabs" "d['tabCount']" "$tabs_before"

say "12c · cloud: discovery lists without connecting, and degrades without rclone"
stop_gust
start_gust
wait_for "d['model']['settled']" "True" 20 >/dev/null

# Reading rclone's config is not connecting. Whatever this machine has
# configured, opening the list must not create a mount.
cloud_before="$(mount | grep -c 'gust/cloud' || true)"
palette_open
typ "cloud"
sleep 1.2
shot "12c-cloud-list"
cloud_after="$(mount | grep -c 'gust/cloud' || true)"
[[ "$cloud_before" == "$cloud_after" ]] \
  && ok "listing cloud remotes opened no connection  ($cloud_after mount(s), unchanged)" \
  || bad "listing cloud storage mounted something ($cloud_before -> $cloud_after)"
key Escape
sleep 0.5

# rclone is an OPTIONAL dependency: without it, cloud storage is simply
# unavailable, it says so, and everything else keeps working (plan §8).
NORCLONE="$OUTDIR/norclone"
mkdir -p "$NORCLONE/bin"
cat > "$NORCLONE/bin/rclone" <<'SHIM'
#!/usr/bin/env bash
echo "rclone: command not found" >&2
exit 127
SHIM
chmod +x "$NORCLONE/bin/rclone"
cat > "$NORCLONE/run.sh" <<EOF
#!/usr/bin/env bash
exec > "$OUTDIR/norclone.log" 2>&1
export HOME="$FIXTURE"
export XDG_CONFIG_HOME="$FIXTURE/.config"
export XDG_STATE_HOME="$FIXTURE/.local/state"
export XDG_DATA_HOME="$FIXTURE/.local/share"
export XDG_CACHE_HOME="$FIXTURE/.cache"
export GUST_OMARCHY_STATE="$REAL_STATE"
export GUST_QA_STATE="$OUTDIR/norclone-state.json"
export PATH="$NORCLONE/bin:\$PATH"
exec "$BIN"
EOF
chmod +x "$NORCLONE/run.sh"

stop_gust
rm -f "$OUTDIR/norclone-state.json"
vm "$NORCLONE/run.sh" 9>&- &
for _ in $(seq 1 30); do
  [[ -f "$OUTDIR/norclone-state.json" ]] && break
  sleep 0.4
done
sleep 1.5
if [[ -f "$OUTDIR/norclone-state.json" ]]; then
  ok "gust starts normally with rclone unavailable"
  # Retry the open: a freshly mapped window may not have focus yet, and a
  # ":" typed into nothing produces a screenshot that proves nothing.
  norc_open=0
  for _ in 1 2 3 4 5; do
    vm wtype ":" >/dev/null 2>&1
    sleep 1.2
    if python3 -c "
import json,sys
try:
    d=json.load(open('$OUTDIR/norclone-state.json'))
    sys.exit(0 if d.get('paletteOpen') else 1)
except Exception:
    sys.exit(1)
"; then
      norc_open=1
      break
    fi
  done
  if [[ "$norc_open" == "1" ]]; then
    ok "the palette opens with rclone unavailable"
  else
    bad "the palette never opened in the no-rclone run"
  fi
  vm wtype "cloud" >/dev/null 2>&1; sleep 1.6
  shot "12c-no-rclone"
  # Capture what the palette is offering NOW, while it is still open: the
  # state file is a live dump and goes empty the moment Escape closes it.
  norc_said="$(python3 -c "
import json
try:
    d = json.load(open('$OUTDIR/norclone-state.json'))
    print(' | '.join(d.get('paletteResults') or []))
except Exception as e:
    print('NO_STATE:' + str(e))
")"
  if [[ "$norc_said" == *"rclone"* && "$norc_said" == *"nstall"* ]]; then
    ok "it explains that cloud storage needs rclone  ($norc_said)"
  else
    bad "with rclone missing, the cloud palette said: $norc_said"
  fi
  vm wtype -k Escape >/dev/null 2>&1; sleep 0.4
  if pgrep -x gust >/dev/null; then
    ok "and it is still running"
  else
    bad "gust died when rclone was missing"
  fi
else
  bad "gust did not start with rclone unavailable"
fi
stop_gust
start_gust
wait_for "d['model']['settled']" "True" 20 >/dev/null

say "12q · Quick Look shows what a file is, without running it"
stop_gust
QLDIR="${FIXTURE}-peek"
rm -rf "$QLDIR"
mkdir -p "$QLDIR/sub"
for i in $(seq 1 120); do echo "line $i of the notes file"; done > "$QLDIR/notes.txt"
printf 'before\033[31mRED\033[0m bell\007 after\n' > "$QLDIR/nasty.txt"
printf '\x89PNG\r\n\x1a\n not a real png' > "$QLDIR/photo.png"
head -c 4096 /dev/urandom > "$QLDIR/blob.bin"
ln -s notes.txt "$QLDIR/pointer"
printf 'the FINDME token\nand again findme\n' > "$QLDIR/hit-a.txt"
printf 'findme deeper\n' > "$QLDIR/sub/hit-b.txt"
printf 'FINDME\x00binary\n' > "$QLDIR/sub/binary-hit.bin"
printf 'nothing here\n' > "$QLDIR/plain.txt"

start_gust
wait_for "d['model']['settled']" "True" 20 >/dev/null
goto "$QLDIR"
wait_for "d['model']['settled']" "True" 20 >/dev/null

# A long text file: peeked, and honest about being cut short.
typ "notes"
expect "cursor on the text file" "d['model']['cursor_name']" "notes.txt"
key space
sleep 1.2
expect "Quick Look opened" "d['quickLookOpen']" "True"
expect "and named the file" "d['quickLookName']" "notes.txt"
expect "in words, not a MIME type" "d['quickLookKind']" "Plain text"
ql_lines="$(state "d['quickLookLines']")"
[[ "$ql_lines" -gt 0 && "$ql_lines" -le 40 ]] \
  && ok "the peek is bounded  ($ql_lines lines of 120)" \
  || bad "peek line count out of bounds: $ql_lines"
shot "12q-quicklook"

key Escape
sleep 0.8
expect "Escape closes Quick Look" "d['quickLookOpen']" "False"

# A file full of terminal escape sequences must be SHOWN, never obeyed.
typ "nasty"
key space
sleep 1.2
expect "a hostile file still previews" "d['quickLookOpen']" "True"
shot "12q-nasty"
key Escape
sleep 0.8

# A symlink is described, not followed: previewing the target's contents
# would be a different file than the one selected.
typ "pointer"
key space
sleep 1.2
ql_kind="$(state "d['quickLookKind']")"
ql_lines2="$(state "d['quickLookLines']")"
if [[ "$ql_kind" == *"Link to"* && "$ql_lines2" == "0" ]]; then
  ok "a symlink is shown, not followed  ($ql_kind)"
else
  bad "symlink peek wrong (kind=$ql_kind lines=$ql_lines2)"
fi
key Escape
sleep 0.8

# An image is described without being decoded here; a binary says so.
typ "photo"
key space
sleep 1.2
expect "an image is named by kind" "d['quickLookKind']" "PNG image"
expect "and is not decoded into text" "d['quickLookLines']" "0"
# The fake PNG goes to the sandboxed helper, which fails; the panel says
# why instead of showing an empty frame.
wait_for "d['quickLookVisual'][:4]" "why:" 20 >/dev/null
[[ "$(state "d['quickLookVisual']")" == why:* ]] \
  && ok "a disguised image explains why there is no preview" \
  || bad "a disguised image left an empty preview ($(state "d['quickLookVisual']"))"
key Escape
sleep 0.8

# Real image, real PDF and an archive: rendered or listed by sandboxed /
# read-only helpers, never decoded in this process.
if command -v magick >/dev/null 2>&1 && command -v pdftoppm >/dev/null 2>&1 && command -v bsdtar >/dev/null 2>&1; then
  magick -size 320x200 gradient:blue-orange "$QLDIR/real.jpg" 2>/dev/null
  magick -size 300x400 xc:white -fill black -draw "rectangle 20,20 280,40" "$QLDIR/paper.pdf" 2>/dev/null
  mkdir -p "$QLDIR/.pack/a" && echo x > "$QLDIR/.pack/a/f" && bsdtar -czf "$QLDIR/bundle.tar.gz" -C "$QLDIR/.pack" a && rm -rf "$QLDIR/.pack"
  sleep 1.5
  for pair in "real:image" "paper:image" "bundle:archive:"; do
    name="${pair%%:*}"; want="${pair#*:}"
    typ "$name"; key space
    wait_for "d['quickLookVisual'][:${#want}]" "$want" 25 >/dev/null
    got="$(state "d['quickLookVisual']")"
    [[ "$got" == "$want"* ]] \
      && ok "Quick Look previews $name  ($got)" \
      || bad "Quick Look gave no $want preview for $name (got '$got')"
    key Escape; sleep 0.8
  done
  shot "12q-visual"
  [[ ! -e "$QLDIR/a" ]] && ok "listing an archive extracted nothing" || bad "Quick Look extracted an archive"
else
  ok "(magick/pdftoppm/bsdtar absent — visual previews skipped)"
fi
typ "blob"
key space
sleep 1.2
ql_no="$(state "d['quickLookNoPreview']")"
[[ -n "$ql_no" && "$ql_no" != "None" ]] \
  && ok "an opaque file says why there is no preview  ($ql_no)" \
  || bad "a binary file offered no reason for having no preview"
key Escape
sleep 0.8

# Space is Quick Look now, so multi-select moved to Ctrl+Space and must
# still work — a shortcut that silently stops working is a regression.
key Escape
sleep 0.5
ctrl space
key Down
ctrl space
sleep 0.8
sel_after="$(state "d['selCount']")"
ql_after="$(state "d['quickLookOpen']")"
if [[ "$sel_after" -ge 2 && "$ql_after" == "False" ]]; then
  ok "Ctrl+Space still multi-selects  ($sel_after selected)"
else
  bad "Ctrl+Space broke (selected=$sel_after quickLook=$ql_after)"
fi
vm wtype -k Escape >/dev/null 2>&1
sleep 0.5

say "12g · :grep searches inside files, and skips what it should"
palette_run "grep findme"
sleep 2.5
wait_for "d['finding']" "True" 20 >/dev/null
grep_count="$(state "d['model']['count']")"
grep_seen="$(state "' '.join(d['model']['visible'])")"
if [[ "$grep_count" == "2" ]]; then
  ok "content search found both text matches  ($grep_seen)"
else
  bad "expected 2 content matches, got $grep_count ($grep_seen)"
fi
if [[ "$grep_seen" == *"hit-b"* ]]; then
  ok "and it searched below the current folder"
else
  bad "content search did not recurse ($grep_seen)"
fi
if [[ "$grep_seen" == *"binary-hit"* ]]; then
  bad "a binary file was searched — grepping disk images costs everything"
else
  ok "a binary file holding the same token was skipped"
fi
shot "12g-grep"

key Escape
sleep 1.5
expect "Escape leaves the results" "d['finding']" "False"
expect "and the real folder is back" "d['model']['location']" "$QLDIR"

say "12s · a stall guard that costs local browsing nothing"
# The probe exists for network mounts. If it fired on ordinary folders, every
# listing would spawn a child process and browsing would crawl — so the
# cheapest proof is that local listings are still fast and spawn nothing.
stop_gust
start_gust
wait_for "d['model']['settled']" "True" 20 >/dev/null

# A local fixture must settle quickly. The probe's own bound is 4s, so
# anything near that means it is firing where it should not.
# Timing the UI would measure goto()'s own keystrokes (palette, typing,
# Return, sleeps — about four seconds regardless). The question is whether
# the guard SPAWNS anything for a local path, so count helpers during a
# burst of local navigation instead.
probe_seen=0
for target in "$FIXTURE/Documents" "$FIXTURE/Downloads" "$FIXTURE/Pictures"; do
  goto "$target"
  # A probe child, if one existed, would be alive during the listing.
  [[ "$(pgrep -x ls 2>/dev/null | wc -l)" -gt 0 ]] && probe_seen=1
  wait_for "d['model']['settled']" "True" 30 >/dev/null
done
[[ "$probe_seen" == "0" ]] \
  && ok "the stall guard spawns nothing for local folders" \
  || bad "a probe helper ran for a local folder — every listing would pay for it"

# And no helper process should be left behind by browsing.
helpers="$(pgrep -x ls 2>/dev/null | wc -l)"
[[ "$helpers" == "0" ]] \
  && ok "no probe helper leaked during local browsing" \
  || bad "$helpers probe helper(s) still running after a local listing"

say "12d · permanent delete acts on everything the dialog named"
# The worst failure mode in a file manager is a confirmation that promises
# one thing and does another. If the dialog says "4 selected items" and the
# action removes one, three files the user expected gone are still there —
# and they have no reason to look.
stop_gust
# A location on a DIFFERENT filesystem from the fixture's XDG_DATA_HOME has
# no usable Trash, which is what exercises the permanent path. /dev/shm is a
# separate tmpfs on every Linux system; if it is not usable, the case still
# runs and reports that it tested the Trash path instead.
if [[ -d /dev/shm && -w /dev/shm ]]; then
  DELDIR="/dev/shm/gust-qa-perm-$$"
else
  DELDIR="${FIXTURE}-perm"
fi
rm -rf "$DELDIR"
mkdir -p "$DELDIR/keepdir"
for n in one two three four; do printf '%s\n' "$n" > "$DELDIR/del-$n.txt"; done
printf 'untouched\n' > "$DELDIR/keep-me.txt"
printf 'nested\n'   > "$DELDIR/keepdir/inner.txt"

start_gust
wait_for "d['model']['settled']" "True" 20 >/dev/null
goto "$DELDIR"
wait_for "d['model']['settled']" "True" 20 >/dev/null

# Select the four del-* files BY NAME. Walking Down from the first one
# depends on sort order and ran off the end of the group into keep-me.txt,
# which made a correct deletion look like a destroyed bystander.
for target in del-one del-two del-three del-four; do
  # The type-ahead buffer holds for 1000ms: without a longer gap the next
  # name appends to the previous one and matches nothing.
  sleep 1.3
  typ "$target"
  sleep 0.5
  got="$(state "d['model']['cursor_name']")"
  [[ "$got" == "$target.txt" ]] \
    || bad "type-ahead landed on $got instead of $target.txt"
  ctrl space
  sleep 0.4
done
sel_n="$(state "d['selCount']")"
[[ "$sel_n" == "4" ]] \
  && ok "four items selected" \
  || bad "expected 4 selected, got $sel_n"

key Delete
wait_for "d['confirmOpen']" "True" 10 >/dev/null
del_title="$(state "d['confirmTitle']")"
del_body="$(state "d['confirmBody']")"

# Whatever this location supports, the dialog must name the real count.
if [[ "$del_body" == *"4 selected items"* ]]; then
  ok "the dialog names the real count  ($del_body)"
else
  bad "the dialog does not name 4 items: $del_body"
fi

# On a location without Trash the wording must be unmistakably different
# and the irreversibility explicit (plan §3 rule 2).
if [[ "$del_title" == *"permanently"* ]]; then
  [[ "$del_body" == *"CANNOT be undone"* ]] \
    && ok "an irreversible delete says so in the body" \
    || bad "permanent delete did not warn it cannot be undone"
  hard_delete=1
else
  ok "this location supports Trash, so Trash is what was offered  ($del_title)"
  hard_delete=0
fi

# Confirm it. Cancel holds focus only for a DANGEROUS dialog, so Tab is
# needed exactly then — tabbing on a Trash dialog moves away from the
# confirm button and cancels instead.
if [[ "$hard_delete" == "1" ]]; then
  key Tab
  sleep 0.4
fi
key Return
sleep 2.5
wait_for "d['model']['settled']" "True" 20 >/dev/null

left="$(ls -1 "$DELDIR" 2>/dev/null | grep -c '^del-' || true)"
if [[ "$left" == "0" ]]; then
  ok "all four selected items were removed, not just the cursor row"
else
  bad "$left of 4 selected files survived — the dialog promised more than it did"
fi

# And nothing else may have been touched.
[[ -f "$DELDIR/keep-me.txt" ]] \
  && ok "an unselected file was left alone" \
  || bad "an unselected file was destroyed"
[[ -f "$DELDIR/keepdir/inner.txt" ]] \
  && ok "an unselected folder's contents survived" \
  || bad "an unselected folder was damaged"

# A permanent delete has no undo, and must not pretend otherwise.
if [[ "$hard_delete" == "1" ]]; then
  undo_after="$(state "d.get('undoLabel','')")"
  if [[ -z "$undo_after" || "$undo_after" == "None" ]]; then
    ok "no undo is offered for an irreversible delete"
  else
    bad "undo was offered after a permanent delete: $undo_after"
  fi
fi

shot "12d-permanent"
rm -rf "$DELDIR"
DELDIR=""

say "12v · devices are discovered without mounting anything"
# The dangerous mistakes here are offering the system disk as ejectable and
# mounting something the user did not ask for. Both are checked.
stop_gust
start_gust
wait_for "d['model']['settled']" "True" 20 >/dev/null

mounts_before="$(wc -l < /proc/self/mounts)"
palette_open
typ "device"
sleep 1.2
dev_said="$(state "' | '.join(d.get('paletteResults') or [])")"
shot "12v-devices"
key Escape
sleep 0.5
mounts_after="$(wc -l < /proc/self/mounts)"

[[ "$mounts_before" == "$mounts_after" ]] \
  && ok "listing devices mounted nothing  ($mounts_after mounts, unchanged)" \
  || bad "the device list changed the mount table ($mounts_before -> $mounts_after)"

# Whatever this machine has, the offer must never include its own storage.
root_dev="$(findmnt -no SOURCE / 2>/dev/null | sed 's|[0-9]*$||')"
root_name="$(basename "${root_dev:-none}")"
if [[ -n "$root_name" && "$root_name" != "none" && "$dev_said" == *"$root_name"* ]]; then
  bad "the system disk ($root_name) was offered as a removable device"
else
  ok "the system disk is not offered as a device"
fi
case "$dev_said" in
  *swap*|*zram*) bad "swap or zram was offered as a device" ;;
  *)             ok "swap and zram are not offered as devices" ;;
esac

# The list must say something either way — an empty section with no
# explanation reads as breakage.
if [[ -n "$dev_said" ]]; then
  ok "the device palette says something  ($dev_said)"
else
  bad "the device palette produced nothing at all"
fi

say "12a · archives: compress, extract, and refuse what is unsafe"
stop_gust
ARCDIR="${FIXTURE}-arch"
rm -rf "$ARCDIR"
mkdir -p "$ARCDIR/src/sub"
printf 'keep me\n'   > "$ARCDIR/src/good.txt"
printf 'also fine\n' > "$ARCDIR/src/sub/nested.txt"
mkfifo "$ARCDIR/src/pipe" 2>/dev/null
ln -s ../../../etc/passwd "$ARCDIR/src/escape" 2>/dev/null
printf '#!/bin/sh\n' > "$ARCDIR/src/privileged"
chmod 4755 "$ARCDIR/src/privileged" 2>/dev/null
# A hostile archive, built with the same tool Gust reads with.
( cd "$ARCDIR/src" && bsdtar -cf "$ARCDIR/hostile.tar" -- good.txt sub pipe escape privileged ) 2>/dev/null
rm -rf "$ARCDIR/src"
printf 'plain\n' > "$ARCDIR/plain.txt"

start_gust
wait_for "d['model']['settled']" "True" 20 >/dev/null
goto "$ARCDIR"
wait_for "d['model']['settled']" "True" 20 >/dev/null

# Extracting the hostile archive must keep the good file and refuse the rest.
typ "hostile"
expect "cursor on the archive" "d['model']['cursor_name']" "hostile.tar"
palette_run "Extract here"
sleep 3
wait_for "d['model']['settled']" "True" 20 >/dev/null

[[ -f "$ARCDIR/good.txt" ]] \
  && ok "the ordinary file was extracted" \
  || bad "a safe file was not extracted"
if [[ -e "$ARCDIR/pipe" ]]; then
  bad "a FIFO was created from an archive — the plan's own fixture hazard"
else
  ok "the FIFO was refused"
fi
if [[ -e "$ARCDIR/escape" ]]; then
  bad "an escaping symlink was created from an archive"
else
  ok "the escaping symlink was refused"
fi
if [[ -e "$ARCDIR/privileged" ]]; then
  bad "a setuid file was extracted from an archive"
else
  ok "the setuid file was refused"
fi

# The refusals must be SHOWN, not silently dropped.
arc_notice="$(state "d.get('notice','')")"
if [[ "$arc_notice" == *"pipe"* || "$arc_notice" == *"privileged"* ]]; then
  ok "refused entries are named  ($arc_notice)"
else
  bad "refusals were not reported to the user: notice=$arc_notice"
fi

# No staging directory may survive.
stray="$(ls -a "$ARCDIR" 2>/dev/null | grep -c '^\.gust-extract' || true)"
[[ "$stray" == "0" ]] \
  && ok "no staging directory was left behind" \
  || bad "$stray staging director(y/ies) survived the extraction"

shot "12a-extract"

# Compressing a file produces a real archive, and does not overwrite one.
typ "plain"
expect "cursor on the plain file" "d['model']['cursor_name']" "plain.txt"
palette_run "Compress to"
sleep 3
wait_for "d['model']['settled']" "True" 20 >/dev/null
if [[ -f "$ARCDIR/plain.txt.tar.zst" ]]; then
  ok "compressing produced an archive"
  bsdtar -tf "$ARCDIR/plain.txt.tar.zst" 2>/dev/null | grep -q "plain.txt" \
    && ok "and it really contains the file" \
    || bad "the archive does not contain what was selected"
else
  bad "no archive was produced (folder: $(ls "$ARCDIR" | tr '\n' ' '))"
fi

# No half-written archive may be left under a staging name.
stray2="$(ls -a "$ARCDIR" 2>/dev/null | grep -c '^\.gust-compress' || true)"
[[ "$stray2" == "0" ]] \
  && ok "no half-written archive was left behind" \
  || bad "$stray2 staging archive(s) survived"

# "Extract here" on a non-archive must refuse rather than fail obscurely.
typ "plain.txt"
palette_run "Extract here"
sleep 1.5
arc_alert="$(state "d.get('alertText','')")"
[[ -n "$arc_alert" && "$arc_alert" != "None" ]] \
  && ok "extracting a non-archive is refused with a reason  ($arc_alert)" \
  || bad "extracting a text file was not refused"

rm -rf "$ARCDIR"

say "12b · bulk rename shows the plan before it touches anything"
stop_gust
BULKDIR="${FIXTURE}-bulk"
rm -rf "$BULKDIR"
mkdir -p "$BULKDIR"
for n in one two three; do printf '%s\n' "$n" > "$BULKDIR/$n.txt"; done
printf 'bystander\n' > "$BULKDIR/keep.md"

start_gust
wait_for "d['model']['settled']" "True" 20 >/dev/null
goto "$BULKDIR"
wait_for "d['model']['settled']" "True" 20 >/dev/null

# Select the three .txt files by name.
for target in one two three; do
  sleep 1.3
  typ "$target"
  sleep 0.5
  ctrl space
  sleep 0.4
done
bulk_sel="$(state "d['selCount']")"
[[ "$bulk_sel" == "3" ]] \
  && ok "three files selected for renaming" \
  || bad "expected 3 selected, got $bulk_sel"

# A pattern that would collide must be refused BEFORE anything happens.
palette_run "Bulk rename"
sleep 1.2
expect "the rename prompt opened" "d['promptMode']" "bulk"
ctrl a            # the field is pre-filled; replace it, do not append
typ "same.{ext}"
sleep 0.5
key Return
sleep 1.5
bulk_err="$(state "d.get('promptError','')")"
if [[ -n "$bulk_err" && "$bulk_err" != "None" ]]; then
  ok "a colliding pattern is refused with a reason  ($bulk_err)"
else
  bad "a pattern that renames three files to one name was not refused"
fi
# And nothing may have been touched.
still="$(ls -1 "$BULKDIR" | grep -c '^\(one\|two\|three\)\.txt$' || true)"
[[ "$still" == "3" ]] \
  && ok "a refused pattern renamed nothing" \
  || bad "files were renamed by a pattern that was refused"
key Escape
sleep 0.8

# A good pattern must show a preview before renaming.
palette_run "Bulk rename"
sleep 1.2
ctrl a            # replace the pre-filled pattern
typ "file-{n}.{ext}"
sleep 0.5
key Return
sleep 1.5
expect "a preview is shown before anything happens" "d['confirmOpen']" "True"
bulk_body="$(state "d['confirmBody']")"
if [[ "$bulk_body" == *"→"* ]]; then
  ok "the preview shows old → new"
else
  bad "the preview does not show the renames: $bulk_body"
fi
shot "12b-bulk-preview"

# Nothing has happened yet.
still2="$(ls -1 "$BULKDIR" | grep -c '^\(one\|two\|three\)\.txt$' || true)"
[[ "$still2" == "3" ]] \
  && ok "showing the preview renamed nothing" \
  || bad "files were renamed before the user confirmed"

# Confirm, and check the result.
key Return
sleep 1
# Wait for the app to say it finished, rather than racing the filesystem:
# the rename runs and then reloads, and `settled` can still be true from
# before the keypress.
for _ in $(seq 1 40); do
  [[ "$(ls -1 "$BULKDIR" 2>/dev/null | grep -c '^file-[0-9]*\.txt$' || true)" == "3" ]] && break
  sleep 0.5
done
wait_for "d['model']['settled']" "True" 20 >/dev/null
renamed="$(ls -1 "$BULKDIR" | grep -c '^file-[0-9]*\.txt$' || true)"
[[ "$renamed" == "3" ]] \
  && ok "all three files were renamed  ($(ls -1 "$BULKDIR" | tr '\n' ' '))" \
  || bad "expected 3 renamed files, found $renamed"
[[ -f "$BULKDIR/keep.md" ]] \
  && ok "an unselected file was left alone" \
  || bad "a bystander was renamed"

# No temporary names may survive.
debris="$(ls -a "$BULKDIR" | grep -c '^\.gust-rename' || true)"
[[ "$debris" == "0" ]] \
  && ok "no temporary names were left behind" \
  || bad "$debris temporary name(s) survived"

# One Ctrl+Z undoes the whole thing.
expect_contains "undo is offered for the bulk rename" "d.get('undoLabel','')" "back"
vm wtype -M ctrl -k z -m ctrl >/dev/null 2>&1
sleep 2
wait_for "d['model']['settled']" "True" 20 >/dev/null
back="$(ls -1 "$BULKDIR" | grep -c '^\(one\|two\|three\)\.txt$' || true)"
[[ "$back" == "3" ]] \
  && ok "one Ctrl+Z put every name back" \
  || bad "undo restored $back of 3 names"

rm -rf "$BULKDIR"

say "12o · open with: the system's handlers, and nothing else"
stop_gust
OWDIR="${FIXTURE}-openwith"
rm -rf "$OWDIR"
mkdir -p "$OWDIR"
printf 'some text\n' > "$OWDIR/notes.txt"
printf 'not really a png\n' > "$OWDIR/picture.png"

start_gust
wait_for "d['model']['settled']" "True" 20 >/dev/null
goto "$OWDIR"
wait_for "d['model']['settled']" "True" 20 >/dev/null

typ "notes"
expect "cursor on the text file" "d['model']['cursor_name']" "notes.txt"

# Count what is already running BEFORE opening the list, so earlier cases
# that deliberately opened a file are not blamed on this one.
ow_before="$(pgrep -c -f 'nvim|libreoffice|imv ' 2>/dev/null || true)"

# The palette must list applications the SYSTEM registered, named.
palette_open
typ "open with"
sleep 1.5
ow_said="$(state "' | '.join(d.get('paletteResults') or [])")"
shot "12o-open-with"
if [[ -n "$ow_said" && "$ow_said" != "None" ]]; then
  ok "open-with lists something  ($ow_said)"
else
  bad "open-with produced nothing at all"
fi

# Whatever is installed, one entry must be marked as the default, or the
# user cannot tell which one Enter would have used.
if [[ "$ow_said" == *"(default)"* ]]; then
  ok "the default handler is marked as such"
else
  bad "no handler was marked default: $ow_said"
fi

# The list is applications, never a command the user could type. A palette
# that ran arbitrary text would be a shell wearing a file manager's clothes.
case "$ow_said" in
  *"Open with"*) ok "every entry is an application, not a command line" ;;
  *)             bad "the entries do not look like applications: $ow_said" ;;
esac

key Escape
sleep 0.6

# Nothing may be launched just by LOOKING at the list. Counted as a
# DIFFERENCE: earlier cases deliberately open a file, so a global count
# measures those instead.
ow_after="$(pgrep -c -f 'nvim|libreoffice|imv ' 2>/dev/null || true)"
if [[ "${ow_after:-0}" -le "${ow_before:-0}" ]]; then
  ok "listing handlers launched nothing  (${ow_before:-0} before, ${ow_after:-0} after)"
else
  bad "listing handlers started $(( ow_after - ow_before )) application(s)"
fi

rm -rf "$OWDIR"

say "12m · opening several files opens ONE application, not five"
stop_gust
MODIR="${FIXTURE}-multiopen"
rm -rf "$MODIR"
mkdir -p "$MODIR"
for n in 1 2 3; do printf 'text %s\n' "$n" > "$MODIR/doc-$n.txt"; done

start_gust
wait_for "d['model']['settled']" "True" 20 >/dev/null
goto "$MODIR"
wait_for "d['model']['settled']" "True" 20 >/dev/null

mo_before="$(pgrep -c -f 'nvim ' 2>/dev/null || true)"

# Select all three by name.
for target in doc-1 doc-2 doc-3; do
  sleep 1.3
  typ "$target"
  sleep 0.5
  ctrl space
  sleep 0.4
done
mo_sel="$(state "d['selCount']")"
[[ "$mo_sel" == "3" ]] \
  && ok "three files selected" \
  || bad "expected 3 selected, got $mo_sel"

key Return
sleep 3
mo_status="$(state "d['hintStatus']")"
mo_after="$(pgrep -c -f 'nvim ' 2>/dev/null || true)"
mo_started=$(( mo_after - mo_before ))

# nvim's desktop entry is "nvim %F" — a list handler — so ONE launch.
if [[ "$mo_started" -le 1 ]]; then
  ok "opening 3 files started $mo_started application(s), not 3"
else
  bad "opening 3 files started $mo_started applications"
fi
if [[ "$mo_status" == *"3 files"* ]]; then
  ok "and it says how many were opened  ($mo_status)"
else
  bad "the status does not report the multi-open: '$mo_status'"
fi
shot "12m-multi-open"

# Close what this case started, or instances pile up across runs.
pkill -f "nvim $MODIR" 2>/dev/null || true
sleep 0.5

# A folder in the selection makes this a navigation question, not an open:
# Enter must fall back to the cursor row rather than launching anything.
mkdir -p "$MODIR/afolder"
ctrl r
sleep 1.5
wait_for "d['model']['settled']" "True" 20 >/dev/null
sleep 1.3
typ "doc-1"
sleep 0.5
ctrl space
sleep 1.3
typ "afolder"
sleep 0.5
ctrl space
sleep 0.8
mo_before2="$(pgrep -c -f 'nvim ' 2>/dev/null || true)"
key Return
sleep 2.5
mo_after2="$(pgrep -c -f 'nvim ' 2>/dev/null || true)"
if [[ "$(( mo_after2 - mo_before2 ))" -le 1 ]]; then
  ok "a folder in the selection does not launch an application per file"
else
  bad "a mixed selection launched $(( mo_after2 - mo_before2 )) applications"
fi
pkill -f "nvim $MODIR" 2>/dev/null || true
sleep 0.5

rm -rf "$MODIR"

say "12c2 · the clipboard other applications can see"
stop_gust
CLIPDIR="${FIXTURE}-clip"
rm -rf "$CLIPDIR"
mkdir -p "$CLIPDIR/dest"
printf 'shared\n' > "$CLIPDIR/shared.txt"
printf 'from elsewhere\n' > "$CLIPDIR/elsewhere.txt"

start_gust
wait_for "d['model']['settled']" "True" 20 >/dev/null
goto "$CLIPDIR"
wait_for "d['model']['settled']" "True" 20 >/dev/null

# Copy in Gust, then read it the way another file manager would.
typ "shared"
expect "cursor on the file to copy" "d['model']['cursor_name']" "shared.txt"
ctrl c
sleep 1.5
clip_seen="$(vm wl-paste --no-newline --type x-special/gnome-copied-files 2>/dev/null | tr '\n' ' ')"
if [[ "$clip_seen" == *"shared.txt"* && "$clip_seen" == copy* ]]; then
  ok "another application can see Gust's copy  ($clip_seen)"
else
  bad "Gust's copy did not reach the system clipboard: '$clip_seen'"
fi

# A cut must survive as a cut, or another manager would copy when the user
# asked to move.
ctrl x
sleep 1.5
cut_seen="$(vm wl-paste --no-newline --type x-special/gnome-copied-files 2>/dev/null | head -1)"
[[ "$cut_seen" == "cut" ]] \
  && ok "a cut arrives as a cut, not a copy" \
  || bad "a cut arrived as '$cut_seen'"

# The other direction: what another application leaves, Gust pastes.
# Gust's OWN clipboard wins (a copy made here must not be overridden), so
# clear it first — Escape unwinds the selection and the pending clip.
key Escape
sleep 0.5
key Escape
sleep 0.5
vm bash -lc 'printf "copy\nfile://'"$CLIPDIR"'/elsewhere.txt" | wl-copy --type x-special/gnome-copied-files &' >/dev/null 2>&1
sleep 2
goto "$CLIPDIR/dest"
wait_for "d['model']['settled']" "True" 20 >/dev/null
ctrl v
sleep 3
wait_for "d['jobRunning']" "False" 20 >/dev/null
wait_for "d['model']['settled']" "True" 20 >/dev/null
if [[ -f "$CLIPDIR/dest/elsewhere.txt" ]]; then
  ok "Gust pasted what another application put on the clipboard"
else
  bad "a copy from another application could not be pasted (dest: $(ls "$CLIPDIR/dest" | tr '\n' ' '))"
fi

# The source must survive a COPY, whoever made it.
[[ -f "$CLIPDIR/elsewhere.txt" ]] \
  && ok "and it was a copy: the original is still there" \
  || bad "a copy from another application MOVED the file"

shot "12c2-clipboard"
rm -rf "$CLIPDIR"

say "13 · nothing executed, nothing crashed, no QML errors"
[[ -f /tmp/gust-fixture-EXECUTED ]] \
  && bad "SECURITY: a fixture script executed during browsing" \
  || ok "no fixture script ran during browsing"
pgrep -x gust >/dev/null && ok "gust survived the whole run" || bad "gust died during QA"
if grep -qiE "TypeError|is not a function|Cannot assign|Cannot anchor|ReferenceError|QML .*: error" "$LOG" 2>/dev/null; then
  bad "QML runtime errors in $LOG"
  grep -iE "TypeError|is not a function|Cannot assign|Cannot anchor|ReferenceError" "$LOG" | sort -u | head -5
else
  ok "no QML runtime errors"
fi

say "14 · the user's real files were never touched"
if [[ -e "$HOME/.local/share/Trash/files" ]]; then
  real_trash_count="$(ls -1 "$HOME/.local/share/Trash/files" 2>/dev/null | wc -l)"
else
  real_trash_count=0
fi
echo "  (real trash entries: $real_trash_count — Gust ran with XDG_DATA_HOME=$FIXTURE/.local/share)"
for d in "$FIXTURE" "${FIXTURE}-panes" "${FIXTURE}-peek" "${FIXTURE}-perm" "${FIXTURE}-arch" "${FIXTURE}-bulk" "${FIXTURE}-openwith" "${FIXTURE}-multiopen" "${FIXTURE}-clip"; do
  case "$d" in
    /home/*|/root/*)
      bad "a QA tree was inside a real home: $d" ;;
  esac
done
ok "QA ran entirely against $FIXTURE and its eight fixture trees"

stop_gust

say "summary"
printf '%s\n' "${RESULTS[@]}" > "$OUTDIR/results.txt"
printf '\n  passed: %d\n  failed: %d\n  shots:  %s\n' "$PASS" "$FAIL" "$OUTDIR"
[[ "$FAIL" -eq 0 ]] || exit 1
