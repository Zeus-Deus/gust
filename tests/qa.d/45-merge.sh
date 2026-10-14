# shellcheck shell=bash
# Merge a folder into a folder of the same name, by keyboard AND by mouse,
# then one Ctrl+Z puts both folders back exactly (DESIGN §6.24).
say "45 · merge folders, and Undo the merge"
stop_gust
MRG="$FIXTURE/merge"
rm -rf "$MRG"
mkdir -p "$MRG/src/Invoices/2026" "$MRG/dst/Invoices"
printf 'NEW JAN\n'   > "$MRG/src/Invoices/jan.txt"
printf 'OLD JAN\n'   > "$MRG/dst/Invoices/jan.txt"
printf 'same feb\n'  > "$MRG/src/Invoices/feb.txt"
printf 'same feb\n'  > "$MRG/dst/Invoices/feb.txt"
printf 'mar\n'       > "$MRG/src/Invoices/mar.txt"
printf 'q1\n'        > "$MRG/src/Invoices/2026/q1.txt"
printf 'may\n'       > "$MRG/dst/Invoices/may.txt"
snap() { (cd "$1" && find . -printf '%p %s\n' | sort && find . -type f -exec sha256sum {} + | sort); }
SRC_BEFORE="$(snap "$MRG/src")"; DST_BEFORE="$(snap "$MRG/dst")"

start_gust "$MRG/src"
wait_for "d['model']['settled']" "True" 15 >/dev/null
typ "Invoices"; sleep 1.2
ctrl c
goto "$MRG/dst"
wait_for "d['model']['location']" "$MRG/dst" 10 >/dev/null
ctrl v
wait_for "d['conflictOpen']" "True" 15 >/dev/null
expect "pasting a folder onto its twin asks first" "d['conflictOpen']" "True"
expect "Merge is offered for a folder onto a folder" "d['conflictCanMerge']" "True"
expect "Merge has the keyboard" "d['conflictFocus']" "merge"
shot 45-merge-dialog
key Return
wait_for "d['conflictOpen']" "False" 10 >/dev/null
wait_for "d['jobState']" "Confirmed" 20 >/dev/null
expect_contains "the card says it merged" "d['hintStatus']" "merged"
cmp -s "$MRG/dst/Invoices/jan.txt" <(printf 'OLD JAN\n') \
  && ok "nothing at the destination was overwritten" || bad "Merge overwrote jan.txt"
cmp -s "$MRG/dst/Invoices/jan (copy).txt" <(printf 'NEW JAN\n') \
  && ok "a different file with a taken name lands beside it" || bad "jan (copy).txt missing"
[[ -f "$MRG/dst/Invoices/mar.txt" && -f "$MRG/dst/Invoices/2026/q1.txt" && -f "$MRG/dst/Invoices/may.txt" ]] \
  && ok "missing files and folders arrived; the destination's own stayed" || bad "merge lost or missed a file"
[[ ! -e "$MRG/dst/Invoices/feb (copy).txt" ]] \
  && ok "an identical file is not duplicated" || bad "identical feb.txt was copied again"
shot 45-merge-done

ctrl z
sleep 2
[[ "$(snap "$MRG/dst")" == "$DST_BEFORE" ]] \
  && ok "one Ctrl+Z restores the destination exactly" || bad "Undo left the destination different"
[[ "$(snap "$MRG/src")" == "$SRC_BEFORE" ]] \
  && ok "the source is untouched by a copy and its Undo" || bad "the source changed"

# The same journey with the mouse: click Merge.
if [[ "$MOUSE_OK" -eq 1 ]]; then
  goto "$MRG/src"
  wait_for "d['model']['location']" "$MRG/src" 10 >/dev/null
  typ "Invoices"; sleep 1.2
  ctrl x
  goto "$MRG/dst"
  wait_for "d['model']['location']" "$MRG/dst" 10 >/dev/null
  ctrl v
  wait_for "d['conflictOpen']" "True" 15 >/dev/null
  read -r WX WY WW WH <<<"$(win_geo)"
  read -r mx my <<<"$(state "' '.join(map(str, d['conflictPoints']['merge']))")"
  mouse click $((WX + mx)) $((WY + my))
  wait_for "d['conflictOpen']" "False" 10 >/dev/null
  wait_for "d['jobState']" "Confirmed" 20 >/dev/null
  cmp -s "$MRG/src/Invoices/feb.txt" <(printf 'same feb\n') \
    && ok "a merging move leaves an already-there file in the source" || bad "feb.txt vanished from the source"
  [[ ! -e "$MRG/src/Invoices/mar.txt" && -f "$MRG/dst/Invoices/mar.txt" ]] \
    && ok "a merging move moves what was missing" || bad "mar.txt did not move"
  ctrl z
  sleep 2
  [[ "$(snap "$MRG/dst")" == "$DST_BEFORE" && "$(snap "$MRG/src")" == "$SRC_BEFORE" ]] \
    && ok "Undo of a merging move puts both folders back exactly" || bad "Undo of the move left a difference"
else
  bad "Merge mouse journey did NOT run: no pointer injection"
fi
stop_gust
start_gust "$FIXTURE"
