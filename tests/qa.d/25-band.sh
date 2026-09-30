# shellcheck shell=bash
# Rubber-band select with a real pointer: drag on empty space selects what the
# band covers; Ctrl+drag adds; a click on empty space selects nothing; a drag
# that starts ON a row still moves files, not a band.

say "25a · dragging on empty space draws a selection band"
if [[ "$MOUSE_OK" == 1 ]]; then
  d25="$FIXTURE/band"
  rm -rf "$d25"; mkdir -p "$d25"
  for i in 1 2 3 4 5 6; do printf x > "$d25/f$i.txt"; done
  stop_gust; start_gust "$FIXTURE"
  wait_for "d['model']['settled']" "True" 20 >/dev/null
  goto "$d25" || bad "could not open $d25"
  wait_for "d['model']['count']" "6" 10 >/dev/null
  read -r WX WY WW WH <<<"$(win_geo)"
  key Home
  pt="$(state "' '.join(map(str, d['cursorPoint']))")"; read -r PX PY <<<"$pt"
  # Start right of the names (empty row space is still a row), so start
  # BELOW the last row: 6 rows down plus two more.
  rh="$(state "int(d['rowHeight'])")"
  sx=$((WX + WW - 60)); sy=$((WY + PY + rh * 7))
  ex=$((WX + PX + 40));  ey=$((WY + PY + rh * 3))
  mouse drag "$sx" "$sy" "$ex" "$ey"; sleep 0.8
  sel="$(state "d['selCount']")"
  [[ "$sel" =~ ^[0-9]+$ && "$sel" -ge 3 && "$sel" -le 4 ]] \
    && ok "the band selected the $sel rows it covered" || bad "band selected '$sel' rows (want 3–4)"
  shot 25a-band
  mouse click "$sx" "$sy"; sleep 0.6
  expect "a click on empty space selects nothing" "d['selCount']" "0"

  # Ctrl+drag (add to the selection) is covered by the band_tests unit
  # test: the uinput helper cannot hold a keyboard modifier during a drag.
  stop_gust; rm -rf "$d25"; start_gust "$FIXTURE"
else
  bad "no pointer injection: the rubber-band case did NOT run"
fi
