# shellcheck shell=bash
# Holding files and dragging them between folders (DESIGN §6.1, D60):
#  - on the same drive a plain drop MOVES, in Rows, Grid and Columns;
#  - onto another drive it COPIES (the original stays);
#  - folders in the sidebar and the path bar take drops too;
#  - while held, the item and what releasing will do follow the pointer;
#  - Ctrl+Z puts a moved file back.
# /tmp (the fixture) is tmpfs and the home folder is on the root disk, so
# the two really are different drives in the VM.

dd="$FIXTURE/drag"
# The fixture is on tmpfs; /var/tmp is on the root disk: another drive.
docs="/var/tmp/gust-qa-otherdrive"
rm -rf "$dd" "$docs"; mkdir -p "$docs"
mkdir -p "$FIXTURE/.config/gtk-3.0"
printf 'file://%s otherdrive\n' "$docs" > "$FIXTURE/.config/gtk-3.0/bookmarks"
mkdir -p "$dd/inbox/deep"
for f in alpha beta; do echo "$f" > "$dd/$f.txt"; done
echo gamma > "$dd/qa-drag-gamma.txt"
echo delta > "$dd/inbox/deep/delta.txt"
same_dev() { [[ "$(stat -c %d "$1")" == "$(stat -c %d "$2")" ]]; }

pt() { state "' '.join(map(str, d['rowPoints']['$1']))"; }
fresh() { # $1 view, $2 folder
  stop_gust; start_gust "$FIXTURE"
  wait_for "d['model']['settled']" "True" 20 >/dev/null
  goto "$2" || bad "could not open $2"
  wait_for "d['model']['settled']" "True" 20 >/dev/null
  palette_run "View: $1"; sleep 1.5
  palette_open >/dev/null; key Escape; sleep 0.8
  read -r WX WY WW WH <<<"$(win_geo)"
}
# drag_to <file> <x> <y> <label>: hold 3 s at the target, check the chip.
drag_to() {
  local file="$1" tx="$2" ty="$3" label="$4" a
  a="$(pt "$file")"
  if [[ "$a" == ERR* || -z "$a" ]]; then bad "$label: $file not on screen"; return 1; fi
  read -r AX AY <<<"$a"
  ( mouse drag $((WX + AX)) $((WY + AY)) "$tx" "$ty" 3 ) &
  local mp=$!
  sleep 2.4
  DRAG_STATUS="$(state "d['hintStatus']")"
  echo "  (while held: $DRAG_STATUS)"
  shot "34-${label// /-}-held"
  wait "$mp"
  sleep 2.5
}

if [[ "$MOUSE_OK" -ne 1 ]]; then
  bad "SKIPPED: no pointer injection"
else
  same_dev "$dd" "$dd/inbox" || bad "fixture spans drives: the move cases mean nothing"

  i=0
  for view in Rows Grid Columns; do
    case $view in
      Rows) file=alpha.txt ;;
      Grid) file=beta.txt ;;
      # Columns drags alpha again: put it back first (plain mv, not Gust).
      Columns) mv "$dd/inbox/alpha.txt" "$dd/alpha.txt"; file=alpha.txt ;;
    esac
    say "34$((++i)) · $view: drag a file onto a folder on the same drive"
    fresh "$view" "$dd"
    b="$(pt inbox)"; read -r BX BY <<<"$b"
    drag_to "$file" $((WX + BX)) $((WY + BY)) "$view move" || continue
    [[ "$DRAG_STATUS" == "Move 1 item to inbox" ]] && ok "$view: while held it says Move 1 item to inbox" \
      || bad "$view: while held it said '$DRAG_STATUS'"
    [[ -f "$dd/inbox/$file" ]] && ok "$view: $file is in inbox" || bad "$view: $file did not arrive"
    [[ ! -e "$dd/$file" ]] && ok "$view: and is no longer where it was (moved)" || bad "$view: the original is still there"
  done

  say "34u · Ctrl+Z puts a moved file back"
  ctrl z; sleep 1.5
  [[ -f "$dd/alpha.txt" && ! -e "$dd/inbox/alpha.txt" ]] && ok "undo moved alpha.txt back" \
    || bad "undo did not move it back ($(ls "$dd" "$dd/inbox" | tr '\n' ' '))"

  say "34s · drag onto a sidebar bookmark on another drive: copy"
  if same_dev "$dd" "$docs"; then
    bad "the fixture and /var/tmp share a drive: the copy case did NOT run"
  else
    fresh Rows "$dd"
    pp="$(state "' '.join(map(str, d['placePoints']['bookmark:otherdrive']))")"
    if [[ "$pp" == ERR* || -z "$pp" ]]; then bad "no otherdrive bookmark in the sidebar"; else
      read -r PX PY <<<"$pp"
      drag_to qa-drag-gamma.txt $((WX + PX)) $((WY + PY)) "sidebar copy"
      [[ "$DRAG_STATUS" == "Copy 1 item to gust-qa-otherdrive" ]] && ok "over the bookmark it says Copy 1 item to gust-qa-otherdrive" \
        || bad "over the bookmark it said '$DRAG_STATUS'"
      [[ -f "$docs/qa-drag-gamma.txt" ]] && ok "the file landed on the other drive" || bad "nothing landed on the other drive"
      [[ -f "$dd/qa-drag-gamma.txt" ]] && ok "the original stayed (another drive: copy)" || bad "the original is gone"
    fi
  fi

  say "34b · drag onto a path-bar crumb: into that folder"
  fresh Rows "$dd/inbox/deep"
  cp_="$(state "' '.join(map(str, d['crumbPoints']['$dd']))")"
  if [[ "$cp_" == ERR* || -z "$cp_" ]]; then bad "no crumb for $dd"; else
    read -r CX CY <<<"$cp_"
    drag_to delta.txt $((WX + CX)) $((WY + CY)) "crumb move"
    [[ "$DRAG_STATUS" == "Move 1 item to drag" ]] && ok "over the crumb it says Move 1 item to drag" \
      || bad "over the crumb it said '$DRAG_STATUS'"
    [[ -f "$dd/delta.txt" && ! -e "$dd/inbox/deep/delta.txt" ]] && ok "delta.txt moved up into drag" \
      || bad "the crumb drop did nothing ($(ls "$dd" | tr '\n' ' '))"
  fi
fi

stop_gust
rm -rf "$dd" "$docs" "$FIXTURE/.config/gtk-3.0/bookmarks"
start_gust "$FIXTURE"
