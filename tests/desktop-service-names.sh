#!/usr/bin/env bash
# Exercise both desktop service entry points without contacting the user's bus.
set -euo pipefail
binary=$(realpath -e "${1:?usage: desktop-service-names.sh GUST_BINARY}")
dbus-run-session -- bash -eu -c '
    check_name() (
        "$1" "$2" &
        owner=$!
        trap '\''kill "$owner" 2>/dev/null || true; wait "$owner" 2>/dev/null || true'\'' EXIT
        gdbus wait --session --timeout 5 "$3"
        result=0
        timeout 5 "$1" "$2" || result=$?
        [[ $result == 1 ]] || { echo "$2 duplicate exit $result (expected 1)" >&2; exit 1; }
        kill -0 "$owner"
        gdbus wait --session --timeout 2 "$3"
        echo "PASS $2 refuses a second owner"
    )
    check_name "$1" --dbus-service org.freedesktop.FileManager1
    check_name "$1" --portal org.freedesktop.impl.portal.desktop.gust
' service-test "$binary"
