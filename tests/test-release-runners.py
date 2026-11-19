#!/usr/bin/env python3
"""Exercise the shared runner's process boundary with synthetic Cargo output."""

import argparse
import concurrent.futures
import contextlib
import fcntl
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import shutil
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
if os.environ.get('CASE') not in ('empty', 'build-script', 'public-ids'):
    print('test result: ok. 2 passed; 0 failed; ' + ('1' if os.environ.get('CASE') == 'ignored' else '0') + ' ignored; 0 measured; 0 filtered out;')
if os.environ.get('CASE') == 'diagnostic':
    print('error[E0425]: private-marker at /synthetic-private/location')
    print('error: could not compile `gust-app` (test) due to private-marker')
    print('error: could not compile `private-marker`')
    print('error: failed to run custom build command: private-marker')
    print('fatal error: private-marker')
if os.environ.get('CASE') == 'build-script':
    print('error: failed to run custom build command for `gust-app v0.1.0 (/synthetic-private/location)`')
    print('error: failed to run custom build command for `private-marker v0.1.0`')
    print('qmlcachegen failed for /synthetic-private/location: private-marker')
    print('error occurred in cc-rs: private-marker')
if os.environ.get('CASE') == 'public-ids':
    print('     Running tests/archive_complete.rs (/synthetic-private/location)')
    print('test all_2005_files_are_extracted_even_when_preview_is_truncated ... FAILED')
    print('test private_marker ... FAILED')
    print('test private_marker::all_2005_files_are_extracted_even_when_preview_is_truncated ... FAILED')
    print('     Running tests/private_marker.rs (/synthetic-private/location)')
    print('test all_2005_files_are_extracted_even_when_preview_is_truncated ... FAILED')
    print('test result: FAILED. 0 passed; 4 failed; 0 ignored; 0 measured; 0 filtered out;')
    print('private-marker assertion: /synthetic-private/location')
