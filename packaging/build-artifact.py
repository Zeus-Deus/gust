#!/usr/bin/env python3
"""Build release executables and bind their hashes to the actual source inputs.

Owns the nonblocking build lease. Logs/receipt stay private under target/release;
do not wrap this command in another acquisition of /tmp/gust-build.lock.
"""

import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("qt_contract", ROOT / "packaging/qt-contract.py")
qt = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qt)
spec = importlib.util.spec_from_file_location("owned_process", ROOT / "packaging/owned-process.py")
owned = importlib.util.module_from_spec(spec)
spec.loader.exec_module(owned)


def source_revision(root=ROOT):
    if (root / ".git").exists():
        return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root,
                                       stderr=subprocess.DEVNULL, text=True).strip()
    # GitHub source archives have no .git directory. Do not accidentally read
    # an enclosing packaging checkout's HEAD or require Git just to build.
    match = re.fullmatch(r"gust-([0-9a-f]{40})", root.name)
    return match[1] if match else "unversioned-source"


def source_digest(root=ROOT):
    paths = [root / "Cargo.toml", root / "Cargo.lock"]
    qml_link = root / "crates/app/ui"
    if (not qml_link.is_symlink() or os.readlink(qml_link) != "../../ui"
            or qml_link.resolve(strict=True) != (root / "ui").resolve(strict=True)
            or (root / "ui").is_symlink()):
        raise ValueError("unaccounted QML input root")
    digest = hashlib.sha256()
    digest.update(b"crates/app/ui\0symlink\0../../ui\0")
    for folder in ("crates", "ui"):
        for path in (root / folder).rglob("*"):
            if path.is_symlink():
                if path != qml_link:
                    raise ValueError("unaccounted source symlink")
            elif path.is_file() and path.suffix in {".rs", ".toml", ".qml", ".cpp", ".h", ".qrc"}:
                paths.append(path)
    for path in sorted(set(paths)):
        digest.update(str(path.relative_to(root)).encode() + b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def main(*, build_lock=Path("/tmp/gust-build.lock")):
    os.umask(0o077)
    target = ROOT / "target/release"
    target.mkdir(parents=True, exist_ok=True)
    receipt = target / "gust-build.json"
    try:
        with build_lock.open("a") as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                print(json.dumps({"status": "skip", "reason": "build lease busy"}))
                return 75
            receipt.unlink(missing_ok=True)
            before = source_digest()
            selected_qt = qt.selected()
            env = dict(os.environ, CARGO_TARGET_DIR=str(ROOT / "target"),
                       CARGO_BUILD_JOBS=os.environ.get("CARGO_BUILD_JOBS", "2"),
                       QMAKE=selected_qt["qmake"])
            registry = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo"))) / "registry"
            env["RUSTFLAGS"] = (os.environ.get("RUSTFLAGS", "")
                + f" --remap-path-prefix={ROOT}=/usr/src/gust"
                + f" --remap-path-prefix={registry}=/usr/src/cargo-registry")
            commands = [["cargo", "build", "--release", "--locked", "-p", "gust-app"]]
            names = ["gust"]
            if (ROOT / "crates/ops/src/bin/gust-archive-helper.rs").is_file():
                commands.append(["cargo", "build", "--release", "--locked", "-p", "gust-ops", "--bin", "gust-archive-helper"])
                names.append("gust-archive-helper")
            with (target / "gust-build.log").open("w") as log:
                for command in commands:
                    code = owned.run(command, cwd=ROOT, env=env, log=log)
                    if code:
                        raise subprocess.CalledProcessError(code, command)
            if source_digest() != before:
                raise ValueError("source changed during build")
            record = {"schema": 1, "source_sha256": before,
                "revision": source_revision(),
                "qt": selected_qt,
                "executables": {name: hashlib.sha256((target / name).read_bytes()).hexdigest() for name in names}}
            receipt.write_text(json.dumps(record, indent=2) + "\n")
            receipt.chmod(0o600)
    except KeyboardInterrupt:
        print(json.dumps({"status": "fail", "reason": "artifact build cancelled; owned processes stopped"}))
        return 130
    except (OSError, ValueError, subprocess.SubprocessError):
        print(json.dumps({"status": "fail", "reason": "artifact build failed"}))
        return 1
    print(json.dumps({"status": "pass", "executables": len(names)}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
