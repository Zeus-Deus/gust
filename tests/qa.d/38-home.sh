# shellcheck shell=bash
# Home is the configured home, never the first two components of a folder.
# A nested fixture is intentional: /tmp/gust-fixture would hide the old bug.
say "38 · sidebar Home stays tied to HOME, including a deep fixture"
SAVED_FIXTURE="$FIXTURE"
stop_gust
FIXTURE="$OUTDIR/nested/home"
mkdir -p "$FIXTURE/Documents" "$FIXTURE/Downloads" "$FIXTURE/Pictures" \
  "$FIXTURE/Videos" "$FIXTURE/Music" "$OUTDIR/elsewhere/deep"
printf 'private home sentinel\n' > "$FIXTURE/home-only.txt"
printf 'document sentinel\n' > "$FIXTURE/Documents/notes.txt"
start_gust "$OUTDIR/elsewhere/deep"
wait_for "d['model']['settled']" "True" 15 >/dev/null
expect "the actual command-line folder is outside HOME" "d['model']['location']" "$OUTDIR/elsewhere/deep"
if [[ "$MOUSE_OK" -eq 1 ]]; then
  read -r WX WY WW WH <<<"$(win_geo)"
  read -r hx hy <<<"$(state "' '.join(map(str, d['placePoints']['Home']))")"
  mouse click $((WX + hx)) $((WY + hy))
  wait_for "d['model']['location']" "$FIXTURE" 5 >/dev/null
  expect "Home opens the configured deep HOME" "d['model']['location']" "$FIXTURE"
  shot 38-home-click
  goto "$OUTDIR/elsewhere/deep"
  wait_for "d['model']['location']" "$OUTDIR/elsewhere/deep" 10 >/dev/null
  read -r dx dy <<<"$(state "' '.join(map(str, d['placePoints']['Documents']))")"
  mouse click $((WX + dx)) $((WY + dy))
  wait_for "d['model']['location']" "$FIXTURE/Documents" 5 >/dev/null
  expect "Documents does not follow the browsed folder" "d['model']['location']" "$FIXTURE/Documents"
  expect "Documents contains the private fixture file" "d['model']['cursor_name']" "notes.txt"
else
  bad "Home mouse journey did NOT run: no pointer injection"
fi
stop_gust
FIXTURE="$SAVED_FIXTURE"
start_gust "$FIXTURE"
