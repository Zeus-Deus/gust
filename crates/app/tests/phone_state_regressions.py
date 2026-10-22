#!/usr/bin/env python3
"""Compile the exact bridge phone-open method with inert worker/transport seams."""
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
    names = ["crates/app/src/bridge.rs", "crates/app/tests/phone_open_fixture.rs.in",
             "crates/app/tests/phone_state_regressions.py", "crates/app/tests/tab_model_regressions.py"]
    inputs = {name: (source / name).read_bytes() for name in names}
    bridge = inputs[names[0]].decode()
    method = block(bridge, "    fn phone_open(self: Pin<&mut Self>")
    template = inputs[names[1]].decode()
    worker = block(bridge, "    fn place_op<F>")
    statement = "let outcome =" + worker.split("let outcome =", 1)[1].split("let _ = thread.queue", 1)[0]
    guard = block(bridge, "fn run_place_work<F>") if "fn run_place_work<F>" in bridge else ""
    generated = output / "probe.rs"
    generated.write_text(template.replace("@PHONE_OPEN@", method).replace("@WORK_GUARD@", guard).replace("@WORK_OUTCOME@", statement))
    for name in ["home", "tmp"]:
        (output / name).mkdir(mode=0o700)
    rustc = shutil.which("rustc")
    if not rustc:
        raise RuntimeError("rustc is required for bridge worker regressions")
    env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": str(output / "home"),
           "TMPDIR": str(output / "tmp"), "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup"))}
    commands = [[rustc, "--edition=2021", "--test", str(generated), "-o", str(output / "probe")],
                [str(output / "probe"), "--test-threads=1", "--nocapture"]]
    results = []
    for index, argv in enumerate(commands):
        run = subprocess.run(argv, env=env, capture_output=True, text=True, timeout=60)
        log = run.stdout + run.stderr
        (output / f"run-{index}.log").write_text(log)
        print(log, end="")
        results.append({"argv": argv, "exit": run.returncode})
        if run.returncode:
            break
    unchanged = all((source / name).read_bytes() == data for name, data in inputs.items())
    receipt = {"commands": results, "source_unchanged": unchanged,
               "source_hashes": {name: hashlib.sha256(data).hexdigest() for name, data in inputs.items()},
               "method_sha256": hashlib.sha256(method.encode()).hexdigest(),
               "worker_statement_sha256": hashlib.sha256(statement.encode()).hexdigest(),
               "scope": "Exact Rust phone_open method and place_op outcome statement/guard; Qt queue, scan/mount and worker admission are inert seams. Not native CXX acceptance."}
    (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    return results[-1]["exit"] if unchanged else 1


if __name__ == "__main__":
    sys.exit(main())
