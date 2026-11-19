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

toolbar_menu_click() {
  toolbar_click toolbarPoints more && toolbar_click menuPoints "$1"
}

compact_toolbar_actions() {
  local folder="$1" view
  toolbar_click toolbarPoints up || return 1
  expect "Up takes its click" "d['model']['location']" "$folder"
  toolbar_click toolbarPoints back || return 1
  expect "Back takes its click" "d['model']['location']" "$folder/q"
  toolbar_click toolbarPoints forward || return 1
  expect "Forward takes its click" "d['model']['location']" "$folder"

  toolbar_click toolbarPoints sidebar || return 1
  toolbar_click toolbarPoints settings || return 1
  expect "the sidebar toggle hides the sidebar" "d['sidebarVisible']" "False"
  expect "Settings takes its click beside More" "d['settingsOpen']" "True"
  toolbar_click settingsPoints close || return 1

  toolbar_menu_click "Search" || return 1
  expect "Search from More opens the search bar" "d['searchBarActive']" "True"
  typ "q"
  expect "typing immediately starts the search" "d['findNeedle']" "q"
  toolbar_menu_click "Close search" || return 1
  expect "Close search returns to the folder" "d['searchBarActive']" "False"
  toolbar_menu_click "New folder" || return 1
  expect "New folder from More opens its prompt" "(d['promptOpen'], d['promptMode'])" "(True, 'newfolder')"
  key Escape
  toolbar_menu_click "Sort by Name" || return 1
  toolbar_click menuPoints "Size" || return 1
  expect "the Sort submenu applies the chosen field" "d['model']['sort_field']" "size"
  for view in Grid Columns Rows; do
    toolbar_menu_click "$view view" || return 1
    expect "$view from More switches the view" "d['view']" "${view,,}"
  done
}

minimum_toolbar() {
  local test_home
  test_home="$(mktemp -d "$FIXTURE/toolbar-XXXXXXXX")" || return 1
  # Keep font metrics and preferences deterministic across desktop themes.
  local FIXTURE="$test_home" REAL_STATE="$test_home/theme"
  local folder="$FIXTURE/long-directory-name-to-force-breadcrumb-overflow/p"
  local zoom last x y step
  mkdir -p "$folder/q" "$REAL_STATE"
  for zoom in 1 1.1 1.6; do
    start_gust "$folder/q" || return 1
    vm hyprctl dispatch 'hl.dsp.window.float()' >/dev/null
    vm hyprctl dispatch 'hl.dsp.window.resize({ x = 520, y = 320 })' >/dev/null
    case "$zoom" in
      1.1) ctrl equal ;;
      1.6) for step in {1..4}; do ctrl equal; done ;;
    esac
    # Opening Settings also refreshes the QA geometry after the resize.
    ctrl comma
    expect "Settings opens at zoom $zoom" "d['settingsOpen']" "True"
    expect "minimum client size" "(d['windowWidth'], d['windowHeight'])" "(520, 320)"
    expect "text zoom" "d['zoom']" "$zoom"
    shot "50-settings-$zoom"
    toolbar_click settingsPoints close || return 1
    expect "Close dismisses Settings" "d['settingsOpen']" "False"
    shot "50-breadcrumbs-$zoom"
    if [[ "$zoom" == 1 ]]; then
      toolbar_click crumbPoints "$folder" || return 1
      wait_for "d['model']['location']" "$folder" || { bad "visible parent breadcrumb missed"; return 1; }
      ok "the visible parent breadcrumb navigates"
    else
      # The left edge is the ellipsis. It covers hidden crumbs at this zoom.
      toolbar_click toolbarPoints pathStart || return 1
      expect "the cut mark leaves the folder open" "d['model']['location']" "$folder/q"
    fi
    if [[ "$zoom" == 1.6 ]]; then
      compact_toolbar_actions "$folder" || return 1
    else
      expect "the path hitbox ends before Search" \
        "0 < d['toolbarPoints']['pathStart'][0] < d['toolbarPoints']['pathEnd'][0] <= d['toolbarPoints']['searchStart'][0]" "True"
      toolbar_click toolbarPoints search || return 1
      expect "the adjacent Search button takes its click" "d['searchBarActive']" "True"
      key Escape
      toolbar_click toolbarPoints newFolder || return 1
      expect "the adjacent New folder button takes its click" "d['promptOpen']" "True"
      key Escape
    fi
    toolbar_click toolbarPoints more || return 1
    expect "More opens at zoom $zoom" "d['menuOpen']" "True"
    if [[ "$zoom" == 1.6 ]]; then shot "50-overflow-$zoom"; fi
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
    if [[ "$zoom" == 1.6 ]]; then
      # Resize the same window: hidden actions must become clickable inline again.
      key Escape
      vm hyprctl dispatch 'hl.dsp.window.resize({ x = 900, y = 320 })' >/dev/null
      ctrl comma
      toolbar_click settingsPoints close || return 1
      expect "wider client" "d['windowWidth']" "900"
      shot "50-wide-$zoom"
      toolbar_click toolbarPoints search || return 1
      expect "widening restores the inline Search button" "d['searchBarActive']" "True"
      key Escape
      toolbar_click toolbarPoints more || return 1
      expect "wide More omits the inline Search action" "d['menuOpen'] and 'Search' not in d['menuItems']" "True"
    fi
  done
}

say "50 · minimum-size toolbar and panels at default and increased text"
if [[ "$MOUSE_OK" == 1 ]]; then
  minimum_toolbar || bad "minimum-size toolbar journey did not finish"
else
  bad "minimum-size toolbar journey needs pointer injection"
fi
stop_gust; start_gust "$FIXTURE"
