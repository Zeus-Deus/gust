# shellcheck shell=bash
# Drawn file icons (folder / page + family + extension) and emblems, arrow keys after
# a click on empty space, wheel scrolling at the desktop's pace, and the
# toolbar's sort menu (DESIGN §6.21, §6.22).

look="$FIXTURE/look"
look_reset() {
  if mountpoint -q "$look/mounted" 2>/dev/null; then sudo -n umount "$look/mounted" 2>/dev/null; fi
  chmod -R u+rwx "$look" 2>/dev/null
  rm -rf "$look"
}
look_reset
mkdir -p "$look/plain" "$look/Documents" "$look/readonly" "$look/closed" "$look/mounted" "$look/target"
printf '{}\n' > "$look/skills-lock.json"
printf '#!/bin/sh\necho hi\n' > "$look/run.sh"; chmod +x "$look/run.sh"
printf 'notes\n' > "$look/notes.txt"
printf '# readme\n' > "$look/readme.md"
printf 'fn main(){}\n' > "$look/main.rs"
printf 'MIT\n' > "$look/LICENSE"
printf 'x' > "$look/archive.tar.gz"
: > "$look/empty"
ln -s target "$look/link-to-dir"
ln -s gone "$look/broken-link"
chmod 555 "$look/readonly"
chmod 000 "$look/closed"
MOUNTED=0
if sudo -n mount --bind "$look/target" "$look/mounted" 2>/dev/null; then MOUNTED=1; fi
# Documents is special only because user-dirs.dirs says so.
mkdir -p "$FIXTURE/.config"
printf 'XDG_DOCUMENTS_DIR="%s"\n' "$look/Documents" > "$FIXTURE/.config/user-dirs.dirs"
# look(name, i): field i of the drawn look (0 shape, 1 family, 2 label, 3 glyph).
look_of() { echo "next((r['look'].split(chr(9))[$2] for r in d['icons'] if r['name']=='$1'),'?')"; }

say "31a · rows draw folders and labelled pages in family colours"
stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
goto "$look" || bad "could not open $look"
palette_run "View: Rows"; sleep 1.5
palette_open >/dev/null; key Escape; sleep 0.8
expect "a folder is drawn as a folder"          "$(look_of plain 0)" "folder"
expect "Documents gets its glyph"               "$(look_of Documents 3)" "documents"
expect "a link to a folder looks like a folder" "$(look_of link-to-dir 0)" "folder"
expect "a JSON file is a data page"             "$(look_of skills-lock.json 1)" "data"
expect "its page says json"                     "$(look_of skills-lock.json 2)" "json"
expect "a Rust file is code"                    "$(look_of main.rs 1)" "code"
expect "a script is code"                       "$(look_of run.sh 1)" "code"
expect "Markdown is text"                       "$(look_of readme.md 1)" "text"
expect "LICENSE is a text page with no label"   "$(look_of LICENSE 1)+$(look_of LICENSE 2)" "text"
expect "a .tar.gz is an archive labelled tar"   "$(look_of archive.tar.gz 1)+$(look_of archive.tar.gz 2)" "archivetar"
expect "an empty file is a blank page"          "$(look_of empty 3)" "empty"
expect "a broken link is drawn as broken"       "$(look_of broken-link 0)" "broken"
expect "every row on screen draws its icon"     "all(r['shown'] for r in d['icons'])" "True"
expect "the Nerd Font glyphs are available"     "d['nerdGlyphs']" "True"
shot 31a-rows-icons

say "31b · emblems: read-only, no access, mount point, link, broken link"
expect_contains "a 555 folder is marked read-only"   "next((r['emblems'] for r in d['icons'] if r['name']=='readonly'),'')" "readonly"
expect_contains "a 000 folder is marked unreadable"  "next((r['emblems'] for r in d['icons'] if r['name']=='closed'),'')" "unreadable"
expect_contains "a symlink carries the link emblem"  "next((r['emblems'] for r in d['icons'] if r['name']=='link-to-dir'),'')" "link"
expect "a broken link carries the broken emblem"     "next((r['emblems'] for r in d['icons'] if r['name']=='broken-link'),'')" "broken-link"
expect "an ordinary folder has no emblem"            "next((r['emblems'] for r in d['icons'] if r['name']=='plain'),'?')" ""
if [[ "$MOUNTED" == 1 ]]; then
  expect_contains "a bind mount is marked as a mount" "next((r['emblems'] for r in d['icons'] if r['name']=='mounted'),'')" "mount"
else
  bad "could not bind-mount a fixture folder: the mount emblem case did NOT run"
fi

say "31c · Grid draws the same icons, with square badges on the corner"
palette_run "View: Grid"; sleep 1.5
palette_open >/dev/null; key Escape; sleep 0.8
expect "grid view"                                   "d['view']" "grid"
expect "every tile on screen draws its icon"          "all(r['shown'] for r in d['icons'])" "True"
shot 31c-grid-icons

