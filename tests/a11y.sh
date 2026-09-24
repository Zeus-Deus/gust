#!/usr/bin/env bash
# Accessibility and scaling qualification (plan §7).
#
# Drives the real window at several font sizes and spacing scales, and reads
# the accessibility tree that a screen reader would read. Screenshot review
# alone cannot tell you a control has a name; this asserts it.
set -uo pipefail
cd "$(dirname "$0")/.."
HERE="$(pwd)/tests"

OUT="${1:-/tmp/gust-a11y}"
BIN="${GUST_BIN:-target/release/gust}"
FIX="${GUST_A11Y_FIXTURE:-/tmp/gust-a11y-fixture}"
STATE="$OUT/state.json"
REAL_STATE="${GUST_OMARCHY_STATE:-$HOME/.local/state/omarchy}"
mkdir -p "$OUT"

PASS=0; FAIL=0; RESULTS=()
ok()  { PASS=$((PASS+1)); RESULTS+=("PASS $1"); echo "  PASS $1"; }
bad() { FAIL=$((FAIL+1)); RESULTS+=("FAIL $1"); echo "  FAIL $1"; }
say() { echo; echo "== $1"; RESULTS+=("" "== $1"); }
vm()  { "$HERE/vmsession" "$@"; }

# A fixture with names long enough to reveal clipping at a large font.
rm -rf "$FIX"
mkdir -p "$FIX/.config" "$FIX/.cache" "$FIX/.local/share" "$FIX/files"
printf 'x\n' > "$FIX/files/a-short-name.txt"
printf 'x\n' > "$FIX/files/a-considerably-longer-file-name-for-testing.txt"
mkdir -p "$FIX/files/a-folder"

# Each run gets its own theme state, so a font size is a property of the
# run rather than something left behind on the machine.
run_at() {
  local font="$1" scale="$2" tag="$3"
  local themedir="$OUT/theme-$tag"
  mkdir -p "$themedir/current/theme"
  # Copy the live palette, then override only the two metrics under test.
  cp "$REAL_STATE/current/theme/colors.toml" "$themedir/current/theme/" 2>/dev/null
  sed -e "s/^base-size.*/base-size = $font/" \
      -e "s/^scale.*/scale = $scale/" \
      "$REAL_STATE/current/theme/shell.toml" > "$themedir/current/theme/shell.toml" 2>/dev/null
  # If the keys were absent rather than present, append them.
  grep -q "base-size" "$themedir/current/theme/shell.toml" 2>/dev/null || \
    printf '\n[font]\nbase-size = %s\n' "$font" >> "$themedir/current/theme/shell.toml"
  grep -q "^scale" "$themedir/current/theme/shell.toml" 2>/dev/null || \
    printf '\n[spacing]\nscale = %s\n' "$scale" >> "$themedir/current/theme/shell.toml"

  pkill -x gust 2>/dev/null; sleep 0.4
  rm -f "$STATE"
  cat > "$OUT/run-$tag.sh" <<EOF
#!/usr/bin/env bash
exec > "$OUT/log-$tag.txt" 2>&1
export HOME="$FIX"
export XDG_STATE_HOME="$FIX/.local/state"
export XDG_DATA_HOME="$FIX/.local/share"
export XDG_CACHE_HOME="$FIX/.cache"
export XDG_CONFIG_HOME="$FIX/.config"
export GUST_OMARCHY_STATE="$themedir"
export GUST_QA_STATE="$STATE"
${GUST_A11Y_ON:+export QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1}
exec "$(pwd)/$BIN" "$FIX/files"
EOF
  chmod +x "$OUT/run-$tag.sh"
  vm "$OUT/run-$tag.sh" 9>&- &
  for _ in $(seq 1 40); do [[ -f "$STATE" ]] && break; sleep 0.4; done
  sleep 1.5
}

state() {
  python3 -c "
import json,sys
try:
    d=json.load(open('$STATE'))
except Exception:
    print('NO_STATE'); sys.exit(0)
try:
    print($1)
except Exception as e:
    print('ERR:'+str(e))
"
}

say "fonts and spacing: the window survives being made large"
for combo in "12 1.0 base" "18 1.3 large" "9 0.8 small"; do
  set -- $combo
  font="$1"; scale="$2"; tag="$3"
  run_at "$font" "$scale" "$tag"
  if [[ ! -f "$STATE" ]]; then
    bad "gust did not start at font $font / scale $scale"
    continue
  fi
  ok "starts at font $font, spacing $scale"

  count="$(state "d['model']['count']")"
  [[ "$count" == "3" ]] \
    && ok "  and lists everything at that size  ($count entries)" \
    || bad "  listing wrong at font $font: $count"

  vm grim "$OUT/font-$tag.png" >/dev/null 2>&1

  # The window must still be usable: open the folder, then come back.
  # Directories sort first, so Home lands on a-folder.
  vm wtype -k Home >/dev/null 2>&1; sleep 0.5
  vm wtype -k Return >/dev/null 2>&1; sleep 1.5
  loc="$(state "d['model']['location']")"
  vm wtype -k BackSpace >/dev/null 2>&1; sleep 1.2
  back="$(state "d['model']['location']")"
  if [[ "$loc" == "$FIX/files/a-folder" && "$back" == "$FIX/files" ]]; then
    ok "  navigation still works at that size"
  else
    bad "  navigation broken at font $font (went $loc, came back to $back)"
  fi

  # Nothing may be reported as missing its own tokens.
  if grep -qi "fd01fd\|missing token" "$OUT/log-$tag.txt" 2>/dev/null; then
    bad "  a token was missing at font $font"
  else
    ok "  every value came from a token"
  fi
