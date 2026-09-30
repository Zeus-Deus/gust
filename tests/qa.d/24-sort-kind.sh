# shellcheck shell=bash
# Sort by kind: a Kind column shows each file's extension, and clicking its
# header (or the palette) orders by it (Flea #190).

say "24a · Sort by kind orders by extension, folders first"
d24="$FIXTURE/kinds"
rm -rf "$d24"; mkdir -p "$d24/zfolder"
for f in b.txt a.pdf c.jpg a.txt z.jpg noext; do printf x > "$d24/$f"; done
stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
goto "$d24" || bad "could not open $d24"
wait_for "d['model']['settled']" "True" 20 >/dev/null
palette_run "Sort by kind"; sleep 0.8
expect "the sort field is kind" "d['model']['sort_field']" "kind"
expect "folders first, then by extension, then name" \
  "' '.join(d['model']['visible'])" "zfolder noext c.jpg z.jpg a.pdf a.txt b.txt"
shot 24a-sort-kind
palette_run "Sort by kind"; sleep 0.8
expect "choosing it again reverses it" "d['model']['sort_ascending']" "False"
palette_run "Sort by name"; sleep 0.6
stop_gust; rm -rf "$d24"; start_gust "$FIXTURE"
