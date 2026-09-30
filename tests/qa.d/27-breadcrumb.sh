# shellcheck shell=bash
# The path bar reads "/ tmp / gust-fixture", never "/ / tmp": the root crumb
# is itself a "/", so no separator follows it.

say "27a · the path bar shows one slash after the root"
stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
bc="$(state "d['breadcrumbText']")"
[[ "$bc" == "/ tmp / gust-fixture" ]] && ok "path bar reads '$bc'" || bad "path bar reads '$bc'"
goto / || bad "could not open /"
expect "at the root it is a single slash" "d['breadcrumbText']" "/"
stop_gust; start_gust "$FIXTURE"
