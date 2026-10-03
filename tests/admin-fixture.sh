#!/usr/bin/env bash
# QA-only polkit rule for "Open as administrator" (qa §30h), for a DISPOSABLE
# test machine: lets the current user start exactly this checkout's debug and
# release Gust through pkexec without a password prompt. Nothing else.
#   tests/admin-fixture.sh start   → installs /etc/polkit-1/rules.d/50-gust-qa-pkexec.rules
#   tests/admin-fixture.sh stop    → removes it
# Never run this on a machine you use: it is a root-without-password rule
# for two program paths. Needs sudo -n; refuses cleanly without it.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
RULE=/etc/polkit-1/rules.d/50-gust-qa-pkexec.rules
start() {
  sudo -n true 2>/dev/null || { echo "installing a polkit rule needs root; sudo -n is unavailable" >&2; return 1; }
  local user tmp
  user="$(id -un)"
  tmp="$(mktemp)"
  cat > "$tmp" <<EOF
// QA fixture only (disposable VM), from tests/admin-fixture.sh.
polkit.addRule(function(action, subject) {
    if (action.id == "org.freedesktop.policykit.exec" && subject.user == "$user") {
        var p = action.lookup("program");
        if (p == "$REPO/target/debug/gust" || p == "$REPO/target/release/gust") {
            return polkit.Result.YES;
        }
    }
});
EOF
  sudo -n install -m 644 "$tmp" "$RULE"
  rm -f "$tmp"
  echo "installed $RULE for $user"
}
stop() {
  sudo -n rm -f "$RULE" && echo "removed $RULE"
}
"${1:-start}"