done

say "the accessibility tree has names, not just shapes"
# Qt only joins the AT-SPI bus when asked. Gust does not set this globally
# (it sets no QT_* variable on the user's session, by design); the test
# enables it for its own run, which is what a screen-reader user's session
# would have set already.
GUST_A11Y_ON=1 run_at 12 1.0 a11y
if [[ ! -f "$STATE" ]]; then
  bad "gust did not start for the accessibility check"
else
  vm python3 - <<'PYEOF' > "$OUT/a11y-tree.txt" 2>&1
import sys
try:
    import gi
    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi
except Exception as e:
    print("ATSPI_UNAVAILABLE:", e); sys.exit(0)

Atspi.init()
desktop = Atspi.get_desktop(0)
app = None
for i in range(desktop.get_child_count()):
    c = desktop.get_child_at_index(i)
    try:
        if (c.get_name() or "").lower() == "gust":
            app = c
    except Exception:
        pass
if app is None:
    print("NO_GUST_IN_TREE"); sys.exit(0)

total = unnamed = 0
roles = {}
lines = []
def walk(n, depth=0):
    global total, unnamed
    if depth > 6:
        return
    try:
        role, name = n.get_role_name(), n.get_name()
    except Exception:
        return
    total += 1
    roles[role] = roles.get(role, 0) + 1
    if not name:
        unnamed += 1
    lines.append("  " * depth + f"{role}: {name!r}")
    for j in range(min(n.get_child_count(), 60)):
        ch = n.get_child_at_index(j)
        if ch is not None:
            walk(ch, depth + 1)
walk(app)
print("\n".join(lines))
print(f"SUMMARY total={total} unnamed={unnamed}")
print("ROLES " + " ".join(f"{k}={v}" for k, v in sorted(roles.items())))
PYEOF

  if grep -q "ATSPI_UNAVAILABLE" "$OUT/a11y-tree.txt"; then
    bad "AT-SPI is not available in this VM — the accessibility tree was NOT verified"
  elif grep -q "NO_GUST_IN_TREE" "$OUT/a11y-tree.txt"; then
    bad "gust did not join the accessibility bus"
  else
    total="$(sed -n 's/.*SUMMARY total=\([0-9]*\).*/\1/p' "$OUT/a11y-tree.txt")"
    unnamed="$(sed -n 's/.*unnamed=\([0-9]*\).*/\1/p' "$OUT/a11y-tree.txt")"
    [[ "${total:-0}" -ge 20 ]] \
      && ok "gust exposes a real accessibility tree  ($total nodes)" \
      || bad "the accessibility tree is too thin ($total nodes)"
    [[ "${unnamed:-99}" -le 5 ]] \
      && ok "almost every node has a name  ($unnamed unnamed of $total)" \
      || bad "$unnamed of $total nodes have no accessible name"

    # A screen reader needs the FILES to be list items, not anonymous shapes,
    # and the controls to say what they do.
    if grep -q "ROLES.*list item=" "$OUT/a11y-tree.txt"; then
      ok "files are exposed as list items"
    else
      bad "the file list is not exposed as list items"
    fi
    for want in "Back" "Up one folder" "Rows view" "Places sidebar"; do
      if grep -q "'$want'" "$OUT/a11y-tree.txt"; then
        ok "  a screen reader can find \"$want\""
      else
        bad "  no accessible node named \"$want\""
      fi
    done
  fi
fi

say "rename puts the keyboard back where it was"
run_at 12 1.0 rename
if [[ -f "$STATE" ]]; then
  vm wtype -k Home >/dev/null 2>&1; sleep 0.5
  before="$(state "d['model']['cursor_name']")"
  vm wtype -k F2 >/dev/null 2>&1; sleep 1.2
  expect_open="$(state "d['promptOpen']")"
  [[ "$expect_open" == "True" ]] \
    && ok "F2 opens the rename prompt" \
    || bad "F2 did not open a rename prompt"

  # Escape must cancel AND give the list back the keyboard, or the next
  # keystroke goes nowhere.
  vm wtype -k Escape >/dev/null 2>&1; sleep 1.0
  vm wtype -k Down >/dev/null 2>&1; sleep 0.8
  after="$(state "d['model']['cursor_name']")"
  if [[ "$after" != "$before" && "$after" != "NO_STATE" ]]; then
    ok "after cancelling a rename the list takes keys again  ($before -> $after)"
  else
    bad "the list did not regain focus after a rename (cursor stuck on $after)"
  fi
fi

say "no orphaned processes are left behind"
# pgrep -c PRINTS 0 and also exits non-zero, so `|| echo 0` yields "0\n0".
count_gust() { pgrep -x gust 2>/dev/null | wc -l; }
before_procs="$(count_gust)"
pkill -x gust 2>/dev/null
sleep 1.5
after_procs="$(count_gust)"
[[ "$after_procs" == "0" ]] \
  && ok "closing gust leaves no process behind (was $before_procs, now $after_procs)" \
  || bad "$after_procs gust process(es) survived"

# Helpers must not outlive the app that spawned them.
stray="$(pgrep -af "gust-thumb|bwrap.*gust" 2>/dev/null | wc -l)"
[[ "$stray" == "0" ]] \
  && ok "no sandboxed helper outlived the window" \
  || bad "$stray helper process(es) still running"

say "summary"
printf '%s\n' "${RESULTS[@]}" > "$OUT/results.txt"
printf '\n  passed: %d\n  failed: %d\n  shots:  %s\n' "$PASS" "$FAIL" "$OUT"
rm -rf "$FIX"
[[ "$FAIL" -eq 0 ]] || exit 1
