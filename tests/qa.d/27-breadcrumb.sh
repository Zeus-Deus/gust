# shellcheck shell=bash
# The fixture is Gust's home. Above it, crumbs use escaped display names.

say "27a · the path bar starts at Home, and shows one slash after the root"
stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
expect "at home the path bar reads Home" "d['breadcrumbText']" "Home"
crumb_home="$(state "d['model']['location']")"
crumb_parent="${crumb_home%/*}"
key BackSpace
wait_for "d['model']['location']" "$crumb_parent" || bad "could not open the folder above home"
crumb_path="${crumb_parent#/}"
crumb_separator=" / "
expect "above home it shows the full parent path" "d['breadcrumbText']" "/ ${crumb_path//\//"$crumb_separator"}"
goto '~/Documents' "$crumb_home/Documents" || bad "could not open Documents"
expect "inside home it starts at Home" "d['breadcrumbText']" "Home / Documents"

# A known label checks the escaping itself, independently of the model location.
crumb_bidi="$qa_run_dir/"$'left\xE2\x80\xAEright'
mkdir -- "$crumb_bidi" || return 1
goto "$crumb_bidi" "$qa_run_dir/left<U+202E>right" || bad "could not open the bidi folder"
expect "a bidi mark is visible in the label" "d['breadcrumbText']" "/ tmp / ${qa_run_dir##*/} / left<U+202E>right"
goto / || bad "could not open /"
expect "at the root it is a single slash" "d['breadcrumbText']" "/"
stop_gust; start_gust "$FIXTURE"
