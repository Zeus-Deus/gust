# shellcheck shell=bash
# Two Gust windows working together (DESIGN §6.1, D62):
#  - a drag from one window onto a folder in another says what will happen
#    while held, rings that folder, and follows the same rule as a drop
#    inside one window: Move on the same drive, Copy onto another;
#  - Ctrl held copies, Shift held moves;
#  - the other window's sidebar takes the drop;
#  - a cut in one window and a paste in the other moves; a newer copy made
#    elsewhere wins over an older cut.
# Each window is its own process, so every drag here is a real Wayland drag.

XA="$FIXTURE/xa"; XB="$FIXTURE/xb"; OTHER="/var/tmp/gust-qa-xwin-other"
rm -rf "$XA" "$XB" "$OTHER"; mkdir -p "$XA" "$XB/box" "$OTHER"
for f in plain ctrl shift side far cutme copyme; do echo "$f" > "$XA/$f.txt"; done
mkdir -p "$FIXTURE/.config/gtk-3.0"
printf 'file://%s otherdrive\n' "$OTHER" > "$FIXTURE/.config/gtk-3.0/bookmarks"
rm -f "$FIXTURE/Documents/side.txt"
same_dev() { [[ "$(stat -c %d "$1")" == "$(stat -c %d "$2")" ]]; }

