# shellcheck shell=bash
# Also runnable with tests/qa-one.sh tests/qa.d/50-fixture-navigation.sh.
# Replay the full runner's startup and Enter/Backspace checks, then use its
# Ctrl+L path helper. The look-alike directory must never supply the listing.
check_newline_fixture() {
  local FIXTURE="$qa_run_dir/"$'home\n'
  local lookalike="$qa_run_dir/home<U+000A>"
  local OUTDIR="$OUTDIR/newline-navigation"
  local LOG="$OUTDIR/gust.log" STATE="$OUTDIR/qa-state.json"
  mkdir -p "$OUTDIR" "$lookalike/Documents" || return 1
  touch "$lookalike/Documents/wrong-folder" || return 1
  "$HERE/make-fixture.sh" "$FIXTURE" >/dev/null || return 1
  source "$HERE/qa-startup.sh" || return 1
  goto "$FIXTURE/Documents" || return 1
  expect "Documents displays the escaped HOME" "d['model']['location']" "$lookalike/Documents"
  expect "Documents lists the original fixture" "sorted(d['model']['visible'])" "['contract.pdf', 'quarterly-plan.md', 'todo.txt']"
  shot 50-documents
  goto "$FIXTURE" || return 1
  expect "Ctrl+L returns to the fixture" "d['breadcrumbText']" "Home"
}

say "50 · navigate a fixture whose name ends in a newline"
check_newline_fixture || return 1
start_gust "$FIXTURE"
