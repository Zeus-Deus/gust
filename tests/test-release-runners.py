#!/usr/bin/env python3
"""Exercise the shared runner's process boundary with synthetic Cargo output."""

import concurrent.futures
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
RUNNER = ROOT / "tests/run-tests.sh"


class CargoRunnerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="gust-runner-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        cargo = self.root / "cargo"
        cargo.write_text("""#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
Path(os.environ['CAPTURE'] + '-' + str(os.getpid())).write_text(json.dumps({
    'argv': sys.argv[1:], 'ledger': os.environ['GUST_SKIP_LEDGER']}))
if os.environ.get('CASE') == 'skip':
    Path(os.environ['GUST_SKIP_LEDGER']).write_text('SKIP\\tFIXTURE\\tcase\\tprivate fixture missing\\n')
if os.environ.get('CASE') != 'empty':
    print('test result: ok. 2 passed; 0 failed; ' + ('1' if os.environ.get('CASE') == 'ignored' else '0') + ' ignored; 0 measured; 0 filtered out;')
sys.exit(23 if os.environ.get('CASE') == 'failure' else 0)
""")
        cargo.chmod(0o755)

    def run_case(self, case="pass", *args):
        env = dict(os.environ, PATH=str(self.root) + os.pathsep + os.environ["PATH"],
                   CASE=case, CAPTURE=str(self.root / "capture"), GUST_ALL_REQUIRED="0")
        result = subprocess.run([str(RUNNER), *args], env=env, text=True,
                                capture_output=True, timeout=15)
        summary = json.loads(result.stdout)
        self.assertNotIn("private fixture", result.stdout + result.stderr)
        return result, summary

    def test_required_skip_and_ignored_are_failures_even_after_zero_exit(self):
        for case in ("skip", "ignored"):
            with self.subTest(case=case):
                result, summary = self.run_case(case, "--required")
                self.assertEqual(result.returncode, 1)
                self.assertEqual(summary["status"], "fail")

    def test_ordinary_skip_is_subtracted_from_passed(self):
        result, summary = self.run_case("skip")
        self.assertEqual(result.returncode, 0)
        self.assertEqual((summary["passed"], summary["skipped"]), (1, 1))

    def test_empty_success_and_cargo_failure_cannot_pass(self):
        for case in ("empty", "failure"):
            with self.subTest(case=case):
                result, summary = self.run_case(case)
                self.assertEqual(result.returncode, 1)
                self.assertEqual(summary["status"], "fail")

    def test_app_and_workspace_use_accounting_and_locked_cargo(self):
        for selection in ("--app", "--workspace"):
            result, summary = self.run_case("pass", selection)
            self.assertEqual(result.returncode, 0)
            self.assertTrue(summary["tests_observed"])
        captures = [json.loads(p.read_text())["argv"] for p in self.root.glob("capture-*")]
        self.assertTrue(all("--locked" in argv for argv in captures))
        self.assertTrue(any("gust-app" in argv for argv in captures))
        self.assertTrue(any("--workspace" in argv for argv in captures))

    def test_concurrent_runs_have_private_distinct_ledgers(self):
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            results = list(pool.map(self.run_case, ("skip", "pass")))
        self.assertEqual(sorted(s["skipped"] for _, s in results), [0, 1])
        ledgers = [Path(json.loads(p.read_text())["ledger"]) for p in self.root.glob("capture-*")]
        self.assertEqual(len(set(ledgers)), 2)
        for ledger in ledgers:
            self.assertEqual(ledger.parent.stat().st_mode & 0o777, 0o700)

    def test_explicit_package_selector_does_not_expand_to_every_library(self):
        result, _ = self.run_case("pass", "-p", "gust-testkit")
        self.assertEqual(result.returncode, 0)
        capture = json.loads(next(self.root.glob("capture-*")).read_text())
        self.assertIn("gust-testkit", capture["argv"])
        self.assertNotIn("gust-core", capture["argv"])


class FixtureContainmentTests(unittest.TestCase):
    def test_existing_files_and_symlink_targets_survive_fixture_builder(self):
        with tempfile.TemporaryDirectory(prefix="gust-containment-") as temporary:
            root = Path(temporary)
            sentinel = root / "keep"
            sentinel.write_text("preserve this data")
            alias = root / "alias"
            alias.symlink_to(root, target_is_directory=True)
            for destination in (root, alias, alias / "nested", root / ".." / "unsafe"):
                result = subprocess.run([str(ROOT / "tests/make-fixture.sh"), str(destination)],
                                        capture_output=True, timeout=10)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(sentinel.read_text(), "preserve this data")
            self.assertFalse((root / "Projects").exists())

    def test_root_and_home_are_rejected_before_mutation(self):
        for destination in ("/", str(Path.home()), str(ROOT), "/tmp"):
            result = subprocess.run([str(ROOT / "tests/make-fixture.sh"), destination],
                                    capture_output=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)


class GateAdmissionTests(unittest.TestCase):
    def test_unavailable_native_is_skip_and_required_mode_fails(self):
        with tempfile.TemporaryDirectory(prefix="gust-gate-test-") as temporary:
            for required in (False, True):
                destination = Path(temporary) / str(required)
                command = ["python3", str(ROOT / "tests/release-gates.py"),
                           "--gate", "native", "--output", str(destination)]
                if required:
                    command.append("--required")
                result = subprocess.run(command, env=dict(os.environ, GUST_DEDICATED_QA="0"),
                                        capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, int(required))
                self.assertEqual(json.loads(result.stdout)["status"], "SKIP")
                self.assertFalse((destination / "native").exists())

    def test_evidence_cannot_be_written_under_checkout(self):
        destination = ROOT / "target/refused-gate-evidence"
        result = subprocess.run(["python3", str(ROOT / "tests/release-gates.py"),
            "--gate", "native", "--output", str(destination)],
            capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 1)
        self.assertFalse(destination.exists())


if __name__ == "__main__":
    unittest.main(verbosity=2)
