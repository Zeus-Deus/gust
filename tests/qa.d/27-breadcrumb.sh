# shellcheck shell=bash
# The fixture is Gust's home. Above it, each component uses its filesystem name.

say "27a · the path bar starts at Home, and shows one slash after the root"
stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
expect "at home the path bar reads Home" "d['breadcrumbText']" "Home"
goto "$FIXTURE/Documents" || bad "could not open Documents"
expect "inside home it starts at Home" "d['breadcrumbText']" "Home / Documents"

crumb_parent="$(dirname -- "$(realpath -- "$FIXTURE")")"
goto "$crumb_parent" || bad "could not open the folder above home"
crumb_path="${crumb_parent#/}"
crumb_separator=" / "
expect "above home it shows the full parent path" "d['breadcrumbText']" "/ ${crumb_path//\//"$crumb_separator"}"
goto / || bad "could not open /"
expect "at the root it is a single slash" "d['breadcrumbText']" "/"
stop_gust; start_gust "$FIXTURE"
