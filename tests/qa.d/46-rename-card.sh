# shellcheck shell=bash
# The rename card: F2 on several files opens every name as a line. Find /
# Replace across them, a bad line is marked where it is and blocks Rename,
# Enter renames, one Ctrl+Z puts every name back (DESIGN §6.16).
say "46 · rename card: edit many names at once"
stop_gust
RN="$FIXTURE/rename-card"
rm -rf "$RN"; mkdir -p "$RN"
for i in 1 2 3; do printf 'photo %s\n' "$i" > "$RN/IMG_000$i.JPG"; done
printf 'keep\n' > "$RN/notes.txt"
printf 'bystander\n' > "$RN/taken.JPG"
BEFORE="$(cd "$RN" && find . -type f -exec sha256sum {} + | sort)"

start_gust "$RN"
wait_for "d['model']['settled']" "True" 15 >/dev/null
typ "IMG_0001"; sleep 1.2
# Shift+Down from the row the cursor is on: both rows, not just the next.
vm wtype -M shift -k Down -m shift >/dev/null 2>&1; sleep 0.4
vm wtype -M shift -k Down -m shift >/dev/null 2>&1; sleep 0.4
expect "Shift+Down twice from a row selects three rows" "d['selCount']" "3"
key F2
wait_for "d['renameCardOpen']" "True" 10 >/dev/null
expect "F2 on several files opens the rename card" "d['renameCardOpen']" "True"
expect_contains "nothing changes until you edit" "d['renameCardSummary']" "No names would change"

# Tab to Find, type, Tab to Replace, type, Enter = Replace all.
key Tab; typ "IMG_"; key Tab; typ "beach-"; key Return; sleep 0.6
expect_contains "Replace all shows what will happen" "d['renameCardSummary']" "3 files will be renamed"
expect "nothing on disk changed yet" "str(__import__('os').path.exists('$RN/IMG_0001.JPG'))" "True"
shot 46-rename-card

# Back to the list (Tab from Replace), make the first line collide with a
# bystander file: that line is marked and Rename is refused.
key Tab; key Home
vm wtype -M shift -k End -m shift >/dev/null 2>&1; sleep 0.3
typ "taken.JPG"; sleep 0.6
expect "a name that already exists is marked" "d['renameCardBad']" "1"
expect "and Rename is refused" "d['renameCardOk']" "False"
key Return; sleep 0.6
expect "Enter on a bad list does not rename" "d['renameCardOpen']" "True"
cmp -s "$RN/taken.JPG" <(printf 'bystander\n') && ok "the bystander is untouched" || bad "taken.JPG changed"
shot 46-rename-card-bad
vm wtype -M shift -k Home -m shift >/dev/null 2>&1; sleep 0.3
typ "beach-0001.JPG"; sleep 0.6
expect "fixing the line clears the mark" "d['renameCardBad']" "0"
key Return
wait_for "d['renameCardOpen']" "False" 10 >/dev/null
sleep 1
[[ -f "$RN/beach-0001.JPG" && -f "$RN/beach-0002.JPG" && -f "$RN/beach-0003.JPG" && ! -e "$RN/IMG_0001.JPG" ]] \
  && ok "Enter renamed every file to its line" || bad "rename result wrong: $(ls "$RN" | tr '\n' ' ')"
cmp -s "$RN/beach-0002.JPG" <(printf 'photo 2\n') && ok "each file kept its own contents" || bad "contents moved between files"

ctrl z; sleep 1.5
AFTER="$(cd "$RN" && find . -type f -exec sha256sum {} + | sort)"
[[ "$AFTER" == "$BEFORE" ]] && ok "one Ctrl+Z puts every name back" || bad "Undo left: $(ls "$RN" | tr '\n' ' ')"
expect "Undo says what it did" "d['hintStatus']" "Names put back"

# Escape cancels without touching anything; the mouse Cancel does too.
typ "IMG_0001"; sleep 1.2
vm wtype -M shift -k Down -m shift >/dev/null 2>&1; sleep 0.4
key F2
wait_for "d['renameCardOpen']" "True" 10 >/dev/null
key Escape; sleep 0.5
expect "Esc closes the card" "d['renameCardOpen']" "False"
if [[ "$MOUSE_OK" -eq 1 ]]; then
  key F2
  wait_for "d['renameCardOpen']" "True" 10 >/dev/null
  read -r WX WY WW WH <<<"$(win_geo)"
  read -r cx cy <<<"$(state "' '.join(map(str, d['renameCardPoints']['cancel']))")"
  mouse click $((WX + cx)) $((WY + cy))
  sleep 0.5
  expect "clicking Cancel closes the card" "d['renameCardOpen']" "False"
fi
AFTER="$(cd "$RN" && find . -type f -exec sha256sum {} + | sort)"
[[ "$AFTER" == "$BEFORE" ]] && ok "cancelling changed nothing" || bad "cancel changed files"
stop_gust
start_gust "$FIXTURE"
