#!/usr/bin/env python3
"""Run Cargo tests without counting runtime skips as successful tests.

Raw output stays in a private directory under target/test-runs. Public output
uses counts and explicitly allowlisted identifiers, never diagnostic prose.
--workspace includes the Qt app; other arguments are passed to Cargo.
"""

import io
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
PACKAGES = ("gust-core", "gust-ops", "gust-remote", "gust-thumbs", "gust-testkit")
MANUAL_BENCHMARK = "recursive::tests::copy_1000_small_files"
RESULT = re.compile(
    r"^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;",
    re.MULTILINE,
)
KNOWN_CRATES = frozenset(re.findall(r'^name = "([a-zA-Z0-9_-]+)"$',
                                  (ROOT / "Cargo.lock").read_text(), re.MULTILINE))


def enabled(value):
    return value.lower() in {"1", "true"}


def public_test_catalogue():
    """Only committed, unambiguous, top-level integration tests are public IDs.

    Never scan a dirty working tree or infer namespaces from arbitrary log text.
    Missing Git/source metadata disables identifiers without changing test status.
    """
    try:
        result = subprocess.run(["git", "archive", "HEAD", "crates"], cwd=ROOT,
                                capture_output=True, timeout=10, check=True)
        with tarfile.open(fileobj=io.BytesIO(result.stdout)) as archive:
            members = {m.name: m for m in archive if m.isfile()}
            crates = {}
            for name, member in members.items():
                if re.fullmatch(r"crates/[a-zA-Z0-9_-]+/Cargo.toml", name):
                    package = tomllib.loads(archive.extractfile(member).read().decode())["package"]["name"]
                    if package in (*PACKAGES, "gust-app"):
                        crates[name.rsplit("/", 1)[0]] = package
            targets = {}
            for name, member in members.items():
                match = re.fullmatch(r"(crates/[a-zA-Z0-9_-]+)/tests/([a-zA-Z0-9_]+)\.rs", name)
                if match and match[1] in crates:
                    source = archive.extractfile(member).read().decode()
                    names = set(re.findall(r"(?m)^#\[test\]\r?\n(?:#\[[^\n]*\]\r?\n)*"
                                           r"fn ([a-zA-Z_][a-zA-Z0-9_]*)\s*\(", source))
                    targets.setdefault(match[2], []).append((crates[match[1]], names))
            return {target: owners[0] for target, owners in targets.items() if len(owners) == 1}
    except (OSError, subprocess.SubprocessError, tarfile.TarError, ValueError, KeyError, UnicodeError):
        return {}


def public_failures(log, catalogue):
    accepted = set()
    current = None
    for line in log.splitlines():
        if re.match(r"^\s*(?:Running |Doc-tests )", line):
            target = re.match(r"^\s*Running tests/([a-zA-Z0-9_]+)\.rs \(", line)
            current = (target[1], catalogue.get(target[1])) if target else None
        failure = re.fullmatch(r"test ([a-zA-Z_][a-zA-Z0-9_:]*) \.\.\. FAILED", line)
        if failure and current and current[1] and failure[1] in current[1][1]:
            accepted.add((current[1][0], current[0], failure[1]))
    return [dict(zip(("crate", "target", "test"), item)) for item in sorted(accepted)[:64]]


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
        # Emit only bounded compiler identifiers/categories, never diagnostic
        # prose, source excerpts, paths or panic payloads.
        "diagnostics": {
            "rust_error_codes": sorted(set(re.findall(r"(?m)^error\[(E\d{4})\]:", log)))[:16],
            "failed_crates": sorted(set(re.findall(
                r"(?m)^error: (?:could not compile|failed to run custom build command for) "
                r"`([a-zA-Z0-9_-]{1,80})(?=[` ])", log)) & KNOWN_CRATES)[:16],
            "build_script_failure": "failed to run custom build command" in log,
            "native_compiler_failure": any(marker in log for marker in (
                "fatal error:", "error occurred in cc-rs", "ToolExecError"))
                or ("linking with" in log and "failed" in log),
            "qt_detection_failure": any(marker in log for marker in (
                "Could not find Qt installation", "QtMissingVersion", "QMakeSetQtMissing")),
            "qt_tool_missing": any("Could not find " + tool in log for tool in (
                "qmlcachegen", "qmltyperegistrar", "moc", "rcc")),
            "qml_generation_failure": any(marker in log for marker in (
                "qmlcachegen failed", "qmltyperegistrar failed")),
            "test_process_failure": "test failed, to rerun pass" in log,
        },
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
    log = ""
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
    catalogue = public_test_catalogue() if enabled(os.environ.get("GUST_PUBLIC_TEST_IDS", "")) else {}
    # Unknown/unit/nested/ambiguous names stay private. IDs never affect status.
    failures = public_failures(log, catalogue) if catalogue else []
    result["diagnostics"]["failed_test_ids"] = failures
    result["diagnostics"]["test_identifier_catalogue"] = bool(catalogue)
    result["diagnostics"]["unclassified_test_failures"] = max(0, result["failed"] - len(failures))
    (run / "summary.json").write_text(json.dumps(result, indent=2) + "\n")
    # Paths, unknown test names and skip reasons are deliberately private.
    print(json.dumps(result, sort_keys=True))
    return 0 if result["status"] == "pass" else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
