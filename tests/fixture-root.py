#!/usr/bin/env python3
"""Admit a fresh fixture directory; never remove existing contents."""

import os
from pathlib import Path
import sys


def prepare(value):
    candidate = Path(value)
    if not candidate.is_absolute() or ".." in candidate.parts:
        raise ValueError("fixture must be an absolute temporary path")
    if not candidate.is_relative_to("/tmp") or candidate == Path("/tmp"):
        raise ValueError("fixture must be a child of /tmp")
    current = Path("/")
    for part in candidate.parts[1:]:
        current /= part
        if current.is_symlink():
            raise ValueError("fixture ancestry must not contain symlinks")
    if candidate.exists():
        if not candidate.is_dir() or any(candidate.iterdir()):
            raise ValueError("fixture must be new or empty; existing data is preserved")
        if candidate.stat().st_uid != os.getuid():
            raise ValueError("fixture must be owned by the current user")
    candidate.mkdir(mode=0o700, parents=True, exist_ok=True)
    candidate.chmod(0o700)
    return candidate


if __name__ == "__main__":
    try:
        print(prepare(sys.argv[1]))
    except (ValueError, OSError, IndexError):
        sys.exit("fixture refused: use a fresh owned directory under /tmp")
