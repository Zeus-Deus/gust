# shellcheck shell=bash
say "41 · cross-pane clashes ask on the source model, in both directions"
if [[ "$MOUSE_OK" -ne 1 ]]; then
  bad "cross-pane conflict journeys did NOT run: no pointer injection"
else
  for direction in a-to-b b-to-a; do
    for answer in cancel skip keep-both replace; do
      stop_gust
      ROOT="$FIXTURE/pane-conflict-$direction-$answer"
      A="$ROOT/a"; B="$ROOT/b"
      mkdir -p "$A" "$B"
      if [[ "$direction" == a-to-b ]]; then SRC="$A"; DST="$B"; else SRC="$B"; DST="$A"; fi
      printf 'incoming original bytes\n' > "$SRC/clash.txt"
      printf 'preexisting destination bytes\n' > "$DST/clash.txt"
      start_gust "$FIXTURE"; goto "$A"
      wait_for "d['model']['settled']" "True" 15 >/dev/null
      read -r ax ay <<<"$(state "' '.join(map(str, d['rowPoints']['clash.txt']))")"
      key F3; goto "$B"
      wait_for "d['model']['settled']" "True" 15 >/dev/null
      expect "left location is independent" "d['paneLocations'][0]" "$A"
      expect "right location is independent" "d['paneLocations'][1]" "$B"
      read -r bx by <<<"$(state "' '.join(map(str, d['rowPoints']['clash.txt']))")"
      read -r WX WY WW WH <<<"$(win_geo)"
      if [[ "$direction" == a-to-b ]]; then
        sx=$ax; sy=$ay; tx=$((WW * 4 / 5))
      else
        sx=$bx; sy=$by; tx=$((WW * 2 / 5))
      fi
      mouse drag $((WX + sx)) $((WY + sy)) $((WX + tx)) $((WY + WH - 140)) 1
      if ! wait_for "d['conflictOpen']" "True" 20; then
        bad "$direction $answer: a real clash disappeared without asking"
        shot "41-$direction-$answer-missing"
        continue
      fi
      expect "$direction $answer names the clash" "d['conflictName']" "clash.txt"
      cmp -s "$SRC/clash.txt" <(printf 'incoming original bytes\n') \
        && cmp -s "$DST/clash.txt" <(printf 'preexisting destination bytes\n') \
        && ok "nothing changes before $answer" || bad "a clash changed bytes before consent"
      shot "41-$direction-$answer-question"
      case "$answer" in
        cancel) key Escape ;;
        skip) key s ;;
        keep-both) key k ;;
        replace) key r ;;
      esac
      if ! wait_for "d['conflictOpen']" "False" 15; then
        bad "$direction $answer did not dismiss its question"
        shot "41-$direction-$answer-not-dismissed"
        continue
      fi
      expect "$direction $answer closed the question" "d['conflictOpen']" "False"
      expect "$direction $answer returned keyboard focus" "d['focusOwner']" "listFocus"
      if ! wait_for "d['jobsRunningAllTabs']" "0" 30; then
        bad "$direction $answer left a transfer running"
        continue
      fi
      if [[ "$answer" == cancel || "$answer" == skip ]]; then
        cmp -s "$SRC/clash.txt" <(printf 'incoming original bytes\n') \
          && cmp -s "$DST/clash.txt" <(printf 'preexisting destination bytes\n') \
          && ok "$direction $answer preserved both files" || bad "$direction $answer changed bytes"
      else
        incoming="$DST/clash.txt"
        [[ "$answer" == keep-both ]] && incoming="$DST/clash (copy).txt"
        cmp -s "$incoming" <(printf 'incoming original bytes\n') && [[ ! -e "$SRC/clash.txt" ]] \
          && ok "$direction $answer really completed the move" || bad "$direction $answer did not move the intended bytes"
        if [[ "$answer" == keep-both ]]; then
          cmp -s "$DST/clash.txt" <(printf 'preexisting destination bytes\n') \
            && ok "Keep both retained the destination's original" || bad "Keep both replaced the destination"
        fi
        ctrl z
        cmp -s "$SRC/clash.txt" <(printf 'incoming original bytes\n') \
          && cmp -s "$DST/clash.txt" <(printf 'preexisting destination bytes\n') \
          && ok "$direction $answer Undo restored both originals" || bad "$direction $answer Undo lost or misplaced bytes"
      fi
    done
  done

  say "41b · Ctrl remains Copy when a drag crosses into the other pane"
  stop_gust
  ROOT="$FIXTURE/pane-ctrl-copy"
  mkdir -p "$ROOT/a" "$ROOT/b"
  printf 'Ctrl must retain this original\n' > "$ROOT/a/copy.txt"
  start_gust "$FIXTURE"; goto "$ROOT/a"
  wait_for "d['model']['settled']" "True" 15 >/dev/null
  read -r sx sy <<<"$(state "' '.join(map(str, d['rowPoints']['copy.txt']))")"
  key F3; goto "$ROOT/b"
  read -r WX WY WW WH <<<"$(win_geo)"
  vm wtype -M ctrl -s 5000 -m ctrl >/dev/null 2>&1 & modifier_pid=$!
  sleep 0.4
  mouse drag $((WX + sx)) $((WY + sy)) $((WX + WW * 4 / 5)) $((WY + WH - 140)) 1
  wait "$modifier_pid"
  wait_for "d['jobsRunningAllTabs']" "0" 20 || bad "Ctrl-copy did not retire its transfer"
  cmp -s "$ROOT/a/copy.txt" <(printf 'Ctrl must retain this original\n') \
    && cmp -s "$ROOT/b/copy.txt" <(printf 'Ctrl must retain this original\n') \
    && ok "Ctrl cross-pane drag copied without removing the original" \
    || bad "Ctrl cross-pane drag did not preserve both copies"
  shot 41-ctrl-copy
  ctrl z
  cmp -s "$ROOT/a/copy.txt" <(printf 'Ctrl must retain this original\n') && [[ ! -e "$ROOT/b/copy.txt" ]] \
    && ok "Undo removed only the Ctrl-drag copy" || bad "Ctrl-drag Undo did not preserve the original"
fi
stop_gust; start_gust "$FIXTURE"
