#!/usr/bin/env python3
"""Validate real staged/installed files, not just the text of a recipe.

--stage runs only the audited package() function against the selected source, into a
fresh private directory. Nothing is installed on the host. Raw dependency
resolution stays in the private output directory; stdout is counts only.
"""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import stat
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("build_artifact", ROOT / "packaging/build-artifact.py")
build = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build)


def runtime_files(source=ROOT):
    files = json.loads((source / "packaging/runtime-files.json").read_text())
    return [item for item in files if not item.get("when_source_exists")
            or (source / item["when_source_exists"]).is_file()]


def regular_beneath(root, relative):
    path = root
    if Path(relative).is_absolute() or ".." in Path(relative).parts:
        raise ValueError("invalid runtime path")
    for part in Path(relative).parts:
        path /= part
        if path.is_symlink():
            raise ValueError("runtime path contains symlink")
    if not stat.S_ISREG(path.stat().st_mode):
        raise ValueError("runtime artifact is not a regular file")
    return path


def verify(root, source=ROOT, architecture=None, *, require_receipt=True):
    machine = architecture or platform.machine()
    expected = {"x86_64": 62, "aarch64": 183}.get(machine)
    if expected is None:
        raise ValueError("unsupported artifact architecture")
    provenance = None
    if require_receipt:
        provenance = json.loads((source / "target/release/gust-build.json").read_text())
        if provenance["source_sha256"] != build.source_digest(source):
            raise ValueError("build receipt does not match current source")
        build.qt.version(provenance["qt"]["version"])
    receipt = []
    for item in runtime_files(source):
        installed = regular_beneath(root, item["path"])
        built = regular_beneath(source, item["source"])
        mode = stat.S_IMODE(installed.stat().st_mode)
        if mode != int(item["mode"], 8):
            raise ValueError("runtime artifact mode drift")
        content = installed.read_bytes()
        digest = hashlib.sha256(content).hexdigest()
        if digest != hashlib.sha256(built.read_bytes()).hexdigest():
            raise ValueError("runtime artifact differs from tested source/build")
        if item["path"].startswith("usr/bin/"):
            if provenance and provenance["executables"].get(installed.name) != digest:
                raise ValueError("executable is not the recorded build artifact")
            # No execution is needed to check the ELF machine and class.
            if (len(content) < 20 or content[:6] != b"\x7fELF\x02\x01"
                    or int.from_bytes(content[18:20], "little") != expected):
                raise ValueError("runtime executable architecture/format drift")
        receipt.append({"path": item["path"], "sha256": digest, "mode": item["mode"]})
    return receipt


def verify_linkage(root, source, log, qt_root=None):
    recorded = json.loads((source / "target/release/gust-build.json").read_text())["qt"]
    built_version = build.qt.version(recorded["version"])
    if qt_root is not None and Path(recorded["prefix"]).resolve() != qt_root.resolve():
        raise ValueError("build and runtime Qt SDK roots differ")
    versions = set()
    qt_count = 0
    for item in runtime_files(source):
        if not item["path"].startswith("usr/bin/"):
            continue
        result = subprocess.run(["ldd", str(root / item["path"])],
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                text=True, timeout=30)
        log.write(result.stdout)
        if result.returncode or "not found" in result.stdout:
            raise ValueError("unresolved runtime library")
        for line in result.stdout.splitlines():
            if not re.match(r"\s*libQt6", line):
                continue
            match = re.fullmatch(r"\s*libQt6[\w-]+\.so\.6\s+=>\s+(.+?)\s+\(0x[0-9a-fA-F]+\)\s*", line)
            if not match:
                raise ValueError("unrecognized Qt library resolution")
            library = Path(match[1]).resolve(strict=True)
            release = re.fullmatch(r"libQt6[\w-]+\.so\.(6\.\d+\.\d+)", library.name)
            if not release or build.qt.version(release[1]) < built_version:
                raise ValueError("runtime Qt is older than build Qt")
            if qt_root is not None and not library.is_relative_to(qt_root.resolve()):
                raise ValueError("runtime Qt resolved outside selected SDK")
            versions.add(release[1])
            qt_count += 1
    if not qt_count or len(versions) != 1:
        raise ValueError("missing or mixed Qt runtime versions")
    return {"qt_libraries": qt_count, "qt_version": versions.pop(),
            "selected_sdk_checked": qt_root is not None}


def stage(output, source=ROOT):
    # Run the actual trusted staging recipe, after its side-effect guard. A
    # symlink gives package() its normal extracted-source directory name.
    subprocess.run([sys.executable, str(ROOT / "tests/check-setup-packaging.py")],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
    recipe = (ROOT / "PKGBUILD").read_text()
    commit = re.search(r"^_commit=([0-9a-f]{40})$", recipe, re.MULTILINE)
    if not commit:
        raise ValueError("recipe needs a full source revision")
    (output / ("gust-" + commit[1])).symlink_to(source, target_is_directory=True)
    installed = output / "root"
    installed.mkdir()
    env = dict(os.environ, pkgdir=str(installed))
    with (output / "stage.log").open("w") as log:
        subprocess.run(["bash", "-euc", 'source "$1"; package', "bash", str(ROOT / "PKGBUILD")],
                       cwd=output, env=env, stdout=log, stderr=log, check=True)
    expected = {item["path"] for item in runtime_files(source)}
    actual = {str(p.relative_to(installed)) for p in installed.rglob("*") if not p.is_dir()}
    if actual != expected:
        raise ValueError("staged file set differs from runtime manifest")
    return installed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path)
    parser.add_argument("--stage", action="store_true")
    parser.add_argument("--source", type=Path, default=ROOT)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--architecture", choices=("x86_64", "aarch64"))
    args = parser.parse_args()
    output = args.output.resolve()
    if (output.is_relative_to(ROOT) or output.exists()
            or any((parent / ".git").exists() for parent in output.parents)):
        parser.error("output must be fresh and outside the checkout")
    output.mkdir(mode=0o700, parents=True)
    try:
        source = args.source.resolve()
        root = stage(output, source) if args.stage else args.root
        if root is None:
            raise ValueError("select --stage or --root")
        receipt = verify(root, source, architecture=args.architecture)
        with (output / "libraries.log").open("w") as log:
            sdk = os.environ.get("GUST_EXPECT_QT_ROOT")
            linkage = verify_linkage(root, source, log, Path(sdk) if sdk else None)
        (output / "receipt.json").write_text(json.dumps({"files": receipt,
            "linkage": linkage,
            "architecture": args.architecture or platform.machine(),
            "scope": "staged files and host dynamic libraries; not desktop/provider qualification"}, indent=2) + "\n")
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        # The diagnostic class is enough for public CI; private paths stay local.
        print(json.dumps({"status": "fail", "category": type(error).__name__}))
        return 1
    print(json.dumps({"status": "pass", "runtime_files": len(receipt)}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
