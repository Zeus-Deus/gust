# shellcheck shell=bash
# Per-folder sort: a sort chosen in one folder stays with that folder (and
# survives a restart); other folders keep sorting by name.

say "29a · a sort belongs to the folder it was chosen in"
d29="$FIXTURE/sortmem"
rm -rf "$d29"; mkdir -p "$d29/pics" "$d29/docs"
for f in b.txt a.pdf c.jpg; do printf x > "$d29/pics/$f"; done
for f in b.txt a.pdf c.jpg; do printf x > "$d29/docs/$f"; done
stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
goto "$d29/pics" || bad "could not open pics"
wait_for "d['model']['settled']" "True" 20 >/dev/null
palette_run "Sort by kind"; sleep 0.6
palette_run "Sort by kind"; sleep 0.8
expect "pics: kind, reversed" "d['model']['sort_field']+' '+str(d['model']['sort_ascending'])" "kind False"
goto "$d29/docs" || bad "could not open docs"
wait_for "d['model']['settled']" "True" 20 >/dev/null
expect "docs was never sorted: by name" "d['model']['sort_field']+' '+str(d['model']['sort_ascending'])" "name True"
expect "docs rows by name" "' '.join(d['model']['visible'])" "a.pdf b.txt c.jpg"
expect "Name header shows ascending" "d['headerTexts'][0]" "Name ▲"
goto "$d29/pics" || bad "could not reopen pics"
wait_for "d['model']['settled']" "True" 20 >/dev/null
expect "back in pics: its sort came back" "d['model']['sort_field']+' '+str(d['model']['sort_ascending'])" "kind False"
expect "pics rows by kind, reversed" "' '.join(d['model']['visible'])" "b.txt a.pdf c.jpg"
# Users read an up-pointing triangle as ascending; descending points down.
expect "Kind header shows descending" "d['headerTexts'][1]" "Kind ▼"
expect "Name header has no arrow" "d['headerTexts'][0]" "Name"
shot 29a-folder-sort

say "29c · a header click with the real pointer is remembered too"
if [[ "$MOUSE_OK" == 1 ]]; then
  goto "$d29/docs" || bad "could not open docs"
  wait_for "d['model']['settled']" "True" 20 >/dev/null
  read -r WX WY WW WH <<<"$(win_geo)"
  key Home
  pt="$(state "' '.join(map(str, d['nameHeaderPoint']))")"; read -r PX PY <<<"$pt"
  mouse click $((WX + PX)) $((WY + PY)); sleep 0.4
  # A header click writes no dump of its own; a cursor move does (Home
  # alone does not when the cursor is already on the first row).
  key Down; key Up
  wait_for "d['model']['sort_field']+' '+str(d['model']['sort_ascending'])" "name False" 5 >/dev/null \
    && ok "clicking the Name header reverses it here" || bad "the Name header click did not reverse the sort"
  goto "$d29/pics"; wait_for "d['model']['settled']" "True" 20 >/dev/null
  expect "pics kept its own sort" "d['model']['sort_field']+' '+str(d['model']['sort_ascending'])" "kind False"
  goto "$d29/docs"; wait_for "d['model']['settled']" "True" 20 >/dev/null
  expect "docs kept the clicked sort" "' '.join(d['model']['visible'])" "c.jpg b.txt a.pdf"
  expect "Name header shows descending after the click" "d['headerTexts'][0]" "Name ▼"
  shot 29c-header-click
  palette_run "Sort by name"; sleep 0.6
else
  bad "no pointer injection: the header-click case did NOT run"
fi

say "29b · and it survives a restart"
stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
expect "the start folder is still by name" "d['model']['sort_field']" "name"
goto "$d29/pics" || bad "could not open pics"
wait_for "d['model']['settled']" "True" 20 >/dev/null
expect "pics after a restart: kind, reversed" "d['model']['sort_field']+' '+str(d['model']['sort_ascending'])" "kind False"
m="$(stat -c %a "$FIXTURE/.config/gust/sorts.toml" 2>/dev/null)"
[[ "$m" == 600 ]] && ok "sorts.toml is owner-only" || bad "sorts.toml mode is '$m'"
palette_run "Sort by name"; sleep 0.6
stop_gust; rm -rf "$d29" "$FIXTURE/.config/gust/sorts.toml"; start_gust "$FIXTURE"
