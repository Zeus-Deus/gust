# shellcheck shell=bash
# Hashes in Properties: computed only on request (h), on a worker, stopped
# by closing; the digest must match sha256sum's.

say "21a · h in Quick Look shows the file's SHA-256"
printf 'gust hash check\n' > "$FIXTURE/hashme.txt"
want="$(sha256sum "$FIXTURE/hashme.txt" | cut -d' ' -f1)"
stop_gust; start_gust "$FIXTURE"
wait_for "d['model']['settled']" "True" 20 >/dev/null
typ "hashm"; sleep 1.2
expect "cursor on hashme.txt" "d['model']['cursor_name']" "hashme.txt"
key space; sleep 0.8
expect "Quick Look is open" "d['quickLookOpen']" "True"
expect "no hash until asked" "d['quickLookHash']" ""
typ "h"
wait_for "d['quickLookHash']" "$want" 10 >/dev/null
expect "the digest matches sha256sum" "d['quickLookHash']" "$want"
expect "h did not leave the folder" "d['model']['location']" "$FIXTURE"
shot 21a-hash
key Escape; sleep 0.5
expect "Escape closes Quick Look" "d['quickLookOpen']" "False"
expect "and the hash is dropped" "d['quickLookHash']" ""
rm -f "$FIXTURE/hashme.txt"
