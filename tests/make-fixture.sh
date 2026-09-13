#!/usr/bin/env bash
# Build a self-contained fixture tree for Gust QA.
# NOTHING here touches the user's real files: everything lives under the
# directory passed as $1 (default: a private dir under /tmp).
set -euo pipefail

ROOT="${1:-/tmp/gust-fixture}"
rm -rf "$ROOT"
mkdir -p "$ROOT"

cd "$ROOT"

# --- an ordinary-looking project folder -----------------------------------
mkdir -p Projects/omarchy/{bin,config,themes}
printf 'Gust fixture readme\n' > Projects/omarchy/README.md
printf '# agents\n' > Projects/omarchy/AGENTS.md
head -c 1800 /dev/urandom > Projects/omarchy/PKGBUILD
head -c 9400 /dev/urandom > Projects/omarchy/logo.svg
head -c 240000 /dev/urandom > Projects/omarchy/demo.mp4
for i in $(seq 1 12); do printf 'file %s\n' "$i" > "Projects/omarchy/bin/tool-$i"; done

mkdir -p Downloads Pictures Documents Videos Music
printf 'quarterly plan\n' > Documents/quarterly-plan.md
printf 'todo\n' > Documents/todo.txt
head -c 4096 /dev/urandom > Documents/contract.pdf

# --- natural sort proof: file9 must precede file10 ------------------------
mkdir -p Downloads/sorting
for i in 1 2 9 10 11 100; do printf 'x' > "Downloads/sorting/file$i.txt"; done
for i in 06 007 8; do printf 'x' > "Downloads/sorting/track$i"; done

# --- hidden files ---------------------------------------------------------
printf 'hidden\n' > .hidden-config
mkdir -p .hidden-dir

# --- symlinks: good and dangling -----------------------------------------
mkdir -p links
printf 'target content\n' > links/real-target.txt
ln -s real-target.txt links/good-link
ln -s /definitely/does/not/exist links/dangling-link
ln -s ../Projects links/dir-link

# --- awkward but legal names ---------------------------------------------
mkdir -p edge
printf 'x' > "edge/with space.txt"
printf 'x' > "edge/with'quote.txt"
printf 'x' > 'edge/with"doublequote.txt'
printf 'x' > "edge/with#hash.txt"
printf 'x' > "edge/üñïçødé-ñâmé.txt"
printf 'x' > "edge/$(printf 'newline\nin-name')" 2>/dev/null || true
# Non-UTF-8 byte in a filename (must not crash the lister).
python3 - "$ROOT" <<'PY'
import os, sys
root = sys.argv[1]
p = os.path.join(root.encode(), b"edge", b"non-utf8-\xff\xfe.txt")
with open(p, "wb") as f:
    f.write(b"x")
PY
# Bidi-override spoof name.
printf 'x' > "edge/photo$(printf '\xe2\x80\xae')gpj.exe"

# --- an executable that must NOT auto-run --------------------------------
mkdir -p danger
cat > danger/pretend-script.sh <<'EOS'
#!/bin/sh
echo "THIS MUST NEVER RUN FROM BROWSING" > /tmp/gust-fixture-EXECUTED
EOS
chmod +x danger/pretend-script.sh

# --- a big directory for listing performance -----------------------------
mkdir -p big
python3 - "$ROOT" <<'PY'
import os, sys
d = os.path.join(sys.argv[1], "big")
for i in range(5000):
    open(os.path.join(d, f"entry-{i:05d}.dat"), "wb").close()
PY

# --- a private trash for destructive tests -------------------------------
mkdir -p Trash/files Trash/info

echo "fixture ready at $ROOT"
find "$ROOT" -maxdepth 1 -mindepth 1 | sort
