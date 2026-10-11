# shellcheck shell=bash
# A single F2 rename has a safe, retryable inverse, just like a transfer.
say "39 · single-file rename Undo preserves changed or occupied files"
stop_gust; start_gust "$FIXTURE"
REN="$FIXTURE/rename-undo"
mkdir -p "$REN"
printf 'original rename bytes\n' > "$REN/from.txt"
goto "$REN"
wait_for "d['model']['settled']" "True" 15 >/dev/null
key F2; ctrl a; typ "to.txt"; key Return
wait_for "d['model']['cursor_name']" "to.txt" 10 >/dev/null
ctrl z
wait_for "d['model']['cursor_name']" "from.txt" 5 >/dev/null
expect "rename Undo names the operation correctly" "d['hintStatus']" "Restored the original name"
[[ -f "$REN/from.txt" && ! -e "$REN/to.txt" ]] \
  && ok "Ctrl+Z puts the original name back" || bad "a single rename could not be undone"
[[ -f "$REN/from.txt" ]] && cmp -s "$REN/from.txt" <(printf 'original rename bytes\n') \
  && ok "rename Undo keeps the file contents" || bad "rename Undo did not preserve the bytes"
shot 39-rename-undo

# Start afresh, so a previous failed assertion cannot masquerade as the guard.
printf 'original occupied bytes\n' > "$REN/occupied-from.txt"
goto "$REN"; ctrl r; sleep 0.8
typ "occupied-from.txt"; key F2; ctrl a; typ "occupied-to.txt"; key Return
wait_for "d['model']['cursor_name']" "occupied-to.txt" 10 >/dev/null
printf 'new original name belongs to somebody else\n' > "$REN/occupied-from.txt"
ctrl z
cmp -s "$REN/occupied-from.txt" <(printf 'new original name belongs to somebody else\n') \
  && ok "an occupied original name is never replaced" || bad "Undo replaced new work at the original name"
cmp -s "$REN/occupied-to.txt" <(printf 'original occupied bytes\n') \
  && ok "a refused Undo keeps the renamed file" || bad "refused Undo lost the renamed file"
rm -f "$REN/occupied-from.txt"
ctrl z
[[ -f "$REN/occupied-from.txt" && ! -e "$REN/occupied-to.txt" ]] \
  && ok "a refused rename Undo stays available to retry" || bad "the refusal forgot the rename Undo"

printf 'before editing\n' > "$REN/edited-from.txt"
ctrl r; sleep 0.8; typ "edited-from.txt"
key F2; ctrl a; typ "edited-to.txt"; key Return
wait_for "d['model']['cursor_name']" "edited-to.txt" 10 >/dev/null
printf 'edited after the rename, must survive\n' > "$REN/edited-to.txt"
ctrl z
[[ ! -e "$REN/edited-from.txt" ]] && cmp -s "$REN/edited-to.txt" <(printf 'edited after the rename, must survive\n') \
  && ok "Undo refuses a changed target without moving or losing its bytes" \
  || bad "Undo moved or lost a changed rename target"
shot 39-rename-refused

say "39b · no-op/failed rename keeps prior Undo; successful rename supersedes it"
for flow in noop failed supersede; do
  stop_gust; start_gust "$FIXTURE"
  P="$REN/prior-$flow"
  mkdir -p "$P"
  printf 'original preceding-copy bytes\n' > "$P/item.txt"
  [[ "$flow" == failed ]] && printf 'unrelated occupied name\n' > "$P/taken.txt"
  goto "$P"
  ctrl d
  wait_for "'copied' in d['undoLabel']" "True" 20 >/dev/null
  expect_contains "the preceding copy offers Undo" "d['undoLabel']" "copied"
  prior_label="$(state "d['undoLabel']")"
  typ "item.txt"
  key F2; ctrl a
  case "$flow" in
    noop) typ "item.txt"; key Return ;;
    failed) typ "taken.txt"; key Return; expect_contains "the clash is refused" "d['promptError']" "exists"; key Escape ;;
    supersede) typ "new-item.txt"; key Return ;;
  esac
  if [[ "$flow" == supersede ]]; then
    expect "successful rename owns the next Undo" "d['undoLabel']" "Undo: restore the original name"
    ctrl z
    cmp -s "$P/item.txt" <(printf 'original preceding-copy bytes\n') \
      && cmp -s "$P/item (copy).txt" <(printf 'original preceding-copy bytes\n') \
      && [[ ! -e "$P/new-item.txt" ]] \
      && ok "rename Undo restored the name without undoing the older copy" || bad "rename Undo acted on older work"
    ctrl z
    cmp -s "$P/item (copy).txt" <(printf 'original preceding-copy bytes\n') \
      && ok "consuming rename Undo does not expose the old copy again" || bad "a second Undo removed unrelated older work"
  else
    expect "$flow rename retains the exact preceding Undo offer" "d['undoLabel']" "$prior_label"
    ctrl z
    cmp -s "$P/item.txt" <(printf 'original preceding-copy bytes\n') && [[ ! -e "$P/item (copy).txt" ]] \
      && ok "$flow rename left the preceding copy undoable" || bad "$flow rename lost or replaced prior Undo"
    if [[ "$flow" == failed ]]; then
      cmp -s "$P/taken.txt" <(printf 'unrelated occupied name\n') \
        && ok "the failed rename never touched the occupied name" || bad "the failed rename damaged a bystander"
    fi
  fi
done
stop_gust; start_gust "$FIXTURE"
