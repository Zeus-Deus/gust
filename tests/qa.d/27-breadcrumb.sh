# shellcheck shell=bash
# Inside the home folder the path bar starts at "Home" ("Home / Documents"),
# not at "/ tmp / gust-fixture". Outside it reads "/ tmp", never "/ / tmp":
# the root crumb is itself a "/", so no separator follows it.

say "27a · the path bar starts at Home, and shows one slash after the root"
stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
bc="$(state "d['breadcrumbText']")"
[[ "$bc" == "Home" ]] && ok "path bar reads '$bc'" || bad "path bar reads '$bc'"
goto "$FIXTURE/Documents" || bad "could not open Documents"
expect "inside home it starts at Home" "d['breadcrumbText']" "Home / Documents"
goto "$(dirname "$FIXTURE")" || bad "could not open the folder above home"
bc="$(state "d['breadcrumbText']")"
[[ "$bc" == /\ * && "$bc" != "/ / "* ]] && ok "above home it reads '$bc'" || bad "above home it reads '$bc'"
goto / || bad "could not open /"
expect "at the root it is a single slash" "d['breadcrumbText']" "/"
stop_gust; start_gust "$FIXTURE"
