# shellcheck shell=bash
# A long path and the actual More action list share the smallest supported window.
toolbar_point() {
  local point x y wx wy ww wh
  point="$(python3 - "$STATE" "$1" "$2" <<'PY'
import json, sys
with open(sys.argv[1]) as f:
    points = json.load(f)[sys.argv[2]]
    print(*points[int(sys.argv[3]) if isinstance(points, list) else sys.argv[3]])
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
  mouse click "$x" "$y" "${3:-left}"
}

toolbar_menu_click() {
  toolbar_click toolbarPoints more && toolbar_click menuPoints "$1"
}

check_more_width() {
  expect "both More popup edges fit inside the client" \
    "0 < d['menuEdges'][0] < d['menuEdges'][1] < d['windowWidth']" "True"
}

# At default zoom More fits in a tall window. Shrink it while it is open
# in each input journey, so opening directly at minimum size cannot mask clipping.
open_more_at_minimum_height() {
  local zoom="$1"
  if [[ "$zoom" == 1 ]]; then
    vm hyprctl dispatch 'hl.dsp.window.resize({ x = 520, y = 700 })' >/dev/null || return 1
  fi
  toolbar_click toolbarPoints more || return 1
  if [[ "$zoom" == 1 ]]; then
    expect "the open More menu initially fits" \
      "d['windowHeight'] == 700 and 320 < d['menuPoints']['Keyboard shortcuts'][1] < 700" "True"
    vm hyprctl dispatch 'hl.dsp.window.resize({ x = 520, y = 320 })' >/dev/null || return 1
    wait_for "d['menuOpen'] and d['windowHeight'] == 320" "True" || return 1
  fi
}

check_search_query() {
  typ "probe"
  expect "typing reaches the focused Search field and starts the query" \
    "(d['focusOwner'], d['searchText'], d['findNeedle'])" "('searchInput', 'probe', 'probe')"
  expect "the complete query fits inside the visible field" \
    "0 <= d['searchFieldEdges'][0] < d['searchFieldEdges'][1] <= d['windowWidth'] and d['searchTextFits']" "True"
}

roomy_settings_options() {
  local preset
  key Down
  key Right
  expect "Roomy density takes effect" "d['settings']['density']" "roomy"
  key Down
  expect "navigating lower settings scrolls the body" "d['settingsScrollY'] > 0" "True"
  key Up
  key Up
  expect "scrolling back reveals all four complete Keys buttons" \
    "sorted(d['settingsOptionPoints'])" "['mac', 'standard', 'vim', 'windows']"
  shot "50-settings-keys-roomy"
  for preset in vim mac windows standard; do
    toolbar_click settingsOptionPoints "$preset" || return 1
    expect "the $preset button selects its Keys preset" "d['keyPreset']" "$preset"
  done
  key Down
  key Left
  expect "normal density is restored" "d['settings']['density']" "normal"
}

compact_toolbar_navigation() {
  local folder="$1"
  expect "Up ends before More begins" \
    "d['toolbarPoints']['upEnd'][0] <= d['toolbarPoints']['moreStart'][0]" "True"
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
}

