#!/usr/bin/env python3
"""Pin both recipes to an immutable, fetchable GitHub source snapshot.

Downloads only. No release/tag/package is created. Review the changed recipes
before committing. The archive is inspected without unpacking or running it.
"""

import argparse
import hashlib
import io
from pathlib import Path
import re
import subprocess
import tarfile
import tomllib
import urllib.request

ROOT = Path(__file__).resolve().parents[1]


def inspect_archive(data, revision):
    prefix = "gust-" + revision
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        member = archive.getmember(prefix + "/Cargo.toml")
        if not member.isfile() or member.size > 65536:
            raise ValueError("invalid source manifest")
        version = tomllib.loads(archive.extractfile(member).read().decode())["workspace"]["package"]["version"]
        # A recipe must not pin a snapshot lacking the scripts it invokes.
        for required in ("tests/run-tests.py", "packaging/build-artifact.py",
                         "packaging/check-package.py", "packaging/check-source.py",
                         "packaging/runtime-files.json", "packaging/qt-contract.py",
                         "packaging/owned-process.py",
                         "crates/ops/src/bin/gust-archive-helper.rs"):
            if not archive.getmember(prefix + "/" + required).isfile():
                raise ValueError("source snapshot predates the release tooling")
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise ValueError("unsupported source version")
    return version


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("revision", help="full 40-character published source commit")
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}", args.revision):
        parser.error("a full lowercase commit SHA is required")
    url = "https://codeload.github.com/Zeus-Deus/gust/tar.gz/" + args.revision
    with urllib.request.urlopen(url, timeout=60) as response:
        data = response.read(64 * 1024 * 1024 + 1)
    if len(data) > 64 * 1024 * 1024:
        raise ValueError("source archive exceeds budget")
    version = inspect_archive(data, args.revision)
    digest = hashlib.sha256(data).hexdigest()
    recipe = (ROOT / "PKGBUILD").read_text()
    recipe = re.sub(r"^_commit=.*$", "_commit=" + args.revision, recipe, flags=re.MULTILINE)
    # This is a source snapshot, not a new upstream release tag.
    count = subprocess.check_output(["git", "rev-list", "--count", args.revision],
                                    cwd=ROOT, text=True).strip()
    recipe = re.sub(r"^pkgver=.*$", f"pkgver={version}.r{count}.g{args.revision[:12]}", recipe, flags=re.MULTILINE)
    recipe = re.sub(r"^sha256sums=.*$", f"sha256sums=('{digest}')", recipe, flags=re.MULTILINE)
    for path in (ROOT / "PKGBUILD", ROOT / "docs/release/PKGBUILD"):
        path.write_text(recipe)
    print(f"Pinned source revision {args.revision}; SHA-256 {digest}")


if __name__ == "__main__":
    main()
