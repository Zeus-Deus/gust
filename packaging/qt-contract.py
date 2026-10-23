#!/usr/bin/env python3
"""Select and record Qt without changing system packages or UI source."""

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

MINIMUM = (6, 11, 2)
INSTALLER_ERRORS = frozenset((
    "ArchiveDownloadError", "ArchiveChecksumError", "ChecksumDownloadFailure",
    "ArchiveConnectionError", "ArchiveListError", "NoPackageFound", "EmptyMetadata",
    "CliInputError", "ArchiveExtractionError", "UpdaterError", "OutOfMemory",
    "OutOfDiskSpace", "DiskAccessNotPermitted", "Bad7zFile", "BadZipFile",
    "ConnectionError", "SSLError", "ReadTimeout", "TimeoutError", "PermissionError",
    "FileNotFoundError", "OSError", "ValueError", "UnicodeDecodeError", "EOFError",
))


def installer_failure(path, phase, code):
    if not isinstance(code, int) or not 0 < code < 256:
        raise ValueError("expected an actual installer failure")
    with path.open("rb") as log:
        content = log.read(8 * 1024 * 1024 + 1)
    text = content[:8 * 1024 * 1024].decode("utf-8", errors="replace")
    # Only fixed identifiers/booleans leave the private installer log. The
    # pinned aqt's private traceback exposes its typed exception without any
    # need to publish URLs, arguments, paths, host details or exception prose.
    errors = sorted(set(re.findall(r"\b(?:" + "|".join(sorted(INSTALLER_ERRORS)) + r")\b", text)))
    return {"status": "fail", "phase": phase, "installer_exit": code,
            "exception_classes": errors, "log_truncated": len(content) > 8 * 1024 * 1024,
            "pip_distribution_unavailable": "No matching distribution found" in text,
            "pip_dependency_conflict": "ResolutionImpossible" in text,
            "disk_full": "No space left on device" in text or "Disk quota exceeded" in text}


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
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--diagnose-installer", type=Path)
    parser.add_argument("--phase", choices=("bootstrap", "sdk"), default="sdk")
    parser.add_argument("--exit-code", type=int)
    args = parser.parse_args()
    try:
        if args.diagnose_installer:
            print(json.dumps(installer_failure(args.diagnose_installer, args.phase, args.exit_code)))
        else:
            print(json.dumps({"status": "pass", "qt_version": selected()["version"]}))
    except (OSError, ValueError, subprocess.SubprocessError):
        print(json.dumps({"status": "fail", "reason": "Qt selection or installer accounting failed"}))
        sys.exit(1)
