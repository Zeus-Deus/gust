# shellcheck shell=bash
# Visual pass (v1 scope): every surface the owner listed, shot in one theme.
# Run with tests/qa-one.sh tests/visual-pass.sh <outdir>; set GUST_OMARCHY_STATE
# to a private theme state dir to shoot another theme (tests/visual-pass-run.sh).
# Each surface is asserted open before it is shot, so a screenshot never
# shows the wrong thing.

vp="$FIXTURE/visual"; rm -rf "$vp"; mkdir -p "$vp/Photos" "$vp/Reports" "$vp/dest"
printf 'Quarterly numbers\nline two\n' > "$vp/notes.txt"
printf '# Plan\n\n- ship v1\n' > "$vp/plan.md"
printf 'a,b\n1,2\n' > "$vp/data.csv"
printf 'x' > "$vp/dest/notes.txt"
for i in 1 2 3; do printf 'r%s' "$i" > "$vp/Reports/report-$i.txt"; done
python3 -c "
import zlib, struct
w,h=64,40
rows=b''.join(b'\x00'+b''.join(bytes([x*4%256,y*6%256,180]) for x in range(w)) for y in range(h))
c=lambda t,d: struct.pack('>I',len(d))+t+d+struct.pack('>I',zlib.crc32(t+d)&0xffffffff)
open('$vp/Photos/sunset.png','wb').write(b'\x89PNG\r\n\x1a\n'+c(b'IHDR',struct.pack('>IIBBBBB',w,h,8,2,0,0,0))+c(b'IDAT',zlib.compress(rows))+c(b'IEND',b''))
"

stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
goto "$vp" || bad "could not open $vp"
wait_for "d['model']['count']" "6" 10 >/dev/null
key Home; key Down; key Up

say "VP · main window: rows, grid, columns"
expect "rows view" "d['view']" "rows"
shot vp-rows
palette_run "View: Grid"; sleep 1.5
expect "grid view" "d['view']" "grid"
shot vp-grid
palette_run "View: Columns"; sleep 1.5
expect "columns view" "d['view']" "columns"
shot vp-columns
palette_run "View: Rows"; sleep 1.2

say "VP · context menu"
key Home
if [[ "$MOUSE_OK" == 1 ]]; then
  read -r WX WY WW WH <<<"$(win_geo)"
  read -r px py <<<"$(state "' '.join(map(str, d['cursorPoint']))")"
  mouse click $((WX + px + 40)) $((WY + py)) right; sleep 0.8
else
  vm wtype -M shift -k F10 -m shift >/dev/null 2>&1; sleep 0.8
fi
expect "the context menu is open" "d['menuOpen']" "True"
key Escape; sleep 0.4
vm wtype -M shift -k F10 -m shift >/dev/null 2>&1; sleep 0.8
expect "Shift+F10 opens it too" "d['menuOpen']" "True"
expect "and starts on its first entry" "d['menuCurrent']" "Open"
shot vp-context-menu
key Escape; sleep 0.4

say "VP · dialogs"
# The right-click above selected a row; every step below acts on the cursor.
palette_run "Select nothing"; sleep 0.4
expect "nothing is selected" "d['selCount']" "0"
typ "notes"; sleep 1.2
key Delete; wait_for "d['confirmOpen']" "True" 10 >/dev/null
expect "the delete confirmation is open" "d['confirmOpen']" "True"
shot vp-dialog-confirm
key Escape; sleep 0.5
key F2; sleep 0.6
expect "the rename prompt is open" "d['promptOpen']" "True"
shot vp-dialog-rename
key Escape; sleep 0.4
palette_open; typ "Owner"; sleep 0.8
shot vp-palette
key Escape; sleep 0.3
palette_run "Owner"; sleep 0.8
expect "the owner prompt is open" "d['promptMode']" "owner"
shot vp-dialog-owner
key Escape; sleep 0.4
palette_run "Permissions…"; sleep 0.8
wait_for "d['permissions'] and d['permissions']['ready']" "True" 10 >/dev/null
expect "the permissions card is open" "d['permissionsOpen']" "True"
shot vp-dialog-permissions
key Escape; sleep 0.4
# A name clash: copy notes.txt into dest, which has a notes.txt.
key Home; typ "notes"; sleep 1.2
wait_for "d['model']['cursor_name']" "notes.txt" 10 >/dev/null
expect "the cursor is on notes.txt before Ctrl+C" "d['model']['cursor_name']" "notes.txt"
ctrl c; sleep 0.4
typ "dest"; sleep 1.2
wait_for "d['model']['cursor_name']" "dest" 10 >/dev/null
key Return
wait_for "d['model']['location']" "$vp/dest" 10 >/dev/null
ctrl v
wait_for "d['conflictOpen']" "True" 15 >/dev/null
expect "the name-clash dialog is open" "d['conflictOpen']" "True"
shot vp-dialog-conflict
key Escape; sleep 1.0
key BackSpace; sleep 0.8
wait_for "d['model']['location']" "$vp" 10 >/dev/null
vm wtype -M shift -k question -m shift >/dev/null 2>&1; sleep 0.8
expect "the key map is open" "d['keyMapOpen']" "True"
shot vp-keymap
key Escape; sleep 0.4
palette_run "Open the Trash"; sleep 1.0
expect "the Trash panel is open" "d['trashOpen']" "True"
shot vp-trash
key Escape; sleep 0.4
palette_run "Recent files"; sleep 1.0
expect "the Recent panel is open" "d['recentOpen']" "True"
shot vp-recent
key Escape; sleep 0.4

say "VP · Quick Look"
typ "plan"; sleep 1.2; key space; sleep 1.2
expect "Quick Look is open on text" "d['quickLookOpen']" "True"
expect "its close hint is inside the panel" "d['quickLookCloseInside']" "True"
shot vp-quicklook-text
key Escape; sleep 0.4
goto "$vp/Photos"; wait_for "d['model']['count']" "1" 10 >/dev/null
key Home; key space; sleep 1.8
expect "Quick Look is open on an image" "d['quickLookOpen']" "True"
expect "its close hint is inside the panel" "d['quickLookCloseInside']" "True"
expect "the status no longer names a file from the last folder" "d['hintShown'].startswith('Selected plan')" "False"
shot vp-quicklook-image
key Escape; sleep 0.4

say "VP · Settings"
ctrl comma; wait_for "d['settingsOpen']" "True" 8 >/dev/null
expect "Settings is open" "d['settingsOpen']" "True"
shot vp-settings
key Escape; sleep 0.4

say "VP · transfer card (running, then done)"
big="/var/tmp/gust-vp-big"; rm -rf "$big"; mkdir -p "$big/src" "$big/dest"
python3 -c "
for i in range(1500): open('$big/src/f%05d' % i, 'wb').write(b'x' * 64)
"
goto "$big"; wait_for "d['model']['count']" "2" 10 >/dev/null
typ "src"; sleep 1.2; ctrl c
typ "dest"; sleep 1.2; key Return
wait_for "d['model']['location']" "$big/dest" 10 >/dev/null
ctrl v
wait_for "d['jobRunning']" "True" 20 >/dev/null
sleep 0.8
expect "a copy is running" "d['jobRunning']" "True"
shot vp-transfer-running
wait_for "d['jobRunning']" "False" 300 >/dev/null
sleep 0.6
shot vp-transfer-done
rm -rf "$big"
stop_gust; rm -rf "$vp"; start_gust "$FIXTURE"
