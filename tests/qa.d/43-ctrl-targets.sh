# shellcheck shell=bash
say "43 · Ctrl-copy survives platform handoff to sidebar and path-bar targets"
if [[ "$MOUSE_OK" -ne 1 ]]; then
  bad "Ctrl target journeys did NOT run: no pointer injection"
else
  for target in sidebar crumb; do
    stop_gust
    ROOT="$FIXTURE/ctrl-target-$target"
    mkdir -p "$ROOT/from"
    printf 'Ctrl must keep the original here\n' > "$ROOT/from/copy.txt"
    start_gust "$FIXTURE"; goto "$ROOT/from"
    wait_for "d['model']['settled']" "True" 15 >/dev/null
    read -r WX WY WW WH <<<"$(win_geo)"
    read -r sx sy <<<"$(state "' '.join(map(str, d['rowPoints']['copy.txt']))")"
    if [[ "$target" == sidebar ]]; then
      read -r tx ty <<<"$(state "' '.join(map(str, d['placePoints']['Documents']))")"
      DEST="$FIXTURE/Documents/copy.txt"
    else
      read -r tx ty <<<"$(state "' '.join(map(str, d['crumbPoints']['$ROOT']))")"
      DEST="$ROOT/copy.txt"
    fi
    vm wtype -M ctrl -s 5000 -m ctrl >/dev/null 2>&1 & modifier_pid=$!
    sleep 0.4
    mouse drag $((WX + sx)) $((WY + sy)) $((WX + tx)) $((WY + ty)) 1
    wait "$modifier_pid"
    wait_for "d['jobsRunningAllTabs']" "0" 20 >/dev/null
    cmp -s "$ROOT/from/copy.txt" <(printf 'Ctrl must keep the original here\n') \
      && cmp -s "$DEST" <(printf 'Ctrl must keep the original here\n') \
      && ok "Ctrl $target drop retained both copies" || bad "Ctrl $target drop did not keep both copies"
    shot "43-ctrl-$target"
    ctrl z
    cmp -s "$ROOT/from/copy.txt" <(printf 'Ctrl must keep the original here\n') && [[ ! -e "$DEST" ]] \
      && ok "$target copy Undo left the original intact" || bad "$target copy Undo changed the original"
  done
fi
stop_gust; start_gust "$FIXTURE"
