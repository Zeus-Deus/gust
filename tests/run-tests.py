#!/usr/bin/env python3
"""Run Cargo tests without counting runtime skips as successful tests.

Raw output stays in a private directory under target/test-runs. Only aggregate
counts reach stdout; logs can contain fixture paths and must not be uploaded.
--workspace includes the Qt app; other arguments are passed to Cargo.
"""

import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
PACKAGES = ("gust-core", "gust-ops", "gust-remote", "gust-thumbs", "gust-testkit")
MANUAL_BENCHMARK = "recursive::tests::copy_1000_small_files"
RESULT = re.compile(
    r"^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;",
    re.MULTILINE,
)


def enabled(value):
    return value.lower() in {"1", "true"}


def summarize(log, ledger, cargo_exit, required):
    groups = RESULT.findall(log)
    passed = sum(int(p) for p, _, _ in groups)
    failed = sum(int(f) for _, f, _ in groups)
    ignored = sum(int(i) for _, _, i in groups)
    records = set()
    invalid = False
    for line in ledger.splitlines():
        fields = line.split("\t")
        if len(fields) != 4 or fields[0] != "SKIP":
            invalid = True
        else:
            records.add(tuple(fields[1:3]))
    skipped = len(records)
    invoked = bool(groups) and passed + failed + ignored > 0
    invalid |= skipped > passed
    ok = cargo_exit == 0 and failed == 0 and invoked and not invalid
    if required and (skipped or ignored):
        ok = False
    return {
        "schema": 1, "status": "pass" if ok else "fail",
        "tests_observed": invoked, "cargo_exit": cargo_exit,
        "passed": max(0, passed - skipped), "failed": failed,
        "skipped": skipped, "ignored": ignored, "required": required,
        "accounting_valid": not invalid,
    }


def main(argv):
    package_unit = "--package-unit" in argv
    if package_unit:
        argv.remove("--package-unit")
        # This unconditional #[ignore] is a manual disk benchmark, not a unit
        # test. Do not enable ignored tests: it silently returns without its
        # separately provisioned destination. All other ignores still fail.
        boundary = argv.index("--") if "--" in argv else len(argv)
        argv.insert(boundary, "--lib")
        if "--" not in argv:
            argv.append("--")
        argv += ["--skip", MANUAL_BENCHMARK]
    required = enabled(os.environ.get("GUST_ALL_REQUIRED", ""))
    if "--required" in argv:
        required = True
        argv.remove("--required")
    packages = [arg for package in PACKAGES for arg in ("-p", package)]
    if "--workspace" in argv:
        argv.remove("--workspace")
        packages = ["--workspace"]
    elif "--app" in argv:
        argv.remove("--app")
        packages = ["-p", "gust-app"]
    elif any(arg in {"-p", "--package"} or arg.startswith("--package=") for arg in argv):
        # Explicit Cargo selectors replace the default set, allowing a focused
        # check without compiling every library in the workspace.
        packages = []
    parent = ROOT / "target/test-runs"
    parent.mkdir(parents=True, exist_ok=True)
    run = Path(tempfile.mkdtemp(prefix="run-", dir=parent))
    ledger = run / "skips.tsv"
    ledger.touch(mode=0o600)
    env = dict(os.environ, GUST_SKIP_LEDGER=str(ledger), CARGO_TERM_COLOR="never")
    if required:
        env["GUST_ALL_REQUIRED"] = "1"
    # No live clipboard or remote fixture is enabled implicitly. Their owners
    # must provision and opt in to those destructive/interactive test cases.
    command = ["cargo", "test", "--locked", "--no-fail-fast", *packages, *argv]
    try:
        with (run / "cargo.log").open("w", encoding="utf-8") as output:
            process = subprocess.run(command, cwd=ROOT, env=env, stdout=output,
                                     stderr=subprocess.STDOUT, check=False)
        log = (run / "cargo.log").read_text(errors="replace")
        result = summarize(log, ledger.read_text(), process.returncode, required)
    except OSError:
        result = summarize("", "", 127, required)
    result["scope"] = "package-library-unit" if package_unit else "selected-cargo-tests"
    result["excluded_manual_benchmarks"] = [MANUAL_BENCHMARK] if package_unit else []
    (run / "summary.json").write_text(json.dumps(result, indent=2) + "\n")
    # Paths, test names and skip reasons are deliberately private.
    print(json.dumps(result, sort_keys=True))
    return 0 if result["status"] == "pass" else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
