# shellcheck shell=bash
# Snapping folders into panes (DESIGN §6.7): a folder held to the right edge,
# or a sidebar place held over a pane third, opens there; nothing is moved,
# and the split this makes leaves no extra tab behind when it closes.
say "49 · folders and places snap into panes"
if [[ "$MOUSE_OK" -ne 1 ]]; then
  bad "pane snap journeys did NOT run: no pointer injection"
else
  stop_gust; start_gust "$FIXTURE"
  wait_for "d['model']['settled']" "True" 20 >/dev/null
  read -r WX WY WW WH <<<"$(win_geo)"
  read -r sx sy <<<"$(state "' '.join(map(str, d['rowPoints']['Projects']))")"

  mouse drag $((WX + sx)) $((WY + sy)) $((WX + WW - 12)) $((WY + WH / 2)) 1
  wait_for "d['paneCount']" "2" 10 >/dev/null
  expect "a folder held to the right edge opens a pane" "d['paneCount']" "2"
  expect "the new pane shows that folder" "d['paneLocations'][1]" "$FIXTURE/Projects"
  expect "the left pane stays where it was" "d['paneLocations'][0]" "$FIXTURE"
  [[ -d "$FIXTURE/Projects" ]] && ok "the dragged folder was not moved" || bad "the edge drop moved Projects"
  shot 49-edge-drop
  key F3; sleep 0.8
  expect "F3 closes the snapped pane" "d['paneCount']" "1"
  expect "and leaves no stray tab" "d['tabCount']" "1"

  read -r px py <<<"$(state "' '.join(map(str, d['placePoints']['Documents']))")"
  mouse drag $((WX + px)) $((WY + py)) $((WX + WW * 5 / 6)) $((WY + WH / 2)) 1
  wait_for "d['paneCount']" "2" 10 >/dev/null
  expect "a place held over the right third opens a pane" "d['paneCount']" "2"
  expect "showing that place" "d['paneLocations'][1]" "$FIXTURE/Documents"
  shot 49-place-right
  key F3; sleep 0.8
  expect "closing it leaves one tab" "d['tabCount']" "1"

  mouse drag $((WX + px)) $((WY + py)) $((WX + sx)) $((WY + WH / 2)) 1
  wait_for "d['model']['location']" "$FIXTURE/Documents" 10 >/dev/null
  expect "a place held over the left third opens in this pane" "d['model']['location']" "$FIXTURE/Documents"
  expect "without splitting" "d['paneCount']" "1"

  # A transfer makes the split's own tab the user's, even once undone.
  key BackSpace
  wait_for "d['model']['location']" "$FIXTURE" 10 >/dev/null
  printf 'move me\n' > "$FIXTURE/snap-move.txt"
  wait_for "'snap-move.txt' in d['rowPoints']" "True" 10 >/dev/null
  read -r dx dy <<<"$(state "' '.join(map(str, d['rowPoints']['Documents']))")"
  key F3; sleep 0.8
  read -r fx fy <<<"$(state "' '.join(map(str, d['rowPoints']['snap-move.txt']))")"
  mouse drag $((WX + fx)) $((WY + fy)) $((WX + dx)) $((WY + dy)) 1
  for _ in $(seq 20); do [[ -e "$FIXTURE/Documents/snap-move.txt" ]] && break; sleep 0.5; done
  ctrl z
  for _ in $(seq 20); do [[ -e "$FIXTURE/snap-move.txt" ]] && break; sleep 0.5; done
  [[ -e "$FIXTURE/snap-move.txt" ]] && ok "the move from the split's tab was undone" || bad "snap-move.txt did not come back"
  key F3; sleep 0.8
  expect "a split tab that moved files stays as a tab" "d['tabCount']" "2"
  ctrl w
  rm -f "$FIXTURE/snap-move.txt" "$FIXTURE/Documents/snap-move.txt"
fi
stop_gust; start_gust "$FIXTURE"
