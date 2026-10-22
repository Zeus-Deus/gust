#!/usr/bin/env python3
"""Exercise the shared runner's process boundary with synthetic Cargo output."""

import concurrent.futures
import contextlib
import fcntl
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import signal
import sys
import tempfile
import time
import unittest
from types import SimpleNamespace
from unittest.mock import patch

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
if os.environ.get('CASE') == 'diagnostic':
    print('error[E0425]: private-marker at /synthetic-private/location')
    print('error: could not compile `gust-app` (test) due to private-marker')
    print('error: could not compile `private-marker`')
    print('error: failed to run custom build command: private-marker')
    print('fatal error: private-marker')
sys.exit(23 if os.environ.get('CASE') in ('failure', 'diagnostic') else 0)
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

    def test_package_unit_scope_filters_manual_benchmark_but_rejects_other_ignores(self):
        result, summary = self.run_case("pass", "--workspace", "--package-unit", "--required", "--release")
        self.assertEqual(result.returncode, 0)
        self.assertEqual(summary["excluded_manual_benchmarks"], ["recursive::tests::copy_1000_small_files"])
        argv = json.loads(next(self.root.glob("capture-*")).read_text())["argv"]
        self.assertEqual(argv[-3:], ["--", "--skip", "recursive::tests::copy_1000_small_files"])
        self.assertIn("--lib", argv)
        self.assertNotIn("--ignored", argv)
        result, summary = self.run_case("ignored", "--package-unit", "--required")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(summary["ignored"], 1)

    def test_failure_diagnostics_emit_only_allowlisted_identifiers_and_categories(self):
        result, summary = self.run_case("diagnostic", "--app", "--required")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(summary["diagnostics"]["rust_error_codes"], ["E0425"])
        self.assertEqual(summary["diagnostics"]["failed_crates"], ["gust-app"])
        self.assertTrue(summary["diagnostics"]["build_script_failure"])
        self.assertTrue(summary["diagnostics"]["native_compiler_failure"])
        self.assertNotIn("private-marker", result.stdout + result.stderr)
        self.assertNotIn("/synthetic-private/", result.stdout + result.stderr)