say "31d · arrow keys still work after clicking empty space"
if [[ "$MOUSE_OK" == 1 ]]; then
  for v in Grid Rows; do
    palette_run "View: $v"; sleep 1.2
    read -r WX WY WW WH <<<"$(win_geo)"
    key Home
    c0="$(state "d['model']['cursor']")"
    # Bottom-right of the list: below and right of every item.
    mouse click $((WX + WW - 30)) $((WY + WH - 70)); sleep 0.6
    key Right; key Down; sleep 0.4
    c1="$(state "d['model']['cursor']")"
    [[ "$c1" =~ ^[0-9]+$ && "$c1" -gt "$c0" ]] \
      && ok "$v: arrows moved the cursor after an empty-space click ($c0 -> $c1)" \
      || bad "$v: arrows dead after an empty-space click (cursor $c0 -> $c1)"
    expect "$v: the window's key handler owns the keyboard" "d['focusOwner']" "listFocus"
  done
else
  bad "no pointer injection: the empty-space click case did NOT run"
fi

say "31e · one wheel notch scrolls about three rows, like the rest of the desktop"
if [[ "$MOUSE_OK" == 1 ]]; then
  big="$FIXTURE/wheel300"; rm -rf "$big"; mkdir -p "$big"
  for i in $(seq -w 1 300); do printf x > "$big/f$i.txt"; done
  goto "$big" || bad "could not open $big"
  wait_for "d['model']['count']" "300" 10 >/dev/null
  for v in Rows Grid; do
    palette_run "View: $v"; sleep 1.2
    read -r WX WY WW WH <<<"$(win_geo)"
    cx=$((WX + WW / 2 + 60)); cy=$((WY + WH / 2))
    key Home; sleep 0.4
    palette_open >/dev/null; key Escape; sleep 0.6
    s0="$(state "d['listScroll']")"
    # A high-resolution wheel: each notch arrives as eight small steps.
    mouse hiwheel "$cx" "$cy" -5; sleep 0.8
    palette_open >/dev/null; key Escape; sleep 0.6
    s1="$(state "d['listScroll']")"
    mouse wheel "$cx" "$cy" -5; sleep 0.8
    palette_open >/dev/null; key Escape; sleep 0.6
    s2="$(state "d['listScroll']")"
    rh="$(state "int(d['rowHeight'])")"
    hi=$((s1 - s0)); lo=$((s2 - s1))
    echo "  $v: 5 hi-res notches ${hi}px, 5 classic notches ${lo}px (row ${rh}px)"
    [[ "$hi" -ge $((rh * 8)) ]] && ok "$v: a high-resolution wheel scrolls (${hi}px for 5 notches)" \
      || bad "$v: a high-resolution wheel barely scrolls (${hi}px for 5 notches)"
    [[ "$lo" -ge $((rh * 8)) ]] && ok "$v: a classic wheel scrolls (${lo}px for 5 notches)" \
      || bad "$v: a classic wheel barely scrolls (${lo}px for 5 notches)"
    diff=$(( hi > lo ? hi - lo : lo - hi ))
    [[ "$diff" -le $(( (hi + lo) / 10 + 2 )) ]] && ok "$v: both wheels move the same distance per notch" \
      || bad "$v: the two wheels disagree (${hi} vs ${lo}px)"
    mouse wheel "$cx" "$cy" 30; sleep 0.8
    palette_open >/dev/null; key Escape; sleep 0.6
    expect "$v: scrolling up returns to the top" "d['listScroll']" "0"
  done
  rm -rf "$big"
else
  bad "no pointer injection: the wheel case did NOT run"
fi

say "31f · a sort button in the toolbar, in every view"
goto "$look"; sleep 0.8
palette_run "View: Grid"; sleep 1.2
expect_contains "the button says what the list is sorted by" "d['sortButtonText']" "Name"
if [[ "$MOUSE_OK" == 1 ]]; then
  read -r WX WY WW WH <<<"$(win_geo)"
  read -r BX BY <<<"$(state "' '.join(map(str, d['sortButtonPoint']))")"
  mouse click $((WX + BX)) $((WY + BY)); sleep 0.8
  expect "the sort menu opened" "d['menuOpen']" "True"
  expect_contains "it offers every order" "d['menuItems']" "Date modified"
  shot 31f-sort-menu
  read -r MX MY <<<"$(state "' '.join(map(str, d['menuPoints']['Size']))")"
  mouse click $((WX + MX)) $((WY + MY)); sleep 1.2
  expect "choosing Size sorts by size" "d['model']['sort_field']" "size"
  expect "the direction was kept" "d['model']['sort_ascending']" "True"
  mouse click $((WX + BX)) $((WY + BY)); sleep 0.8
  read -r MX MY <<<"$(state "' '.join(map(str, d['menuPoints']['Descending']))")"
  mouse click $((WX + MX)) $((WY + MY)); sleep 1.2
  expect "Descending flips only the direction" "d['model']['sort_ascending']" "False"
  expect "the field stayed Size" "d['model']['sort_field']" "size"
  expect_contains "the button shows it" "d['sortButtonText']" "Size ▼"
  expect "the keyboard is back on the list" "d['focusOwner']" "listFocus"
  shot 31f-sorted
  # Put it back so later sections see the default.
  palette_run "Sort by name"; sleep 0.6
  mouse click $((WX + BX)) $((WY + BY)); sleep 0.8
  read -r MX MY <<<"$(state "' '.join(map(str, d['menuPoints']['Ascending']))")"
  mouse click $((WX + MX)) $((WY + MY)); sleep 1
else
  bad "no pointer injection: the sort button case did NOT run"
fi

stop_gust
look_reset
rm -f "$FIXTURE/.config/user-dirs.dirs"
start_gust "$FIXTURE"
