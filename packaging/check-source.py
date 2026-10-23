#!/usr/bin/env python3
"""Verify the recipe's remote archive; optionally build and stage that source.

No checkout alias can substitute for the downloaded bytes. Evidence is private,
fresh and outside Git. This prepares files only, never installs or publishes.
"""

import argparse
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("pin_source", ROOT / "packaging/pin-source.py")
pin = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pin)
LIMIT = 64 * 1024 * 1024


def metadata(recipe):
    revision = re.search(r"^_commit=([0-9a-f]{40})$", recipe, re.MULTILINE)
    checksum = re.search(r"^sha256sums=\('([0-9a-f]{64})'\)$", recipe, re.MULTILINE)
    version = re.search(r"^pkgver=(\d+\.\d+\.\d+)\.r\d+\.g([0-9a-f]{12})$", recipe, re.MULTILINE)
    source = 'source=("$pkgname-$_commit.tar.gz::https://codeload.github.com/Zeus-Deus/gust/tar.gz/$_commit")'
    if (not revision or not checksum or not version or version[2] != revision[1][:12]
            or source not in recipe.splitlines() or "pkgname=gust" not in recipe.splitlines()):
        raise ValueError("unsupported or inconsistent source metadata")
    return {"revision": revision[1], "sha256": checksum[1], "version": version[1],
            "url": "https://codeload.github.com/Zeus-Deus/gust/tar.gz/" + revision[1]}


def validate(data, record):
    if len(data) > LIMIT or hashlib.sha256(data).hexdigest() != record["sha256"]:
        raise ValueError("source checksum or size mismatch")
    if pin.inspect_archive(data, record["revision"]) != record["version"]:
        raise ValueError("source version mismatch")
    prefix = "gust-" + record["revision"]
    seen = set()
    expanded = 0
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        for member in archive:
            path = PurePosixPath(member.name)
            if (path.is_absolute() or ".." in path.parts or not path.parts or path.parts[0] != prefix
                    or member.name.rstrip("/") != str(path) or str(path) in seen or len(seen) >= 100000):
                raise ValueError("unsafe or duplicate source member")
            seen.add(str(path))
            relative = str(path.relative_to(prefix))
            if relative.startswith("crates/app/ui/"):
                raise ValueError("source member traverses QML symlink")
            if member.issym():
                if relative != "crates/app/ui" or member.linkname != "../../ui":
                    raise ValueError("unaccounted source symlink")
            elif not (member.isfile() or member.isdir()):
                raise ValueError("unsupported source member type")
            expanded += member.size
            if member.size > 128 * 1024 * 1024 or expanded > 512 * 1024 * 1024:
                raise ValueError("expanded source exceeds budget")
    return prefix


def extract(data, output, record):
    prefix = validate(data, record)
    # Validation permits only regular files/directories and one canonical
    # internal directory link. The data filter adds extraction-time checks.
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        archive.extractall(output, filter="data")
    return output / prefix


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--build", action="store_true")
    parser.add_argument("--architecture", choices=("x86_64", "aarch64"))
    args = parser.parse_args()
    os.umask(0o077)
    output = args.output.resolve()
    try:
        if output.exists() or any((p / ".git").exists() for p in output.parents):
            raise ValueError("output must be fresh and outside Git")
        output.mkdir(parents=True, mode=0o700)
        record = metadata((ROOT / "PKGBUILD").read_text())
        with urllib.request.urlopen(record["url"], timeout=60) as response:
            data = response.read(LIMIT + 1)
        source = extract(data, output, record)
        (output / "source.json").write_text(json.dumps(record, indent=2) + "\n")
        if args.build:
            # Use the immutable snapshot's builder, never this checkout's
            # compiled outputs or a directory merely named after the pin.
            with (output / "qualification.log").open("w") as log:
                result = subprocess.run([sys.executable, str(source / "packaging/build-artifact.py")],
                                        cwd=source, stdout=log, stderr=log)
                if result.returncode in {75, 77}:
                    print(json.dumps({"status": "skip", "reason": "build prerequisite or lease unavailable"}))
                    return result.returncode
                result.check_returncode()
                command = [sys.executable, str(ROOT / "packaging/check-package.py"), "--stage",
                           "--source", str(source), "--output", str(output / "package")]
                if args.architecture:
                    command += ["--architecture", args.architecture]
                subprocess.run(command, stdout=log, stderr=log, check=True)
    except (OSError, ValueError, KeyError, tarfile.TarError, subprocess.SubprocessError) as error:
        print(json.dumps({"status": "fail", "category": type(error).__name__}))
        return 1
    print(json.dumps({"status": "pass", "scope": "source-and-staged-build" if args.build else "source-archive"}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
