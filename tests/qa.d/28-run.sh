# shellcheck shell=bash
# Run… : a program starts only from the menu/palette, after a prompt that
# names it, with Cancel focused. A file without +x is refused. The program
# runs in the default terminal (xdg-terminal-exec), never through a shell.

say "28a · Run… asks first; Enter cancels; Run runs it"
d28="$FIXTURE/run"
marker=/tmp/gust-fixture-RAN
rm -rf "$d28" "$marker"; mkdir -p "$d28"
printf '#!/bin/sh\ntouch %s\n' "$marker" > "$d28/hello.sh"; chmod 755 "$d28/hello.sh"
printf '#!/bin/sh\ntouch %s\n' "$marker" > "$d28/plain.sh"; chmod 644 "$d28/plain.sh"
# A stand-in terminal launcher: records its argv, runs what follows "--"
# directly, opens no window. A real terminal would take the keyboard and
# swallow the next section's keys; the argv Gust builds is what is tested.
stub="$d28/.bin"; mkdir -p "$stub"
cat > "$stub/xdg-terminal-exec" <<'STUB'
#!/bin/sh
printf '%s\n' "$@" > /tmp/gust-fixture-TERMARGV
while [ $# -gt 0 ] && [ "$1" != "--" ]; do shift; done
[ "$1" = "--" ] && shift
exec "$@"
STUB
chmod 755 "$stub/xdg-terminal-exec"
rm -f /tmp/gust-fixture-TERMARGV
export QA_PATH_PREPEND="$stub"
stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
goto "$d28" || bad "could not open $d28"
wait_for "d['model']['settled']" "True" 20 >/dev/null
typ "hello"; sleep 1.0
expect "cursor on hello.sh" "d['model']['cursor_name']" "hello.sh"
key Return; sleep 0.8
[[ ! -e "$marker" ]] && ok "Enter on a program runs nothing" || bad "Enter RAN a program"
palette_run "Run program"; sleep 0.8
expect "a confirmation opens" "d['confirmOpen']" "True"
shot 28a-run-confirm
hk="$(state "d['hintKeys']")"
[[ "$hk" == *"Esc cancel"* && "$hk" != *"Enter open"* ]] \
  && ok "the hint strip shows the dialog's keys ($hk)" || bad "the hint strip shows '$hk' under a dialog"
key Return; sleep 0.8
expect "Enter cancels (Cancel has focus)" "d['confirmOpen']" "False"
[[ ! -e "$marker" ]] && ok "cancelled: nothing ran" || bad "a cancelled Run ran the program"
palette_run "Run program"; sleep 0.8
key Tab; key Return; sleep 3.0
[[ -e "$marker" ]] && ok "confirmed Run started the program" || bad "confirmed Run did not start it"
argv="$(tr '\n' ' ' < /tmp/gust-fixture-TERMARGV 2>/dev/null)"
[[ "$argv" == "--hold --dir=$d28 -- $d28/hello.sh " ]] \
  && ok "it went to the terminal launcher as argv: $argv" || bad "terminal launcher got '$argv'"
[[ "$(vm hyprctl activewindow -j | python3 -c 'import json,sys;print(json.load(sys.stdin).get("class",""))')" == gust ]] \
  && ok "Gust still has the keyboard" || bad "another window took the keyboard"

say "28b · a file without the x bit is refused, not guessed"
rm -f "$marker"
hyprctl dispatch 'hl.dsp.focuswindow({ "class:gust" })' >/dev/null 2>&1; sleep 0.4
typ "plain"; sleep 1.0
expect "cursor on plain.sh" "d['model']['cursor_name']" "plain.sh"
palette_run "Run program"; sleep 0.8
expect "no confirmation for a non-executable" "d['confirmOpen']" "False"
expect_contains "and it says why" "d['alertText']" "not executable"
[[ ! -e "$marker" ]] && ok "nothing ran" || bad "a non-executable file was run"
key Escape
unset QA_PATH_PREPEND
stop_gust; rm -rf "$d28" "$marker" /tmp/gust-fixture-TERMARGV; start_gust "$FIXTURE"