SB="$OUTDIR/b-state.json"
stateb() { python3 -c "
import json,sys
try: d=json.load(open('$SB'))
except Exception: print('NO_STATE'); sys.exit(0)
try: print($1)
except Exception as e: print('ERR:'+str(e))" 2>/dev/null; }
geo_of() { vm hyprctl clients -j | python3 -c "
import json,sys
for c in json.load(sys.stdin):
    if c.get('class')=='gust' and c['title'].startswith('$1 '):
        print(c['at'][0], c['at'][1], c['size'][0], c['size'][1]); break"; }
ptA() { state "' '.join(map(str, d['rowPoints']['$1']))"; }
ptB() { stateb "' '.join(map(str, d['$1']['$2']))"; }

start_b() {
  cat > /tmp/gust-qa-run-b.sh <<EOF
#!/usr/bin/env bash
exec > "$OUTDIR/gust-b.log" 2>&1
export HOME="$FIXTURE" XDG_DATA_HOME="$FIXTURE/.local/share" XDG_CACHE_HOME="$FIXTURE/.cache"
export XDG_CONFIG_HOME="$FIXTURE/.config" XDG_STATE_HOME="$FIXTURE/.local/state"
export GUST_OMARCHY_STATE="$REAL_STATE" GUST_QA_STATE="$SB"
exec "$BIN" "$XB"
EOF
  chmod +x /tmp/gust-qa-run-b.sh; rm -f "$SB"
  vm /tmp/gust-qa-run-b.sh 9>&- &
  for _ in $(seq 1 40); do [[ -f "$SB" ]] && break; sleep 0.3; done
  sleep 2
}
# xdrag <file in A> <x> <y> <tag> [modifier]: hold 3 s over B, read B's hint.
xdrag() {
  local f="$1" tx="$2" ty="$3" tag="$4" mod="${5:-}" a kp=""
  a="$(ptA "$f")"
  if [[ "$a" == ERR* || -z "$a" ]]; then bad "$f is not on screen in window A"; return 1; fi
  read -r fx fy <<<"$a"
  read -r AX AY AW AH <<<"$(geo_of "$XA")"
  # A modifier is held from before the press until after the release.
  if [[ -n "$mod" ]]; then vm wtype -M "$mod" -s 6500 -m "$mod" >/dev/null 2>&1 & kp=$!; sleep 0.4; fi
  ( mouse drag $((AX + fx)) $((AY + fy)) "$tx" "$ty" 3 ) & local mp=$!
  sleep 2.9
  HELD="$(stateb "d['hintStatus']")"
  RING="$(stateb "d['dropRing']")"
  shot "36-$tag-held"
  wait "$mp"; [[ -n "$kp" ]] && wait "$kp"
  sleep 2.5
}

if [[ "$MOUSE_OK" -ne 1 ]]; then
  bad "no pointer injection: section 36 did NOT run"
else
  stop_gust; start_gust "$FIXTURE"
  goto "$XA"; wait_for "d['model']['settled']" "True" 20 >/dev/null
  start_b
  read -r BX BY BW BH <<<"$(geo_of "$XB")"
  [[ -n "$BX" ]] && ok "a second window opened beside the first" || bad "no second window"
  same_dev "$XA" "$XB" || bad "the fixture spans drives: the move cases mean nothing"

  say "36a · drag onto a folder in the other window: says Move, rings it, moves"
  read -r bx by <<<"$(ptB rowPoints box)"
  xdrag plain.txt $((BX + bx)) $((BY + by)) move
  [[ "$HELD" == "Move 1 item to box" ]] && ok "while held, window B says: $HELD" || bad "window B said '$HELD'"
  [[ "$RING" == "0" ]] && ok "window B rings the box folder" || bad "no ring on box (dropRing=$RING)"
  [[ -f "$XB/box/plain.txt" && ! -e "$XA/plain.txt" ]] && ok "plain.txt moved into box" \
    || bad "not moved (A: $(ls "$XA" | tr '\n' ' ') B/box: $(ls "$XB/box" | tr '\n' ' '))"

  say "36b · Ctrl held: copy"
  xdrag ctrl.txt $((BX + bx)) $((BY + by)) ctrl ctrl
  [[ "$HELD" == "Copy 1 item to box" ]] && ok "with Ctrl it says: $HELD" || bad "with Ctrl it said '$HELD'"
  [[ -f "$XB/box/ctrl.txt" && -f "$XA/ctrl.txt" ]] && ok "ctrl.txt copied, the original stayed" || bad "Ctrl did not copy"

  say "36c · onto the other window's sidebar (Documents): moves there"
  read -r dx dy <<<"$(ptB placePoints Documents)"
  xdrag side.txt $((BX + dx)) $((BY + dy)) sidebar
  if same_dev "$XA" "$FIXTURE/Documents"; then want="Move 1 item to Documents"; else want="Copy 1 item to Documents"; fi
  [[ "$HELD" == "$want" ]] && ok "over B's sidebar it says: $HELD" || bad "over B's sidebar it said '$HELD' (wanted '$want')"
  [[ -f "$FIXTURE/Documents/side.txt" ]] && ok "side.txt landed in Documents" || bad "the sidebar drop did nothing"

  say "36d · onto a bookmark on another drive in the other window: copies"
  if same_dev "$XA" "$OTHER"; then
    bad "/var/tmp is on the fixture's drive: the copy case did NOT run"
  else
    read -r ox oy <<<"$(ptB placePoints bookmark:otherdrive)"
    xdrag far.txt $((BX + ox)) $((BY + oy)) otherdrive
    [[ "$HELD" == "Copy 1 item to gust-qa-xwin-other" ]] && ok "onto another drive it says: $HELD" || bad "it said '$HELD'"
    [[ -f "$OTHER/far.txt" && -f "$XA/far.txt" ]] && ok "far.txt copied, the original stayed" || bad "not copied"
  fi

  say "36e · Shift held onto another drive: move"
  if ! same_dev "$XA" "$OTHER"; then
    xdrag shift.txt $((BX + ox)) $((BY + oy)) shift shift
    [[ "$HELD" == "Move 1 item to gust-qa-xwin-other" ]] && ok "with Shift it says: $HELD" || bad "with Shift it said '$HELD'"
    [[ -f "$OTHER/shift.txt" && ! -e "$XA/shift.txt" ]] && ok "shift.txt moved across drives" || bad "Shift did not move"
  fi

  say "36f · cut in window A, paste in window B: moves"
  read -r AX AY AW AH <<<"$(geo_of "$XA")"; read -r cx cy <<<"$(ptA cutme.txt)"
  mouse click $((AX + cx)) $((AY + cy)); ctrl x; sleep 0.8
  expect_contains "window A says it cut the file" "d['hintStatus']" "Cut cutme.txt"
  mouse click $((BX + BW / 2)) $((BY + BH - 80)); ctrl v; sleep 2.5
  [[ -f "$XB/cutme.txt" && ! -e "$XA/cutme.txt" ]] && ok "the cut file moved to window B's folder" \
    || bad "cut/paste across windows did not move ($(ls "$XA" "$XB" | tr '\n' ' '))"

  say "36g · a newer copy made elsewhere wins over an older cut"
  read -r AX AY AW AH <<<"$(geo_of "$XA")"
  # A cuts ctrl.txt, then B copies box/ctrl.txt; pasting in A must take B's copy.
  read -r cx cy <<<"$(ptA ctrl.txt)"
  mouse click $((AX + cx)) $((AY + cy)); ctrl x; sleep 0.8
  echo "from b" > "$XB/fromb.txt"; sleep 1.5
  read -r fbx fby <<<"$(ptB rowPoints fromb.txt)"
  mouse click $((BX + fbx)) $((BY + fby)); ctrl c; sleep 0.8
  mouse click $((AX + AW / 2)) $((AY + AH - 80)); ctrl v; sleep 2.5
  [[ -f "$XA/fromb.txt" && -f "$XB/fromb.txt" ]] && ok "window A pasted B's newer copy" || bad "A did not paste B's copy"
  [[ -f "$XA/ctrl.txt" ]] && ok "and A's older cut moved nothing" || bad "A moved its old cut instead"
  shot 36-two-windows-done

  pkill -f "$BIN $XB" 2>/dev/null
fi
stop_gust
rm -rf "$XA" "$XB" "$OTHER" "$FIXTURE/.config/gtk-3.0/bookmarks" "$FIXTURE/Documents/side.txt"
start_gust "$FIXTURE"