class FixtureContainmentTests(unittest.TestCase):
    def test_fresh_fixture_is_built_and_sibling_data_is_untouched(self):
        with tempfile.TemporaryDirectory(prefix="gust-fresh-fixture-") as temporary:
            parent = Path(temporary)
            sibling = parent / "keep"
            sibling.write_text("outside fixture")
            destination = parent / "fixture"
            result = subprocess.run([str(ROOT / "tests/make-fixture.sh"), str(destination)],
                                    capture_output=True, timeout=20)
            self.assertEqual(result.returncode, 0)
            self.assertEqual(len(list((destination / "big").iterdir())), 5000)
            self.assertEqual(sibling.read_text(), "outside fixture")
            self.assertEqual(destination.stat().st_mode & 0o777, 0o700)

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
    def test_private_compositor_contract_accounts_for_mixed_skips_and_qml_errors(self):
        spec = importlib.util.spec_from_file_location("release_gates", ROOT / "tests/release-gates.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        for case, status, exit_code in (("pass", "PASS", 0), ("partial", "SKIP", 1),
                                       ("warnings", "FAIL", 1), ("missing-ledger", "FAIL", 1)):
            with self.subTest(case=case), tempfile.TemporaryDirectory(prefix="gust-native-contract-") as temporary:
                root = Path(temporary)
                binary = root / "installed/usr/bin/gust"
                binary.parent.mkdir(parents=True)
                binary.write_bytes(b"inert fixture; never executed")
                runner = root / "runner"
                runner.write_text("""#!/usr/bin/env python3
import os, sys
from pathlib import Path
out = Path(sys.argv[1]); out.mkdir()
rows = 'journey\\tstatus\\tseconds\\treason\\tartifact\\nlaunch\\tPASS\\t0\\t\\t\\n'
if os.environ['NATIVE_CASE'] == 'partial':
    rows += 'layout\\tSKIP\\t0\\tmissing pointer\\t\\n'
if os.environ['NATIVE_CASE'] != 'missing-ledger':
    (out / 'qml-warnings.txt').touch()
if os.environ['NATIVE_CASE'] == 'warnings':
    (out / 'qml-warnings.txt').write_text('private-runtime-marker')
(out / 'results.tsv').write_text(rows)
(out / 'invoked').touch()
""")
                runner.chmod(0o755)
                gate = module.Gates(SimpleNamespace(output=root / "evidence", gate=["native"],
                    required=True, installed_root=root / "installed", native_runner=runner))
                # Isolate runner accounting from artifact qualification, which
                # has its own real-file/provenance suite. No desktop is used.
                gate.installed_verified = True
                public = io.StringIO()
                with patch.dict(os.environ, GUST_DEDICATED_QA="1", WAYLAND_DISPLAY="",
                                HYPRLAND_INSTANCE_SIGNATURE="", NATIVE_CASE=case), contextlib.redirect_stdout(public):
                    self.assertEqual(gate.run(), exit_code)
                self.assertTrue((gate.output / "native/invoked").is_file())
                self.assertEqual(gate.results[-1]["status"], status)
                self.assertNotIn("private-runtime-marker", public.getvalue())

    def test_cancel_stops_owned_descendants_before_releasing_lease(self):
        # No Gust, compiler or shared lease is involved: exercise the real
        # process boundary with a synthetic child/grandchild and a temp lock.
        for cancellation in (signal.SIGINT, signal.SIGTERM):
            with self.subTest(signal=cancellation), tempfile.TemporaryDirectory(prefix="gust-cancel-test-") as temporary:
                root = Path(temporary)
                child_script = root / "child.py"
                child_script.write_text("""import os, subprocess, sys, time
from pathlib import Path
child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'])
Path(sys.argv[1]).write_text(str(os.getpid()) + ' ' + str(child.pid))
time.sleep(30)
""")
                wrapper = root / "wrapper.py"
                wrapper.write_text("""import importlib.util, signal, sys
from pathlib import Path
from types import SimpleNamespace
spec = importlib.util.spec_from_file_location('gates', sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
signal.signal(signal.SIGTERM, module.cancel_on_signal)
gate = module.Gates(SimpleNamespace(output=Path(sys.argv[2]) / 'out'))
gate.build_lock = Path(sys.argv[2]) / 'lease'
try:
    gate.command('owned', [sys.executable, sys.argv[2] + '/child.py', sys.argv[2] + '/pids'], build=True)
except KeyboardInterrupt:
    sys.exit(130)
""")
                sentinel = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
                owner = subprocess.Popen([sys.executable, str(wrapper), str(ROOT / "tests/release-gates.py"), str(root)],
                                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                pids = []
                try:
                    deadline = time.monotonic() + 5
                    while not (root / "pids").exists() and time.monotonic() < deadline:
                        time.sleep(0.02)
                    pids = [int(pid) for pid in (root / "pids").read_text().split()]
                    with (root / "lease").open("a") as lease:
                        with self.assertRaises(BlockingIOError):
                            fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
                        owner.send_signal(cancellation)
                        self.assertEqual(owner.wait(timeout=5), 130)
                        fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
                        for pid in pids:
                            state = Path(f"/proc/{pid}/stat")
                            self.assertTrue(not state.exists() or state.read_text().split()[2] == "Z")
                    self.assertIsNone(sentinel.poll())
                finally:
                    if owner.poll() is None:
                        owner.kill()
                        owner.wait()
                    if pids:
                        try:
                            os.killpg(pids[0], signal.SIGKILL)
                        except ProcessLookupError:
                            pass
                    sentinel.terminate()
                    sentinel.wait()

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

    def test_evidence_cannot_be_written_into_another_checkout(self):
        with tempfile.TemporaryDirectory(prefix="gust-other-checkout-") as temporary:
            root = Path(temporary)
            (root / ".git").mkdir()
            destination = root / "evidence"
            result = subprocess.run(["python3", str(ROOT / "tests/release-gates.py"),
                "--gate", "native", "--output", str(destination)],
                capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 1)
            self.assertFalse(destination.exists())


if __name__ == "__main__":
    unittest.main(verbosity=2)
