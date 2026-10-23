#!/usr/bin/env bash
# Private, per-invocation skip accounting, shared by library and app tests.
set -euo pipefail
exec python3 "$(dirname "${BASH_SOURCE[0]}")/run-tests.py" "$@"
