#!/usr/bin/env bash
# Shared setup and helpers for the desktop QA suite (tests/qa.sh) and for
# running single sections (tests/qa-one.sh). Sourced, never executed.
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

say "fixture"
# A completed or interrupted run may still contain evidence or mounts.
# Honour explicit roots only when the fixture builder admits them.
source "$HERE/qa-fixture.sh"
prepare_fixture || exit 1
mkdir -p "$FIXTURE/.local/share" "$FIXTURE/.cache" || exit 1
rm -f /tmp/gust-fixture-EXECUTED
[[ -d "$FIXTURE/Projects" ]] && ok "fixture built at $FIXTURE" || bad "fixture missing"

vm()   { "$HERE/vmsession" "$@" 9>&-; }

# Pointer injection through a temporary virtual mouse (tests/uinput-mouse.py).
# Needs write access to /dev/uinput, so it goes through sudo -n; when that is
# unavailable the mouse cases are skipped rather than silently passing.
MOUSE_OK=0
if sudo -n python3 "$HERE/uinput-mouse.py" move 640 400 >/dev/null 2>&1; then
  MOUSE_OK=1
fi
mouse() {
  local status=0
  sudo -n python3 "$HERE/uinput-mouse.py" "$@" >/dev/null || status=$?
  if [[ "$status" -ne 0 ]]; then
    bad "pointer injection failed ($status): $*"
    return "$status"
  fi
  sleep 0.6
}

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
    d=json.load(open(sys.argv[1]))
except Exception as e:
    print('NO_STATE'); sys.exit(0)
try:
    print($1)
except Exception as e:
    print('ERR:'+str(e))
" "$STATE" "${@:2}"
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

# Extra env(1) arguments for a section's device settings or supervised launch.
qa_launch_env=()

# Pass paths as individual arguments through vmsession, including the isolated
# HOME and every XDG base. A fixture name can contain quotes and newlines.
launch_gust() {
  local snapshot="$1"
  shift
  vm env HOME="$FIXTURE" XDG_DATA_HOME="$FIXTURE/.local/share" \
    XDG_CACHE_HOME="$FIXTURE/.cache" XDG_CONFIG_HOME="$FIXTURE/.config" \
    XDG_STATE_HOME="$FIXTURE/.local/state" GUST_OMARCHY_STATE="$REAL_STATE" \
    GUST_QA_STATE="$snapshot" PATH="${QA_PATH_PREPEND:+$QA_PATH_PREPEND:}$PATH" \
    "${qa_launch_env[@]}" "$BIN" "$@"
}

start_gust() {
  # Cleanup becomes necessary only when the runner starts managing Gust.
  trap 'pkill -x gust 2>/dev/null || true' EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  pkill -x gust 2>/dev/null || true
  sleep 0.4
  rm -f "$STATE"
  launch_gust "$STATE" "$@" > "$LOG" 2>&1 9>&- &
  qa_gust_job=$!
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

# Match LocationKey::display_path (crates/core/src/location.rs). Only the
# projection escapes controls and bidi marks. File operations keep the bytes.
display_path() {
  python3 - "$1" <<'PYTHON'
import os, sys
path = os.fsencode(sys.argv[1]).decode("utf-8", "replace")
for c in path:
    n = ord(c)
    if (n < 0x20 or 0x7f <= n <= 0x9f or n in (0x061c, 0x200e, 0x200f)
            or 0x202a <= n <= 0x202e or 0x2066 <= n <= 0x2069):
        sys.stdout.write(f"<U+{n:04X}>")
    else:
        sys.stdout.write(c)
PYTHON
}

expect_location() { expect "$1" "d['model']['location']" "$(display_path "$2")"; }
wait_location() { wait_for "d['model']['location']" "$(display_path "$1")" "${2:-20}"; }

# Ctrl+L accepts ~ and ~/child even when HOME itself cannot be typed into a
# single-line field. Sibling paths sharing a prefix must stay absolute.
goto() {
  local path="$1" input="$1"
  case "$path" in
    "$FIXTURE")   input='~' ;;
    "$FIXTURE"/*) input="~/${path#"$FIXTURE"/}" ;;
  esac
  for _ in 1 2 3; do
    ctrl l
    wait_for "d['paletteOpen']" "True" 8 || continue
    typ "$input"
    key Return
    if wait_location "$path" "${2:-10}"; then
      return 0
    fi
  done
  bad "could not navigate to $(display_path "$path") (got $(state "d['model']['location']"))" >&2
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
  # "Update System" and "Learn Keybindings" sit over the toolbar's right end
  # (view and sort buttons), where they took a click meant for Gust.
  for card in "Set your default agent" "Install Dictation with Voxtype" \
              "Update available" "Omarchy" "Update System" "Learn Keybindings"; do
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
