# shellcheck shell=bash
# Holding a file and dragging it onto a folder works in every view (DESIGN
# §6.1): Rows, Grid and Columns. While it is held, the dragged item follows
# the pointer and the folder under it is ringed, so the gesture is visible.

dd="$FIXTURE/drag"
rm -rf "$dd"; mkdir -p "$dd/inbox"
for f in alpha beta gamma; do echo "$f" > "$dd/$f.txt"; done

pt() { state "' '.join(map(str, d['rowPoints']['$1']))"; }
drag_case() { # $1 view label, $2 file to drag
  local view="$1" file="$2"
  # A fresh window per view: nothing left over from the previous drag.
  stop_gust; start_gust "$FIXTURE"
  wait_for "d['model']['settled']" "True" 20 >/dev/null
  goto "$dd" || bad "could not open $dd"
  wait_for "d['model']['settled']" "True" 20 >/dev/null
  palette_run "View: $view"; sleep 1.5
  palette_open >/dev/null; key Escape; sleep 0.8
  expect "$view view" "d['view']" "$(tr '[:upper:]' '[:lower:]' <<<"$view")"
  read -r WX WY WW WH <<<"$(win_geo)"
  local a b
  a="$(pt "$file")"; b="$(pt inbox)"
  if [[ "$a" == ERR* || -z "$a" || "$b" == ERR* || -z "$b" ]]; then
    bad "$view: $file or inbox not on screen ($a / $b)"; return
  fi
  read -r AX AY <<<"$a"; read -r BX BY <<<"$b"
  # Hold at the end, and look while it is held.
  ( mouse drag $((WX + AX)) $((WY + AY)) $((WX + BX)) $((WY + BY)) 3 ) &
  local mp=$!
  sleep 2.4
  expect "$view: a drag is in flight while the button is held" "d['dragging']" "True"
  expect_contains "$view: it says where the drop goes" "d['hintStatus']" "inbox"
  expect "$view: the file follows the pointer" "d['dragGhost']" "$file"
  shot "34-$view-held"
  wait "$mp"
  sleep 2.5
  [[ -f "$dd/inbox/$file" ]] && ok "$view: dropping $file on inbox copied it there" \
    || bad "$view: dropping $file on inbox did nothing"
  [[ -f "$dd/$file" ]] && ok "$view: the original stays (copy is the default)" \
    || bad "$view: the original is gone"
}

if [[ "$MOUSE_OK" -ne 1 ]]; then
  bad "SKIPPED: no pointer injection"
else
  say "34a · drag in Rows"
  drag_case Rows alpha.txt
  say "34b · drag in Grid"
  drag_case Grid beta.txt
  say "34c · drag in Columns"
  drag_case Columns gamma.txt
fi

palette_run "View: Rows"; sleep 0.8
stop_gust
rm -rf "$dd"
start_gust "$FIXTURE"