compact_toolbar_actions() {
  local view
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

hover_tab() {
  local point x y
  point="$(toolbar_point tabCenters "$1")" || return 1
  read -r x y <<<"$point"
  mouse move "$x" "$y"
}

inspect_restored_tabs() {
  local test_home year index path x y point
  test_home="$(mktemp -d "$FIXTURE/tabs-XXXXXXXX")" || return 1
  local FIXTURE="$test_home"
  # Only the first restored tab has listed its folder. Hover must identify
  # the unopened targets while Search still owns the keyboard.
  mkdir -p "$FIXTURE/.config/gust" "$FIXTURE/.local/state/gust"
  printf 'restore_tabs = true\ndensity = "roomy"\n' > "$FIXTURE/.config/gust/settings.toml"
  python3 - "$FIXTURE" <<'PYTABS' || return 1
import os, sys
from pathlib import Path
home = Path(sys.argv[1])
with (home / ".local/state/gust/tabs.toml").open("w") as session:
    names = ("Aloe", "release-2025-build", "release-2026-build", "long-folder-" * 15,
             "Reed", "Pine", "Sage", "Rose")
    for name in names:
        folder = home / name
        folder.mkdir()
        print(f'tab = "local:{os.fsencode(folder).hex()} rows"', file=session)
PYTABS
  start_gust || return 1
  vm hyprctl dispatch 'hl.dsp.window.float()' >/dev/null
  vm hyprctl dispatch 'hl.dsp.window.resize({ x = 520, y = 320 })' >/dev/null
  for _ in {1..4}; do ctrl equal; done
  toolbar_menu_click Search || return 1
  expect "ambiguous tabs at minimum size with large text" \
    "(d['windowWidth'], d['windowHeight'], d['zoom'], d['settings']['density'], d['tabCount'])" "(520, 320, 1.6, 'roomy', 8)"
  for year in 2025 2026; do
    index=$((year - 2024))
    path="$FIXTURE/release-$year-build"
    hover_tab "$index" || return 1
    wait_for "d['tabDisclosure'] and d['tabDisclosure']['path']" "$path" || {
      bad "hover did not disclose $path"
      return 1
    }
    expect "the elided $year tab exposes its full accessible name" \
      "d['tabDisclosure']['truncated'] and d['tabDisclosure']['accessibleName']" "$path"
    expect "the $year disclosure fits inside the window" \
      "0 <= d['tabDisclosure']['left'] < d['tabDisclosure']['right'] <= d['windowWidth'] and 0 <= d['tabDisclosure']['top'] < d['tabDisclosure']['bottom'] <= d['windowHeight']" "True"
    expect "inspecting $year leaves the folder, tabs and Search focus alone" \
      "(d['activeTab'], d['tabCount'], d['searchBarActive'], d['focusOwner'])" "(0, 8, True, 'searchInput')"
    shot "50-tab-path-$year"
  done
  # A path taller than the remaining client area can be read to its end.
  hover_tab 3 || return 1
  wait_for "d['tabDisclosure'] and not d['tabDisclosure']['atEnd']" True || return 1
  point="$(toolbar_point tabDisclosure center)" || return 1
  read -r x y <<<"$point"
  for _ in {1..8}; do
    mouse wheel "$x" "$y" -12 || return 1
    [[ "$(state "d['tabDisclosure'] and d['tabDisclosure']['atEnd']")" == True ]] && break
  done
  expect "the long path scrolls to its last line with the original tab active" \
    "d['tabDisclosure'] and d['tabDisclosure']['atEnd'] and d['activeTab'] == 0" True
  hover_tab 2 || return 1
  wait_for "d['tabDisclosure'] and d['tabDisclosure']['path']" "$FIXTURE/release-2026-build" || return 1
  toolbar_click tabCenters 2 middle || return 1
  expect "middle-click closes the inspected 2026 tab" "(d['activeTab'], d['tabCount'])" "(0, 7)"
  toolbar_click tabCenters 1 || return 1
  expect "the inspected 2025 tab opens the disclosed folder" "d['activeTabPath']" "$FIXTURE/release-2025-build"
}

inspect_exact_tab_paths() {
  local test_home index path root_uri
  test_home="$(mktemp -d "$FIXTURE/tab-bytes-XXXXXXXX")" || return 1
  local FIXTURE="$test_home" REAL_STATE="$test_home/theme"
  mkdir -p "$FIXTURE/.config/gust" "$FIXTURE/.local/state/gust" "$REAL_STATE"
  echo 'restore_tabs = true' > "$FIXTURE/.config/gust/settings.toml"
  python3 - "$FIXTURE" <<'PYTABS' || return 1
import os, sys
from pathlib import Path
home = Path(sys.argv[1])
with (home / ".local/state/gust/tabs.toml").open("w") as session:
    for index, name in enumerate((b"Aloe", b"release\n", b"release<U+000A>", b"release\xfe", b"release\xff")):
        folder = home / os.fsdecode(name)
        folder.mkdir()
        (folder / f"marker-{index}").touch()
        print(f'tab = "local:{os.fsencode(folder).hex()} rows"', file=session)
PYTABS
  start_gust || return 1
  vm hyprctl dispatch 'hl.dsp.window.float()' >/dev/null
  vm hyprctl dispatch 'hl.dsp.window.resize({ x = 900, y = 320 })' >/dev/null

  wait_for "d['windowWidth']" 900 || return 1
  root_uri="$(python3 -c 'from pathlib import Path; import sys; print(Path(sys.argv[1]).as_uri())' "$FIXTURE")" || return 1
  local -a paths=("" "$root_uri/release%0A" "$FIXTURE/release<U+000A>"
                  "$root_uri/release%FE" "$root_uri/release%FF")
  for index in 1 2 3 4; do
    path="${paths[index]}"
    hover_tab "$index" || return 1
    wait_for "bool(d['tabDisclosure'])" True || return 1
    expect "hover identifies unopened tab $index" "d['tabDisclosure']['path']" "$path"
    expect "accessibility identifies unopened tab $index" "d['tabDisclosure']['accessibleName']" "$path"
    expect "inspecting tab $index leaves every tab in place" "(d['activeTab'], d['tabCount'])" "(0, 5)"
    shot "50-tab-bytes-$index"
  done

  # After the hovered FF tab closes, the FE tab must still open its own folder.
  toolbar_click tabCenters 4 middle || return 1
  expect "middle-click closes only the FF tab" "(d['activeTab'], d['tabCount'])" "(0, 4)"
  for index in 3 1; do
    toolbar_click tabCenters "$index" || return 1
    wait_for "'marker-$index' in d['rowPoints']" True || { bad "tab $index opened the wrong folder"; return 1; }
    hover_tab 0 && hover_tab "$index" || return 1
    wait_for "bool(d['tabDisclosure'])" True || return 1
    expect "hover identifies opened tab $index" "d['tabDisclosure']['path']" "${paths[index]}"
    expect "accessibility identifies opened tab $index" "d['tabDisclosure']['accessibleName']" "${paths[index]}"
  done

  # The listing's display path stays the same across this navigation.
  goto "$FIXTURE/release<U+000A>" || return 1
  wait_for "'marker-2' in d['rowPoints']" True || { bad "did not reach the literal marker folder"; return 1; }
  hover_tab 0 && hover_tab 1 || return 1
  wait_for "bool(d['tabDisclosure'])" True || return 1
  expect "a changed key refreshes the opened tab's hover path" "d['tabDisclosure']['path']" "${paths[2]}"
  expect "a changed key refreshes the opened tab's accessible name" "d['tabDisclosure']['accessibleName']" "${paths[2]}"
}

second_tab_toolbar() {
  local index point x y
  local -a names=(Aloe Fern Moss Iris Reed Pine Sage Rose)
  key Down
  key Right
  expect "Roomy density takes effect" "d['settings']['density']" "roomy"
  toolbar_click settingsPoints close || return 1
  ctrl t
  expect "second tab is active" "(d['tabCount'], d['activeTab'])" "(2, 1)"
  for index in 0 1 0 1; do
    toolbar_click tabCenters "$index" || return 1
    expect "tab $index takes its click at minimum size" "d['activeTab']" "$index"
  done
  shot "50-second-tab-roomy"

  toolbar_click toolbarPoints more || return 1
  expect "More opens with two tabs" "d['menuOpen']" "True"
  check_more_width
  shot "50-more-second-tab-roomy"
  toolbar_click menuPoints Search || return 1
  expect "the overflow Search action takes its click" "d['searchBarActive']" "True"
  check_search_query
  shot "50-search-second-tab-roomy"
  key Escape
  toolbar_click toolbarPoints settings || return 1
  expect "Settings opens with two tabs" "d['settingsOpen']" "True"
  toolbar_click settingsPoints close || return 1
  expect "Close returns to the second tab" "(d['settingsOpen'], d['activeTab'])" "(False, 1)"

  for index in "${!names[@]}"; do
    mkdir -p "$FIXTURE/${names[index]}" || return 1
    case "$index" in
      0|1) toolbar_click tabCenters "$index" || return 1 ;;
      *) ctrl t ;;
    esac
    goto "$FIXTURE/${names[index]}" || return 1
  done
  expect "the eighth tab opens in view" \
    "d['activeTab'] == 7 and 0 < d['tabCenters'][7][0] < d['windowWidth']" "True"
  toolbar_menu_click Search || return 1
  expect "Search leaves distinct neighboring labels readable at the end of the strip" \
    "d['searchBarActive'] and d['visibleTabLabels'][-2:] == ['Sage', 'Rose'] and d['tabCenters'][0][0] < 0" "True"
  shot "50-crowded-tabs-search"
  toolbar_click tabCenters 7 || return 1
  for index in "${!names[@]}"; do
    ctrl Tab
    expect "cycling to ${names[index]} reveals its complete label with Search open" \
      "d['searchBarActive'] and d['activeTab'] == $index and '${names[index]}' in d['visibleTabLabels']" "True"
  done
  ctrl Tab
  expect "cycling back reveals the first tab" \
    "d['activeTab'] == 0 and 0 < d['tabCenters'][0][0] < d['windowWidth']" "True"
  point="$(toolbar_point tabCenters 0)" || return 1
  read -r x y <<<"$point"
  # A wheel gesture's pixel distance depends on the device. Scroll until
  # the last caption is fully visible before clicking it.
  for _ in {1..8}; do
    mouse wheel "$x" "$y" -12 || return 1
    [[ "$(state "'Rose' in d['visibleTabLabels']")" == True ]] && break
  done
  expect "scrolling reveals the last folder name" "'Rose' in d['visibleTabLabels']" "True"
  toolbar_click tabCenters 7 || return 1
  expect "scrolling the tab strip makes the eighth tab clickable" "d['activeTab']" "7"
  toolbar_click tabCenters 6 middle || return 1
  expect "middle-click closes the adjacent tab and keeps the last active" \
    "(d['tabCount'], d['activeTab'])" "(7, 6)"
  vm hyprctl dispatch 'hl.dsp.window.resize({ x = 900, y = 320 })' >/dev/null || return 1
  wait_for "d['windowWidth']" "900" || return 1
  toolbar_click tabCenters 0 || return 1
  expect "widening exposes the first tab again" "d['activeTab']" "0"
  toolbar_click tabCenters 6 || return 1
  vm hyprctl dispatch 'hl.dsp.window.resize({ x = 520, y = 320 })' >/dev/null || return 1
  wait_for "d['windowWidth']" "520" || return 1
  expect "shrinking keeps the active tab in view" \
    "d['activeTab'] == 6 and 0 < d['tabCenters'][6][0] < d['windowWidth']" "True"
}

