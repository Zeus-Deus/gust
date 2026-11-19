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
expect_location "the actual command-line folder is outside HOME" "$OUTDIR/elsewhere/deep"
if [[ "$MOUSE_OK" -eq 1 ]]; then
  read -r WX WY WW WH <<<"$(win_geo)"
  read -r hx hy <<<"$(state "' '.join(map(str, d['placePoints']['Home']))")"
  mouse click $((WX + hx)) $((WY + hy))
  wait_location "$FIXTURE" 5 >/dev/null
  expect_location "Home opens the configured deep HOME" "$FIXTURE"
  shot 38-home-click
  goto "$OUTDIR/elsewhere/deep"
  wait_location "$OUTDIR/elsewhere/deep" 10 >/dev/null
  read -r dx dy <<<"$(state "' '.join(map(str, d['placePoints']['Documents']))")"
  mouse click $((WX + dx)) $((WY + dy))
  wait_location "$FIXTURE/Documents" 5 >/dev/null
  expect_location "Documents does not follow the browsed folder" "$FIXTURE/Documents"
  expect "Documents contains the private fixture file" "d['model']['cursor_name']" "notes.txt"
else
  bad "Home mouse journey did NOT run: no pointer injection"
fi
stop_gust
FIXTURE="$SAVED_FIXTURE"
start_gust "$FIXTURE"
