# shellcheck shell=bash
say "42 · right-drag Link clears old alerts, while mouse Cancel changes nothing"
if [[ "$MOUSE_OK" -ne 1 ]]; then
  bad "right-drag Link/Cancel did NOT run: no pointer injection"
else
  for answer in Cancel 'Link here'; do
    stop_gust
    ROOT="$FIXTURE/right-drag-alert-${answer// /-}"
    mkdir -p "$ROOT/box"
    printf 'link points at these original bytes\n' > "$ROOT/item.txt"
    start_gust "$FIXTURE"; goto "$ROOT"
    wait_for "d['model']['settled']" "True" 15 >/dev/null
    ctrl z
    expect_contains "an old refusal is present" "d['alertText']" "Cannot undo"
    read -r WX WY WW WH <<<"$(win_geo)"
    read -r sx sy <<<"$(state "' '.join(map(str, d['rowPoints']['item.txt']))")"
    read -r tx ty <<<"$(state "' '.join(map(str, d['rowPoints']['box']))")"
    mouse rdrag $((WX + sx)) $((WY + sy)) $((WX + tx)) $((WY + ty))
    if ! wait_for "d['menuOpen']" "True" 15; then
      bad "$answer: right-drag menu never opened"
      continue
    fi
    read -r mx my <<<"$(state "' '.join(map(str, d['menuPoints']['$answer']))")"
    mouse click $((WX + mx)) $((WY + my))
    if ! wait_for "d['menuOpen']" "False" 10; then
      bad "$answer did not close the drop menu"
      shot "42-$answer-not-dismissed"
      continue
    fi
    expect "$answer closed the drop menu" "d['menuOpen']" "False"
    expect "$answer returned keyboard focus" "d['focusOwner']" "listFocus"
    cmp -s "$ROOT/item.txt" <(printf 'link points at these original bytes\n') \
      && ok "$answer kept the original bytes" || bad "$answer damaged the source"
    if [[ "$answer" == Cancel ]]; then
      [[ -z "$(command ls -A "$ROOT/box")" ]] \
        && ok "mouse Cancel created nothing" || bad "mouse Cancel still acted on the drop"
    else
      [[ -L "$ROOT/box/item.txt" ]] \
        && [[ "$(readlink "$ROOT/box/item.txt")" == "$ROOT/item.txt" ]] \
        && ok "Link here made the intended symlink" || bad "Link here did not make the intended link"
      expect "a successful Link clears the old refusal" "d['alertText']" ""
      shot 42-link-alert
      ctrl z
      [[ ! -L "$ROOT/box/item.txt" ]] && cmp -s "$ROOT/item.txt" <(printf 'link points at these original bytes\n') \
        && ok "Undo removed only the link" || bad "Link Undo changed the target"
    fi
  done
fi
stop_gust; start_gust "$FIXTURE"