minimum_toolbar() {
  local spacing="$1" base="$2" test_home
  shift 2
  test_home="$(mktemp -d "$FIXTURE/toolbar-XXXXXXXX")" || return 1
  # Keep font metrics and preferences deterministic across desktop themes.
  local FIXTURE="$test_home" REAL_STATE="$test_home/theme"
  local folder="$FIXTURE/long-directory-name-to-force-breadcrumb-overflow/p"
  local zoom last x y step shot_suffix=""
  mkdir -p "$folder/q" "$REAL_STATE"
  if [[ "$spacing" == 2 ]]; then
    shot_suffix="-spacing-$spacing"
    mkdir -p "$REAL_STATE/current/theme"
    echo 'mode = "dark"' > "$REAL_STATE/current/theme/colors.toml"
    printf '[font]\nbase-size = %s\n[spacing]\nscale = 2.0\n' "$base" > "$REAL_STATE/current/theme/shell.toml"
  fi
  for zoom in "$@"; do
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
    expect "theme spacing scale" "d['spacingScale']" "$spacing"
    if [[ "$base" == 28 ]]; then
      second_tab_toolbar || return 1
      inspect_restored_tabs || return 1
      continue
    fi
    shot "50-settings-$zoom$shot_suffix"
    if [[ "$spacing" == 2 ]]; then roomy_settings_options || return 1; fi
    toolbar_click settingsPoints close || return 1
    expect "Close dismisses Settings" "d['settingsOpen']" "False"
    shot "50-breadcrumbs-$zoom$shot_suffix"
    if [[ "$spacing" == 2 ]]; then
      expect "the crowded controls keep their vertical hit area" \
        "0 < d['navButtonSize'][0] < d['navButtonSize'][1]" "True"
      compact_toolbar_navigation "$folder" || return 1
      shot "50-navigation-$zoom$shot_suffix"
      toolbar_menu_click "Search" || return 1
      expect "Search from More takes its click at doubled spacing" "d['searchBarActive']" "True"
      key Escape
      vm hyprctl dispatch 'hl.dsp.window.resize({ x = 900, y = 320 })' >/dev/null
      ctrl comma
      toolbar_click settingsPoints close || return 1
      expect "widening restores the themed navigation width" \
        "d['navButtonSize'][0] == d['navButtonSize'][1]" "True"
      continue
    fi
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
      compact_toolbar_navigation "$folder" || return 1
      compact_toolbar_actions || return 1
    else
      expect "the path hitbox ends before Search" \
        "0 < d['toolbarPoints']['pathStart'][0] < d['toolbarPoints']['pathEnd'][0] <= d['toolbarPoints']['searchStart'][0]" "True"
      toolbar_click toolbarPoints search || return 1
      expect "the adjacent Search button takes its click" "d['searchBarActive']" "True"
      check_search_query
      expect "the search hint fits at normal spacing" "d['searchHintShown']" "True"
      shot "50-search-$zoom"
      key Escape
      toolbar_click toolbarPoints newFolder || return 1
      expect "the adjacent New folder button takes its click" "d['promptOpen']" "True"
      key Escape
    fi
    open_more_at_minimum_height "$zoom" || return 1
    expect "More opens at zoom $zoom" "d['menuOpen']" "True"
    check_more_width
    if [[ "$zoom" == 1.6 ]]; then shot "50-overflow-$zoom"; fi
    key End
    expect "End reveals the last More action" "d['menuCurrent']" "Keyboard shortcuts"
    shot "50-more-$zoom"
    # Reopen at the top. Reach the same bottom action using only the mouse.
    last="$(toolbar_point menuPoints "Keyboard shortcuts")" || return 1
    read -r x y <<<"$last"
    key Return
    expect "the last More action takes Enter" "d['keyMapOpen']" "True"
    key Escape
    open_more_at_minimum_height "$zoom" || return 1
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
      check_search_query
      expect "widening leaves room for the search hint" "d['searchHintShown']" "True"
      key Escape
      toolbar_click toolbarPoints more || return 1
      expect "wide More omits the inline Search action" "d['menuOpen'] and 'Search' not in d['menuItems']" "True"
      check_more_width
    fi
  done
}

say "50 · minimum-size toolbar and panels at default and increased text"
if [[ "$MOUSE_OK" == 1 ]]; then
  inspect_exact_tab_paths || bad "byte-exact tab path journey did not finish"
  minimum_toolbar 1 12 1 1.1 1.6 || bad "minimum-size toolbar journey did not finish"
  minimum_toolbar 2 12 1.6 || bad "doubled-spacing toolbar journey did not finish"
  minimum_toolbar 2 28 1.6 || bad "large-font second-tab toolbar journey did not finish"
else
  bad "minimum-size toolbar journey needs pointer injection"
fi
stop_gust; start_gust "$FIXTURE"
