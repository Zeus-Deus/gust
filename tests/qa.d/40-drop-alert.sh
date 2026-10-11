# shellcheck shell=bash
# The last refusal must not hide the result of a new successful mouse action.
say "40 · a successful mouse drop clears the previous refusal"
if [[ "$MOUSE_OK" -ne 1 ]]; then
  bad "drop-alert journeys did NOT run: no pointer injection"
else
  for kind in sidebar folder; do
    stop_gust
    ROOT="$FIXTURE/drop-alert-$kind"
    mkdir -p "$ROOT/box"
    printf 'keep these dropped bytes\n' > "$ROOT/item.txt"
    start_gust "$ROOT"
    goto "$ROOT"
    wait_for "d['model']['settled']" "True" 15 >/dev/null
    ctrl z
    expect_contains "a refusal is visible before the $kind drop" "d['alertText']" "Cannot undo"
    read -r WX WY WW WH <<<"$(win_geo)"
    read -r sx sy <<<"$(state "' '.join(map(str, d['rowPoints']['item.txt']))")"
    if [[ "$kind" == sidebar ]]; then
      read -r tx ty <<<"$(state "' '.join(map(str, d['placePoints']['Documents']))")"
      DEST="$FIXTURE/Documents/item.txt"
    else
      read -r tx ty <<<"$(state "' '.join(map(str, d['rowPoints']['box']))")"
      DEST="$ROOT/box/item.txt"
    fi
    mouse drag $((WX + sx)) $((WY + sy)) $((WX + tx)) $((WY + ty)) 1
    wait_for "d['jobMessage']" "Moved 1 item" 15 >/dev/null
    wait_for "d['jobRunning']" "False" 10 >/dev/null
    cmp -s "$DEST" <(printf 'keep these dropped bytes\n') && [[ ! -e "$ROOT/item.txt" ]] \
      && ok "the $kind drop really moved the file" || bad "the $kind drop did not move the bytes"
    expect "the old refusal clears after the $kind drop" "d['alertText']" ""
    shot "40-$kind-drop-alert"
    ctrl z
    wait_for "d['model']['count']" "2" 10 >/dev/null
    cmp -s "$ROOT/item.txt" <(printf 'keep these dropped bytes\n') && [[ ! -e "$DEST" ]] \
      && ok "the $kind drop is still undoable" || bad "the $kind drop could not be undone"
  done
fi
stop_gust; start_gust "$FIXTURE"
