# shellcheck shell=bash
# A long path and the actual More action list share the smallest supported window.
toolbar_point() {
  local point x y wx wy ww wh
  point="$(python3 - "$STATE" "$1" "$2" <<'PY'
import json, sys
with open(sys.argv[1]) as f:
    print(*json.load(f)[sys.argv[2]][sys.argv[3]])
PY
)" || return 1
  read -r x y <<<"$point"
  read -r wx wy ww wh <<<"$(win_geo)"
  if (( x < 0 || x >= ww || y < 0 || y >= wh )); then
    echo "$2 is outside the window at $x $y" >&2
    return 1
  fi
  echo "$((wx + x)) $((wy + y))"
}

toolbar_click() {
  local point x y
  point="$(toolbar_point "$@")" || return 1
  read -r x y <<<"$point"
  mouse click "$x" "$y"
}

minimum_toolbar() {
  local test_home
  test_home="$(mktemp -d "$FIXTURE/toolbar-XXXXXXXX")" || return 1
  # Keep font metrics and preferences deterministic across desktop themes.
  local FIXTURE="$test_home" REAL_STATE="$test_home/theme"
  local folder="$FIXTURE/long-directory-name-to-force-breadcrumb-overflow/p"
  local zoom last x y
  mkdir -p "$folder/q" "$REAL_STATE"
  for zoom in 1 1.1; do
    start_gust "$folder/q" || return 1
    vm hyprctl dispatch 'hl.dsp.window.float()' >/dev/null
    vm hyprctl dispatch 'hl.dsp.window.resize({ x = 520, y = 320 })' >/dev/null
    if [[ "$zoom" != 1 ]]; then ctrl equal; fi
    # Opening Settings also refreshes the QA geometry after the resize.
    ctrl comma
    expect "Settings opens at zoom $zoom" "d['settingsOpen']" "True"
    expect "minimum client size" "(d['windowWidth'], d['windowHeight'])" "(520, 320)"
    expect "text zoom" "d['zoom']" "$zoom"
    shot "50-settings-$zoom"
    toolbar_click settingsPoints close || return 1
    expect "Close dismisses Settings" "d['settingsOpen']" "False"
    shot "50-breadcrumbs-$zoom"
    expect "the path hitbox ends before Search" \
      "0 < d['toolbarPoints']['pathStart'][0] < d['toolbarPoints']['pathEnd'][0] <= d['toolbarPoints']['searchStart'][0]" "True"
    if [[ "$zoom" == 1 ]]; then
      toolbar_click crumbPoints "$folder" || return 1
      wait_for "d['model']['location']" "$folder" || { bad "visible parent breadcrumb missed"; return 1; }
      ok "the visible parent breadcrumb navigates"
    else
      # The left edge is the ellipsis. It covers hidden crumbs at this zoom.
      toolbar_click toolbarPoints pathStart || return 1
      expect "the cut mark leaves the folder open" "d['model']['location']" "$folder/q"
    fi
    toolbar_click toolbarPoints search || return 1
    expect "the adjacent Search button takes its click" "d['searchBarActive']" "True"
    key Escape
    toolbar_click toolbarPoints more || return 1
    expect "More opens at zoom $zoom" "d['menuOpen']" "True"
    key End
    expect "End reveals the last More action" "d['menuCurrent']" "Keyboard shortcuts"
    shot "50-more-$zoom"
    # Reopen at the top. Reach the same bottom action using only the mouse.
    last="$(toolbar_point menuPoints "Keyboard shortcuts")" || return 1
    read -r x y <<<"$last"
    key Escape
    toolbar_click toolbarPoints more || return 1
    mouse wheel "$x" "$y" -12 || return 1
    mouse click "$x" "$y" || return 1
    expect "the last More action takes a pointer click" "d['keyMapOpen']" "True"
  done
}

say "50 · minimum-size toolbar and panels at default and increased text"
if [[ "$MOUSE_OK" == 1 ]]; then
  minimum_toolbar || bad "minimum-size toolbar journey did not finish"
else
  bad "minimum-size toolbar journey needs pointer injection"
fi
stop_gust; start_gust "$FIXTURE"
