# shellcheck shell=bash
# The root crumb supplies its own slash; separators belong between labels.

say "27a · the path bar shows one slash after the root"
stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
crumb_path="$(realpath -- "$FIXTURE")"
crumb_path="${crumb_path#/}"
crumb_separator=" / "
expect "path bar names the full fixture path" "d['breadcrumbText']" "/ ${crumb_path//\//"$crumb_separator"}"
goto / || bad "could not open /"
expect "at the root it is a single slash" "d['breadcrumbText']" "/"
stop_gust; start_gust "$FIXTURE"
