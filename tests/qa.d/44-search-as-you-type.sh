# shellcheck shell=bash
# Search as you type (Ctrl+F): ranked results while typing, Esc back to the
# folder AND the row you left, Enter keeps the results, build output skipped.
say "44 · search as you type"
stop_gust
SRCH="$FIXTURE/search-here"
rm -rf "$SRCH"
mkdir -p "$SRCH/Work/field/a" "$SRCH/Docs" "$SRCH/target/release" "$SRCH/node_modules/bench"
touch "$SRCH/Work/field/bench.md" "$SRCH/Work/field/bench-notes.md" \
      "$SRCH/Work/field/a/bench-2026.md" "$SRCH/Docs/field-bench-notes.md" \
      "$SRCH/bench.txt" "$SRCH/target/release/bench.o" "$SRCH/node_modules/bench/bench.js"
printf 'Signature: 8a477f597d28d172789f06886806bc55\n' > "$SRCH/target/CACHEDIR.TAG"
start_gust "$SRCH"
wait_for "d['model']['settled']" "True" 15 >/dev/null

# Put the cursor on a row that is not the first, so "back to the row you
# left" cannot pass by accident.
typ "Work"; sleep 1.2
expect "cursor starts on Work" "d['model']['cursor_name']" "Work"

ctrl f
wait_for "d['searchBarActive']" "True" 5 >/dev/null
expect "Ctrl+F opens the search field" "d['searchBarActive']" "True"
typ "bench"
wait_for "d['finding']" "True" 5 >/dev/null
wait_for "d['hintStatus'].split(' ')[0]" "5" 5 >/dev/null
expect_contains "results arrive while typing, without Enter" "d['hintStatus']" "5 results"
expect "the best match is first" "d['model']['cursor_name']" "bench.txt"
expect "build output and node_modules are not searched" \
  "any(n.startswith(('target','node_modules')) for n in d.get('rowPoints',{}))" "False"
shot 44-search-typing

key Escape
wait_for "d['finding']" "False" 5 >/dev/null
wait_for "d['model']['cursor_name']" "Work" 5 >/dev/null
expect "Esc leaves the results" "d['finding']" "False"
expect "Esc closes the field" "d['searchBarActive']" "False"
expect "Esc returns to the row you were on" "d['model']['cursor_name']" "Work"
expect "Esc returns to the folder you were in" "d['model']['location']" "$SRCH"

ctrl f
wait_for "d['searchBarActive']" "True" 5 >/dev/null
typ "notes"
wait_for "d['finding']" "True" 5 >/dev/null
key Return
wait_for "d['searchBarActive']" "False" 5 >/dev/null
expect "Enter keeps the results" "d['finding']" "True"
key Down
expect_contains "the keys move through the results" "d['model']['cursor_name']" "notes"
shot 44-search-kept
key Escape
wait_for "d['finding']" "False" 5 >/dev/null
expect "Esc from kept results returns to the row you left" "d['model']['cursor_name']" "Work"

# The two typed in quick succession: the results are for the final text,
# never a slower earlier search that finished last.
ctrl f
wait_for "d['searchBarActive']" "True" 5 >/dev/null
typ "b"; typ "ench-2026"
wait_for "d['hintStatus'].split(' ')[0]" "1" 5 >/dev/null
expect_contains "results match the final text" "d['hintStatus']" "\"bench-2026\""
key Escape
wait_for "d['finding']" "False" 5 >/dev/null
stop_gust
start_gust "$FIXTURE"
