#!/usr/bin/env python3
"""Compile the exact bridge permissions apply with a held write and a channel
standing in for the Qt queue, and prove the change counts as running until
its outcome, undo and report have been handled on the GUI thread."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

from tab_model_regressions import block


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    source = args.source.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    names = ["crates/app/src/bridge.rs", "crates/app/src/place_work.rs",
             "crates/app/tests/permissions_worker_fixture.rs.in",
             "crates/app/tests/permissions_worker_regressions.py",
             "crates/app/tests/tab_model_regressions.py"]
    inputs = {name: (source / name).read_bytes() for name in names}
    bridge = inputs[names[0]].decode()
    place = inputs[names[1]].decode()
    apply = block(bridge, "    fn permissions_apply(self: Pin<&mut Self>")
    running = block(bridge, "    fn permissions_running(&self)")
    claims = next(line for line in place.splitlines() if line.startswith("type Claims ="))
    work = "\n".join([block(place, "pub fn touches("), claims,
                      block(place, "#[derive(Clone, Default)]\npub struct ArchiveWork"),
                      block(place, "pub struct ArchiveClaim"), block(place, "impl ArchiveWork"),
                      block(place, "impl Drop for ArchiveClaim"), block(place, "fn lock<T>(")])
    template = inputs[names[2]].decode()
    for marker in ["@PLACE_WORK@", "@PERMISSIONS_RUNNING@", "@PERMISSIONS_APPLY@"]:
        if template.count(marker) != 1:
            raise ValueError(f"Fixture requires exactly one {marker}")
    generated = output / "probe.rs"
    generated.write_text(template.replace("@PLACE_WORK@", work)
                         .replace("@PERMISSIONS_RUNNING@", running)
                         .replace("@PERMISSIONS_APPLY@", apply))
    for name in ["home", "tmp"]:
        (output / name).mkdir(mode=0o700)
    rustc = shutil.which("rustc")
    if not rustc:
        raise RuntimeError("rustc is required for bridge worker regressions")
    env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": str(output / "home"),
           "TMPDIR": str(output / "tmp"), "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup"))}
    # One test at a time: the held write and the panic switch are globals.
    commands = [[rustc, "--edition=2021", "--test", str(generated), "-o", str(output / "probe")],
                [str(output / "probe"), "--test-threads=1"]]
    results = []
    for index, argv in enumerate(commands):
        run = subprocess.run(argv, env=env, capture_output=True, text=True, timeout=120)
        log = run.stdout + run.stderr
        (output / f"run-{index}.log").write_text(log)
        print(log, end="")
        results.append({"argv": argv, "exit": run.returncode})
        if run.returncode:
            break
    unchanged = all((source / name).read_bytes() == data for name, data in inputs.items())
    receipt = {"commands": results, "source_unchanged": unchanged,
               "source_hashes": {name: hashlib.sha256(data).hexdigest() for name, data in inputs.items()},
               "apply_sha256": hashlib.sha256(apply.encode()).hexdigest(),
               "running_sha256": hashlib.sha256(running.encode()).hexdigest(),
               "place_work_sha256": hashlib.sha256(work.encode()).hexdigest(),
               "scope": "Exact Rust permissions_apply/permissions_running and place-work claim; "
                        "set_modes, finish_mode_change, permissions_done and the Qt queue are seams. "
                        "Not native CXX acceptance."}
    (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    return results[-1]["exit"] if unchanged else 1


if __name__ == "__main__":
    sys.exit(main())
