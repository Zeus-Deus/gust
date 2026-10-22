#!/usr/bin/env python3
"""Select and record Qt without changing system packages or UI source."""

import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

MINIMUM = (6, 11, 2)


def version(value):
    if not isinstance(value, str) or not re.fullmatch(r"6\.\d+\.\d+", value):
        raise ValueError("unsupported Qt version")
    parsed = tuple(map(int, value.split(".")))
    if parsed < MINIMUM:
        raise ValueError("Qt below qualification floor")
    return parsed


def selected():
    # Match qt-build-utils: an explicit QMAKE must never silently fall back.
    explicit = os.environ.get("QMAKE")
    qmake = shutil.which(explicit) if explicit is not None else next(
        (found for name in ("qmake6", "qmake") if (found := shutil.which(name))), None)
    if not qmake:
        raise ValueError("Qt build tools unavailable")
    record = {"qmake": str(Path(qmake).resolve())}
    for key, query in (("version", "QT_VERSION"), ("prefix", "QT_INSTALL_PREFIX"),
                       ("libraries", "QT_INSTALL_LIBS")):
        record[key] = subprocess.check_output([qmake, "-query", query], text=True,
            stderr=subprocess.DEVNULL, timeout=15).strip()
    version(record["version"])
    expected = os.environ.get("GUST_EXPECT_QT_ROOT")
    if expected and (Path(record["prefix"]).resolve() != Path(expected).resolve()
                     or record["version"] != "6.11.2"):
        raise ValueError("selected Qt SDK mismatch")
    return record


if __name__ == "__main__":
    try:
        print(json.dumps({"status": "pass", "qt_version": selected()["version"]}))
    except (OSError, ValueError, subprocess.SubprocessError):
        print(json.dumps({"status": "fail", "reason": "Qt selection/version mismatch"}))
        sys.exit(1)