sys.exit(23 if os.environ.get('CASE') in ('failure', 'diagnostic', 'build-script', 'public-ids') else 0)
""")
        cargo.chmod(0o755)

    def run_case(self, case="pass", *args, public_ids=False, runner=RUNNER):
        env = dict(os.environ, PATH=str(self.root) + os.pathsep + os.environ["PATH"],
                   CASE=case, CAPTURE=str(self.root / "capture"), GUST_ALL_REQUIRED="0",
                   GUST_PUBLIC_TEST_IDS="1" if public_ids else "0")
        result = subprocess.run([str(runner), *args], env=env, text=True,
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

    def test_caller_locked_flag_is_not_repeated(self):
        result, _ = self.run_case("pass", "--locked", "--", "--locked")
        self.assertEqual(result.returncode, 0)
        argv = json.loads(next(self.root.glob("capture-*")).read_text())["argv"]
        boundary = argv.index("--")
        self.assertEqual(argv[:boundary].count("--locked"), 1)
        self.assertEqual(argv[boundary:], ["--", "--locked"])

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

    def test_app_scope_filters_only_its_named_manual_trial_and_reports_it(self):
        trial = "projection::tests::release_scheduler_supersession_trials"
        result, summary = self.run_case("pass", "--app", "--required", "--", "caller_filter")
        self.assertEqual(result.returncode, 0)
        self.assertEqual(summary["excluded_manual_benchmarks"], [trial])
        argv = json.loads(next(self.root.glob("capture-*")).read_text())["argv"]
        self.assertEqual(argv[argv.index("--"):], ["--", "caller_filter", "--skip", trial])
        self.assertNotIn("--ignored", argv)
        result, summary = self.run_case("ignored", "--app", "--required")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(summary["ignored"], 1)

    def test_custom_build_header_reports_locked_crate_without_version_or_paths(self):
        result, summary = self.run_case("build-script", "--app", "--required")
        self.assertEqual(result.returncode, 1)
        self.assertFalse(summary["tests_observed"])
        self.assertEqual(summary["diagnostics"]["failed_crates"], ["gust-app"])
        self.assertEqual(summary["diagnostics"]["rust_error_codes"], [])
        self.assertTrue(summary["diagnostics"]["build_script_failure"])
        self.assertTrue(summary["diagnostics"]["qml_generation_failure"])
        self.assertTrue(summary["diagnostics"]["native_compiler_failure"])
        self.assertFalse(summary["diagnostics"]["qt_detection_failure"])
        self.assertFalse(summary["diagnostics"]["qt_tool_missing"])
        self.assertNotIn("private-marker", result.stdout + result.stderr)
        self.assertNotIn("/synthetic-private/", result.stdout + result.stderr)
        self.assertNotIn("v0.1.0", result.stdout + result.stderr)

    def test_public_failures_require_opt_in_and_exact_committed_crate_target_and_name(self):
        # A tiny committed public-source stand-in proves that dirty declarations
        # and another crate cannot expand the log disclosure allowlist.
        fixture = self.root / "source"
        (fixture / "tests").mkdir(parents=True)
        for name in ("run-tests.py", "run-tests.sh"):
            shutil.copy2(ROOT / "tests" / name, fixture / "tests" / name)
        (fixture / "Cargo.lock").write_text('name = "gust-ops"\n')
        public_name = "all_2005_files_are_extracted_even_when_preview_is_truncated"
        for directory, package, target in (("ops", "gust-ops", "archive_complete"),
                                            ("private", "private_marker", "private_marker")):
            crate = fixture / "crates" / directory
            (crate / "tests").mkdir(parents=True)
            (crate / "Cargo.toml").write_text('[package]\nname = "' + package + '"\n')
            (crate / "tests" / (target + ".rs")).write_text("#[test]\nfn " + public_name + "() {}\n")
        commands = (["git", "init", "--initial-branch=fixture"], ["git", "add", "."],
                    ["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                     "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null",
                     "commit", "-m", "Synthetic public source"])
        for command in commands:
            subprocess.run(command, cwd=fixture, capture_output=True, check=True)
        source = fixture / "crates/ops/tests/archive_complete.rs"
        source.write_text(source.read_text() + "#[test]\nfn private_marker() {}\n")
        for public_ids in (False, True):
            result, summary = self.run_case("public-ids", public_ids=public_ids,
                                            runner=fixture / "tests/run-tests.sh")
            self.assertEqual(result.returncode, 1)
            self.assertEqual(summary["cargo_exit"], 23)
            self.assertEqual(summary["failed"], 4)
            self.assertEqual(summary["diagnostics"]["failed_test_ids"], [{
                "crate": "gust-ops", "target": "archive_complete",
                "test": "all_2005_files_are_extracted_even_when_preview_is_truncated",
            }] if public_ids else [])
            self.assertEqual(summary["diagnostics"]["unclassified_test_failures"], 3 if public_ids else 4)
            self.assertNotIn("private_marker", result.stdout + result.stderr)
            self.assertNotIn("private-marker", result.stdout + result.stderr)
            self.assertNotIn("/synthetic-private/", result.stdout + result.stderr)


class FixtureContainmentTests(unittest.TestCase):
    def test_missing_section_fails_before_desktop_setup(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            runner = root / "qa-one.sh"
            shutil.copy2(ROOT / "tests/qa-one.sh", runner)
            # Trap any attempt to enter the desktop harness on the test host.
            (root / "qa-lib.sh").write_text('touch "$1/setup-started"\nexit 0\n')
            for section in (root / "missing.sh", root):
                with self.subTest(section=section):
                    result = subprocess.run(["bash", str(runner), str(section), str(root)],
                                            capture_output=True, text=True, timeout=5)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("readable QA section", result.stderr)
                    self.assertFalse((root / "setup-started").exists())

    def test_section_disappearing_after_setup_preserves_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            runner = root / "qa-one.sh"
            shutil.copy2(ROOT / "tests/qa-one.sh", runner)
            section = root / "selected section.sh"
            section.touch()
            (root / "qa-lib.sh").write_text("""
                OUTDIR=$1
                FIXTURE=$OUTDIR/home
                LOG=$OUTDIR/gust.log
                PASS=0; FAIL=0; RESULTS=()
                start_gust() { mv -- "$SECTION" "$SECTION.moved"; }
                stop_gust() { :; }
                bad() { FAIL=$((FAIL+1)); RESULTS+=("FAIL $1"); }
                finish_fixture() { touch "$OUTDIR/cleanup-called"; }
            """)
            result = subprocess.run(["bash", str(runner), str(section), str(root)],
                                    capture_output=True, timeout=5)
            self.assertNotEqual(result.returncode, 0)
            self.assertTrue((root / "selected section.sh.moved").is_file())
            self.assertIn(str(section), (root / "results.txt").read_text())
            self.assertFalse((root / "cleanup-called").exists())

    def test_failed_results_write_preserves_passing_fixture(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            runner = root / "qa-one.sh"
            shutil.copy2(ROOT / "tests/qa-one.sh", runner)
            section = root / "pass.sh"
            section.write_text('PASS=1; RESULTS=("PASS section ran")\n')
            (root / "qa-lib.sh").write_text("""
                OUTDIR=$1
                FIXTURE=$OUTDIR/home
                LOG=$OUTDIR/gust.log
                FAIL=0
                start_gust() { mkdir "$FIXTURE"; }
                stop_gust() { :; }
                finish_fixture() { rmdir "$FIXTURE"; }
            """)
            for failure in ("open", "write"):
                with self.subTest(failure=failure):
                    out = root / failure
                    out.mkdir()
                    results = out / "results.txt"
                    if failure == "open":
                        results.mkdir()
                    else:
                        results.symlink_to("/dev/full")
                    result = subprocess.run(["bash", str(runner), str(section), str(out)],
                                            capture_output=True, text=True, timeout=5)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn(str(results), result.stderr)
                    self.assertTrue((out / "home").is_dir())

    def test_fixture_ending_in_newline_is_distinct_from_its_sibling(self):
        with tempfile.TemporaryDirectory(prefix="gust-fresh-fixture-") as temporary:
            parent = Path(temporary)
            sibling = parent / "fixture"
            sibling.write_text("outside fixture")
            destination = parent / "fixture\n"
            result = subprocess.run([str(ROOT / "tests/make-fixture.sh"), str(destination)],
                                    capture_output=True, timeout=20)
            self.assertEqual(result.returncode, 0)
            self.assertEqual(len(list((destination / "big").iterdir())), 5000)
            self.assertEqual(sibling.read_text(), "outside fixture")
            self.assertEqual(destination.stat().st_mode & 0o777, 0o700)

    def test_runner_returns_builder_failure_before_normalizing(self):
        with tempfile.TemporaryDirectory(dir="/tmp") as temporary:
            root = Path(temporary)
            builder = root / "make-fixture.sh"
            builder.write_text('#!/bin/sh\nprintf "%s" "$1" > "$HERE/received"\nexit 23\n')
            builder.chmod(0o700)
            spelling = f"{root}/../{root.name}/"
            result = subprocess.run(["bash", "-uc", """
                source "$1"
                export HERE=$2
                GUST_FIXTURE=$3
                prepare_fixture
                status=$?
                rmdir -- "$qa_run_dir" || exit 1
                exit "$status"
            """, "qa-fixture-test", str(ROOT / "tests/qa-fixture.sh"), temporary, spelling],
                                    capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 23)
            self.assertEqual((root / "received").read_text(), spelling)

    def test_runner_normalizes_trailing_slash_and_preserves_fixture_siblings(self):
        with tempfile.TemporaryDirectory(prefix="gust-qa-siblings-", dir="/tmp") as temporary:
            parent = Path(temporary)
            home = parent / "explicit home"
            home.mkdir()
            siblings = ["panes", "peek", "perm", "arch", "bulk", "openwith", "multiopen", "clip"]
            for name in siblings:
                sibling = parent / f"{home.name}-{name}"
                sibling.mkdir()
                (sibling / "sentinel").write_text(name)

            subprocess.run(["bash", "-euc", """
                HERE=$1
                GUST_FIXTURE="$2/"
                unset GUST_KEEP_FIXTURE
                source "$HERE/qa-fixture.sh"
                prepare_fixture
                test "$FIXTURE" = "$2"
                for tree in "$PANEDIR" "$QLDIR" "$DELDIR" "$ARCDIR" "$BULKDIR" "$OWDIR" "$MODIR" "$CLIPDIR"; do
                    test "$(dirname -- "$tree")" = "$qa_run_dir"
                    mkdir -- "$tree"
                    rmdir -- "$tree"
                done
                prepare_pane_fixture
                test "$(cat "$PANEDIR/paneL/p1.txt")" = one
                test "$(cat "$PANEDIR/paneL/p2.txt")" = two
                finish_fixture
                # If cleanup namespaces are unavailable, remove only the
                # files and empty directories that this test just created.
                if [[ -d $qa_run_dir ]]; then
                    rm -- "$PANEDIR/paneL/p1.txt" "$PANEDIR/paneL/p2.txt"
                    rmdir -- "$PANEDIR/paneL" "$PANEDIR/paneR" "$PANEDIR" "$qa_run_dir"
                fi
            """, "qa-fixture-test", str(ROOT / "tests"), str(home)], check=True, timeout=20)

            self.assertTrue((home / "Projects/omarchy/README.md").is_file())
            for name in siblings:
                self.assertEqual((parent / f"{home.name}-{name}" / "sentinel").read_text(), name)

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

    def test_fixture_cleanup_preserves_mounts_and_checks_directory_identity(self):
        namespace = ["unshare", "--user", "--map-root-user", "--mount", "--propagation", "private"]
        cleanup_namespace = ["unshare", "--user", "--map-current-user", "--mount", "--propagation", "private"]
        if (not shutil.which("unshare")
                or subprocess.run([*namespace, *cleanup_namespace, "true"], capture_output=True).returncode):
            self.skipTest("private user and mount namespaces unavailable")
        with tempfile.TemporaryDirectory(prefix="gust-fixture-cleanup-") as temporary:
            parent = Path(temporary)
            run = parent / "run"
            mountpoint = run / "nested mount"
            mountpoint.mkdir(parents=True)
            peer = parent / "peer"
            peer.mkdir()
            (peer / "sentinel").write_text("outside")
            (run / "local").write_text("inside")
            subprocess.run([*namespace, "bash", "-euc", """
                source "$1"
                unset GUST_KEEP_FIXTURE
                qa_run_dir="$2/run"
                qa_run_dir_id=$(stat -c '%d:%i' -- "$qa_run_dir")
                mount --bind "$2/peer" "$qa_run_dir/nested mount"
                finish_fixture
                test -f "$qa_run_dir/local"
                test -f "$2/peer/sentinel"
                umount "$qa_run_dir/nested mount"

                mv "$qa_run_dir" "$2/original"
                mkdir "$qa_run_dir"
                finish_fixture
                test -d "$qa_run_dir"
                rmdir "$qa_run_dir"
                mv "$2/original" "$qa_run_dir"
                finish_fixture
                test ! -e "$qa_run_dir"
            """, "fixture-cleanup", str(ROOT / "tests/qa-fixture.sh"), temporary],
                           check=True, timeout=20)
            self.assertEqual((peer / "sentinel").read_text(), "outside")

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


spec = importlib.util.spec_from_file_location("optimization_qa", Path(__file__).with_name("qa-optimization.py"))
qa = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qa)

class OptimizationAdmitted(Exception):
    pass

class OptimizationArguments(unittest.TestCase):
    def test_bounded_search_defaults_and_explicit_limit(self):
        for journey in ("--projection-search-jobs", "--projection-search-operations"):
            for entries in (None, 99999, 100000):
                with self.subTest(journey=journey, entries=entries):
                    argv = ["qa-optimization.py", "--binary", "/synthetic/gust", "--output", "/tmp/gust-synthetic", journey]
                    if entries is not None:
                        argv += ["--entries", str(entries)]
                    captured = []
                    parse = argparse.ArgumentParser.parse_args
                    def capture(parser, *args, **kwargs):
                        result = parse(parser, *args, **kwargs)
                        captured.append(result)
                        return result
                    # Path.resolve is the first external input check after argument admission.
                    with patch.object(argparse.ArgumentParser, "parse_args", capture), patch.object(sys, "argv", argv), patch.object(Path, "resolve", side_effect=OptimizationAdmitted):
                        if entries == 100000:
                            with self.assertRaises(SystemExit) as failure:
                                qa.main()
                            self.assertEqual(failure.exception.code, 2)
                        else:
                            with self.assertRaises(OptimizationAdmitted):
                                qa.main()
                            self.assertEqual(captured[0].entries, 99999)
    def test_timing_fixture_still_admits_100k(self):
        argv = ["qa-optimization.py", "--binary", "/synthetic/gust", "--output", "/tmp/gust-synthetic", "--projection", "--entries", "100000"]
        with patch.object(sys, "argv", argv), patch.object(Path, "resolve", side_effect=OptimizationAdmitted):
            with self.assertRaises(OptimizationAdmitted):
                qa.main()


if __name__ == "__main__":
    unittest.main(verbosity=2)
