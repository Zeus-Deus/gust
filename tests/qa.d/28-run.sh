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
before="$(vm hyprctl clients -j | python3 -c 'import json,sys;print(" ".join(c["address"] for c in json.load(sys.stdin)))')"
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
# Close the terminal it opened so later cases have a clear desktop.
vm hyprctl clients -j | python3 -c '
import json,sys
before=set(sys.argv[1].split())
for c in json.load(sys.stdin):
    if c["address"] not in before and c.get("class")!="gust": print(c["address"])' "$before" |
  while read -r a; do vm hyprctl dispatch "hl.dsp.window.close({ window = \"address:$a\" })" >/dev/null 2>&1; done
sleep 0.8

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
stop_gust; rm -rf "$d28" "$marker"; start_gust "$FIXTURE"
