#!/usr/bin/env python3
"""Compile exact bridge permissions/Run methods with a held write and Qt-queue
seam. Exercise real finish/reload, lister, identity, argv, chmod and undo,
including insertion between prompt capture and acceptance."""
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
             "crates/app/tests/tab_model_regressions.py", "crates/core/src/open.rs",
             "crates/ops/src/attrs.rs", "crates/core/src/lister.rs"]
    inputs = {name: (source / name).read_bytes() for name in names}
    bridge = inputs[names[0]].decode()
    place = inputs[names[1]].decode()
    apply = block(bridge, "    fn permissions_apply(self: Pin<&mut Self>")
    running = block(bridge, "    fn permissions_running(&self)")
    worker = block(bridge, "    fn start_mode_change(")
    claims = next(line for line in place.splitlines() if line.startswith("type Claims ="))
    work = "\n".join([block(place, "pub fn touches("), claims,
                      block(place, "#[derive(Clone, Default)]\npub struct ArchiveWork"),
                      block(place, "pub struct ArchiveClaim"), block(place, "impl ArchiveWork"),
                      block(place, "impl Drop for ArchiveClaim"), block(place, "fn lock<T>(")])
    methods = {"@FINISH_MODE_CHANGE@": block(bridge, "    fn finish_mode_change("),
               "@RELOAD@": block(bridge, "    fn reload(self: Pin<&mut Self>)"),
               "@RUN_PREPARE@": block(bridge, "    fn run_prepare(mut self:"),
               "@RUN_CONFIRM@": block(bridge, "    fn run_confirm(mut self:"),
               "@RUN_DISCARD@": block(bridge, "    fn run_discard(mut self:")}
    template = inputs[names[2]].decode()
    for marker, method in methods.items():
        if template.count(marker) != 1:
            raise ValueError(f"Fixture requires exactly one {marker}")
        template = template.replace(marker, method)
    for marker in ["@PLACE_WORK@", "@PERMISSIONS_RUNNING@", "@PERMISSIONS_APPLY@", "@MODE_WORKER@"]:
        if template.count(marker) != 1:
            raise ValueError(f"Fixture requires exactly one {marker}")
    generated = output / "probe.rs"
    generated.write_text(template.replace("@PLACE_WORK@", work)
                         .replace("@PERMISSIONS_RUNNING@", running)
                         .replace("@PERMISSIONS_APPLY@", apply)
                         .replace("@MODE_WORKER@", worker))
    for name in ["home", "tmp"]:
        (output / name).mkdir(mode=0o700)
    rustc = shutil.which("rustc")
    if not rustc:
        raise RuntimeError("rustc is required for bridge worker regressions")
    env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": str(output / "home"),
           "TMPDIR": str(output / "tmp"), "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup"))}
    # One test at a time: the held write and the panic switch are globals.
    build = subprocess.run(["cargo", "build", "--locked", "-p", "gust-core", "-p", "gust-ops",
                            "-p", "serde_json", "--message-format=json"], cwd=source,
                           capture_output=True, text=True, timeout=600)
    (output / "build.log").write_text(build.stdout + build.stderr)
    if build.returncode:
        raise RuntimeError("Fixture dependencies failed to build; see build.log")
    libraries = {}
    for line in build.stdout.splitlines():
        artifact = json.loads(line)
        if artifact.get("reason") == "compiler-artifact":
            for filename in artifact.get("filenames", []):
                if filename.endswith(".rlib"):
                    libraries[artifact["target"]["name"]] = filename
    deps = Path(libraries["gust_core"]).parent / "deps"
    externs = ["-L", f"dependency={deps}"]
    for alias, name in [("gust_core", "gust_core"), ("actual_ops", "gust_ops"), ("serde_json", "serde_json")]:
        externs.extend(["--extern", f"{alias}={libraries[name]}"])
    commands = [[rustc, *externs, "--edition=2021", "--test", str(generated), "-o", str(output / "probe")],
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
               "worker_sha256": hashlib.sha256(worker.encode()).hexdigest(),
               "running_sha256": hashlib.sha256(running.encode()).hexdigest(),
               "place_work_sha256": hashlib.sha256(work.encode()).hexdigest(),
               "scope": "Exact Rust permissions_apply/start_mode_change/permissions_running and place-work claim; "
                        "real finish_mode_change/reload, prompt capture/acceptance and core Run identity/argv; "
                        "real attrs and lister in insertion regressions. Qt queue, snapshot publication and terminal launch are seams. "
                        "Not native CXX acceptance."}
    (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    return results[-1]["exit"] if unchanged else 1


if __name__ == "__main__":
    sys.exit(main())
