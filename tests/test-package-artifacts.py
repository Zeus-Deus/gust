#!/usr/bin/env python3
"""Prove that real staged runtime drift and helper omissions are rejected."""

import importlib.util
import hashlib
import io
import json
import fcntl
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("package_artifacts", ROOT / "packaging/check-package.py")
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class PackageArtifactTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="gust-artifact-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source = self.root / "source"
        self.stage = self.root / "installed"
        (self.source / "packaging").mkdir(parents=True)
        shutil.copyfile(ROOT / "packaging/runtime-files.json", self.source / "packaging/runtime-files.json")
        # Synthetic ELF header: the checker never executes this fixture.
        elf = b"\x7fELF\x02\x01" + b"\0" * 12 + (62).to_bytes(2, "little") + b"fixture"
        (self.source / "packaging/gust-chooser.desktop").write_bytes(b"synthetic configuration\n")
        for item in package.runtime_files(self.source):
            built = self.source / item["source"]
            built.parent.mkdir(parents=True, exist_ok=True)
            built.write_bytes(elf if item["path"].startswith("usr/bin/") else b"synthetic configuration\n")
            installed = self.stage / item["path"]
            installed.parent.mkdir(parents=True, exist_ok=True)
            installed.write_bytes(built.read_bytes())
            installed.chmod(int(item["mode"], 8))

    def verify(self):
        return package.verify(self.stage, self.source, "x86_64", require_receipt=False)

    def test_complete_stage_matches_sources(self):
        self.assertEqual(len(self.verify()), 7)

    def test_a_snapshot_older_than_the_dialog_entry_still_packages(self):
        (self.source / "packaging/gust-chooser.desktop").unlink()
        (self.stage / "usr/share/applications/gust-chooser.desktop").unlink()
        self.assertEqual(len(self.verify()), 6)

    def test_missing_runtime_file_and_modified_configuration_fail(self):
        path = self.stage / "usr/share/applications/gust.desktop"
        path.write_text("unexpected configuration")
        with self.assertRaises(ValueError):
            self.verify()
        path.unlink()
        with self.assertRaises(OSError):
            self.verify()

    def test_symlink_to_outside_file_is_refused(self):
        path = self.stage / "usr/bin/gust"
        path.unlink()
        outside = self.root / "outside"
        outside.write_bytes((self.source / "target/release/gust").read_bytes())
        path.symlink_to(outside)
        with self.assertRaises(ValueError):
            self.verify()
        self.assertTrue(outside.is_file())

    def test_wrong_mode_and_architecture_are_refused(self):
        path = self.stage / "usr/bin/gust"
        path.chmod(0o4755)
        with self.assertRaises(ValueError):
            self.verify()
        path.chmod(0o755)
        with self.assertRaises(ValueError):
            package.verify(self.stage, self.source, "aarch64", require_receipt=False)

    def test_an_unrecorded_binary_cannot_qualify_as_installed(self):
        with self.assertRaises(OSError):
            package.verify(self.stage, self.source, "x86_64")

    def test_source_archive_identity_does_not_require_a_git_checkout(self):
        revision = "a" * 40
        archive = self.root / ("gust-" + revision)
        archive.mkdir()
        self.assertEqual(package.build.source_revision(archive), revision)
        self.assertEqual(package.build.source_revision(self.source), "unversioned-source")

    def test_archive_helper_is_required_when_source_has_entrypoint(self):
        helper = self.source / "crates/ops/src/bin/gust-archive-helper.rs"
        helper.parent.mkdir(parents=True)
        helper.write_text("// synthetic entrypoint\n")
        with self.assertRaises(OSError):
            self.verify()
        self.assertEqual(len(package.runtime_files(self.source)), 8)

    def test_actual_recipe_refuses_missing_helper_then_stages_it_beside_gust(self):
        helper = self.source / "crates/ops/src/bin/gust-archive-helper.rs"
        helper.parent.mkdir(parents=True)
        helper.write_text("// synthetic entrypoint\n")
        missing = self.root / "missing-helper"
        missing.mkdir()
        with self.assertRaises(package.subprocess.CalledProcessError):
            package.stage(missing, self.source)
        shutil.copyfile(self.source / "target/release/gust", self.source / "target/release/gust-archive-helper")
        complete = self.root / "complete-helper"
        complete.mkdir()
        staged = package.stage(complete, self.source)
        self.assertEqual(len(package.verify(staged, self.source, "x86_64", require_receipt=False)), 8)
        self.assertEqual((staged / "usr/bin/gust-archive-helper").stat().st_mode & 0o777, 0o755)

    def test_redirected_qml_input_invalidates_real_receipt_check(self):
        (self.source / "Cargo.toml").write_text("[workspace]\n")
        (self.source / "Cargo.lock").write_text("# synthetic lock\n")
        (self.source / "ui").mkdir()
        (self.source / "ui/Main.qml").write_text("// original input\n")
        (self.source / "crates/app").mkdir(parents=True)
        link = self.source / "crates/app/ui"
        link.symlink_to("../../ui", target_is_directory=True)
        record = {"source_sha256": package.build.source_digest(self.source),
                  "qt": {"version": "6.11.2"},
                  "executables": {"gust": hashlib.sha256(
                      (self.source / "target/release/gust").read_bytes()).hexdigest()}}
        (self.source / "target/release/gust-build.json").write_text(json.dumps(record))
        self.assertEqual(len(package.verify(self.stage, self.source, "x86_64")), 7)
        outside = self.root / "different-qml"
        outside.mkdir()
        (outside / "Main.qml").write_text("// redirected input\n")
        link.unlink()
        link.symlink_to(outside, target_is_directory=True)
        with self.assertRaises(ValueError):
            package.verify(self.stage, self.source, "x86_64")


class QtContractTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="gust-qt-contract-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.sdk = self.root / "sdk"
        (self.sdk / "lib").mkdir(parents=True)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        qmake = self.bin / "qmake"
        qmake.write_text("""#!/usr/bin/env python3
import os, sys
print({'QT_VERSION': os.environ['QT_FIXTURE_VERSION'],
       'QT_INSTALL_PREFIX': os.environ['QT_FIXTURE_ROOT'],
       'QT_INSTALL_LIBS': os.environ['QT_FIXTURE_ROOT'] + '/lib'}[sys.argv[2]])
""")
        qmake.chmod(0o755)
        self.env = dict(os.environ, QMAKE=str(qmake), QT_FIXTURE_VERSION="6.11.2",
                        QT_FIXTURE_ROOT=str(self.sdk), GUST_EXPECT_QT_ROOT=str(self.sdk))

    def test_explicit_qmake_accepts_selected_sdk_and_rejects_old_or_wrong_sdk(self):
        with patch.dict(os.environ, self.env, clear=True):
            self.assertEqual(package.build.qt.selected()["version"], "6.11.2")
            os.environ["QT_FIXTURE_VERSION"] = "6.4.2"
            with self.assertRaises(ValueError):
                package.build.qt.selected()
            os.environ["QT_FIXTURE_VERSION"] = "6.12.0"
            with self.assertRaises(ValueError):
                package.build.qt.selected()
            os.environ["QT_FIXTURE_VERSION"] = "6.11.2"
            os.environ["GUST_EXPECT_QT_ROOT"] = str(self.root / "other")
            with self.assertRaises(ValueError):
                package.build.qt.selected()

    def test_broken_explicit_qmake_never_falls_back_to_system(self):
        with patch.dict(os.environ, self.env, clear=True):
            os.environ["QMAKE"] = str(self.root / "missing")
            with self.assertRaises(ValueError):
                package.build.qt.selected()

    def test_installer_failure_exposes_only_fixed_classes_flags_and_exit(self):
        log = self.root / "installer.log"
        log.write_text("aqt.exceptions.ArchiveExtractionError: private-marker /private/host/file\n"
                       "ValueError: private-marker https://private.invalid/secret\n"
                       "UnlistedPrivateError: host-identity\nNo matching distribution found\n")
        result = subprocess.run([sys.executable, str(ROOT / "packaging/qt-contract.py"),
            "--diagnose-installer", str(log), "--phase", "sdk", "--exit-code", "1"],
            capture_output=True, text=True, timeout=15)
        self.assertEqual(result.returncode, 0)  # Formatter; caller retains failure.
        summary = json.loads(result.stdout)
        self.assertEqual(summary["status"], "fail")
        self.assertEqual(summary["installer_exit"], 1)
        self.assertEqual(summary["exception_classes"], ["ArchiveExtractionError", "ValueError"])
        self.assertTrue(summary["pip_distribution_unavailable"])
        for private in ("private-marker", "/private/", "host-identity", "UnlistedPrivateError", "https:", str(self.root)):
            self.assertNotIn(private, result.stdout + result.stderr)

    def test_required_app_gate_reports_old_qt_as_skip_and_fails_without_building(self):
        env = dict(self.env, QT_FIXTURE_VERSION="6.4.2")
        result = subprocess.run([sys.executable, str(ROOT / "tests/release-gates.py"),
            "--gate", "app", "--required", "--output", str(self.root / "evidence")],
            env=env, capture_output=True, text=True, timeout=15)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(json.loads(result.stdout)["status"], "SKIP")
        self.assertNotIn(str(self.root), result.stdout + result.stderr)
        self.assertFalse((self.root / "evidence/app.log").exists())

    def linkage(self, libraries, *, sdk=True, build_version="6.11.2"):
        # Run an actual subprocess boundary with controlled loader output.
        # These are protocol fixtures, never claimed as compiled Qt binaries.
        source = self.root / "source"
        (source / "target/release").mkdir(parents=True, exist_ok=True)
        (source / "packaging").mkdir(exist_ok=True)
        shutil.copyfile(ROOT / "packaging/runtime-files.json", source / "packaging/runtime-files.json")
        (source / "target/release/gust-build.json").write_text(json.dumps({
            "qt": {"version": build_version, "prefix": str(self.sdk)}}))
        ldd = self.bin / "ldd"
        ldd.write_text("#!/usr/bin/env python3\nimport os\nprint(os.environ['LDD_FIXTURE'])\n")
        ldd.chmod(0o755)
        lines = []
        for name, release, directory in libraries:
            path = directory / (name + ".so." + release)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.touch()
            lines.append(f"  {name}.so.6 => {path} (0x1234)")
        with patch.dict(os.environ, PATH=str(self.bin) + os.pathsep + os.environ["PATH"],
                        LDD_FIXTURE="\n".join(lines)):
            return package.verify_linkage(self.root, source, io.StringIO(), self.sdk if sdk else None)

    def test_staging_accepts_exact_sdk_linkage(self):
        result = self.linkage([("libQt6Core", "6.11.2", self.sdk / "lib"),
                               ("libQt6Qml", "6.11.2", self.sdk / "lib")])
        self.assertEqual(result, {"qt_libraries": 2, "qt_version": "6.11.2", "selected_sdk_checked": True})

    def test_linkage_refuses_system_fallback_and_mixed_versions(self):
        with self.assertRaises(ValueError):
            self.linkage([("libQt6Core", "6.11.2", self.root / "system")])
        with self.assertRaises(ValueError):
            self.linkage([("libQt6Core", "6.11.2", self.sdk / "lib"),
                          ("libQt6Qml", "6.12.0", self.sdk / "lib")])

    def test_installed_runtime_cannot_be_older_than_build_or_have_no_qt(self):
        for runtime, built in (("6.4.2", "6.11.2"), ("6.11.2", "6.12.0")):
            with self.subTest(runtime=runtime, built=built), self.assertRaises(ValueError):
                self.linkage([("libQt6Core", runtime, self.sdk / "lib")], sdk=False, build_version=built)
        with self.assertRaises(ValueError):
            self.linkage([])
        self.assertEqual(self.linkage([("libQt6Core", "6.12.0", self.sdk / "lib")], sdk=False)["qt_version"], "6.12.0")


class BuilderCancellationTests(unittest.TestCase):
    def test_normal_child_exits_retain_lease_until_surviving_descendants_retire(self):
        for caller in ("builder", "gate"):
            for ending in ("0", "7", "signal"):
                with self.subTest(caller=caller, ending=ending), tempfile.TemporaryDirectory(prefix="gust-normal-retirement-") as temporary:
                    root = Path(temporary)
                    source = root / "source"
                    (source / "packaging").mkdir(parents=True)
                    (source / "tests").mkdir()
                    for name in ("build-artifact.py", "qt-contract.py", "owned-process.py"):
                        shutil.copyfile(ROOT / "packaging" / name, source / "packaging" / name)
                    shutil.copyfile(ROOT / "tests/release-gates.py", source / "tests/release-gates.py")
                    (source / "Cargo.toml").write_text("[workspace]\n")
                    (source / "Cargo.lock").write_text("# synthetic source\n")
                    (source / "ui").mkdir()
                    (source / "ui/Main.qml").write_text("// synthetic QML\n")
                    (source / "crates/app").mkdir(parents=True)
                    (source / "crates/app/ui").symlink_to("../../ui", target_is_directory=True)
                    cargo = root / "cargo"
                    cargo.write_text("""#!/usr/bin/env python3
import os, subprocess, sys, time
from pathlib import Path
base = Path(os.environ['FIXTURE'])
child = subprocess.Popen([sys.executable, '-c', '''import signal, sys, time
from pathlib import Path
signal.signal(signal.SIGTERM, signal.SIG_IGN)
signal.signal(signal.SIGINT, signal.SIG_IGN)
Path(sys.argv[1]).touch()
time.sleep(30)
''', str(base / 'ready')])
while not (base / 'ready').exists(): time.sleep(0.01)
(base / 'pids').write_text(str(os.getpid()) + ' ' + str(child.pid))
while not (base / 'finish-leader').exists(): time.sleep(0.01)
if os.environ['ENDING'] == '0':
    output = base / 'source/target/release/gust'
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(b'inert artifact fixture; never executed')
sys.exit(int(os.environ['ENDING']))
""")
                    cargo.chmod(0o755)
                    wrapper = root / "invoke.py"
                    wrapper.write_text("""import importlib.util, json, sys, time
from pathlib import Path
from types import SimpleNamespace
base = Path(sys.argv[1]); caller = sys.argv[2]
path = 'packaging/build-artifact.py' if caller == 'builder' else 'tests/release-gates.py'
spec = importlib.util.spec_from_file_location('caller', base / 'source' / path)
module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
module.qt.selected = lambda: {'version': '6.11.2', 'qmake': '/synthetic/qmake'}
original = module.owned.terminate_owned_group
def observed_retirement(child):
    # Pause at the actual retirement boundary while the descendant is live,
    # so lease exclusion is deterministic, rather than a timing assertion.
    (base / 'cleanup-entry').write_text(str(child.poll()))
    while not (base / 'allow-retirement').exists(): time.sleep(0.01)
    return original(child)
module.owned.terminate_owned_group = observed_retirement
if caller == 'builder':
    code = module.main(build_lock=base / 'lease')
else:
    gates = module.Gates(SimpleNamespace(output=base / 'evidence'))
    gates.build_lock = base / 'lease'
    code = gates.command('fixture', ['cargo'], build=True)
(base / 'result.json').write_text(json.dumps({'code': code}))
""")
                    sentinel = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
                    owner = subprocess.Popen([sys.executable, str(wrapper), str(root), caller],
                        env=dict(os.environ, PATH=str(root) + os.pathsep + os.environ["PATH"],
                                 FIXTURE=str(root), ENDING=ending),
                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                    pids = []
                    try:
                        deadline = time.monotonic() + 5
                        while not (root / "pids").exists() and owner.poll() is None and time.monotonic() < deadline:
                            time.sleep(0.01)
                        pids = list(map(int, (root / "pids").read_text().split()))
                        if ending == "signal":
                            os.kill(pids[0], signal.SIGTERM)
                        else:
                            (root / "finish-leader").touch()
                        deadline = time.monotonic() + 5
                        while not (root / "cleanup-entry").exists() and owner.poll() is None and time.monotonic() < deadline:
                            time.sleep(0.01)
                        expected_child = -signal.SIGTERM if ending == "signal" else int(ending)
                        self.assertNotEqual(Path(f"/proc/{pids[1]}/stat").read_text().split()[2], "Z")
                        with (root / "lease").open("a") as lease:
                            with self.assertRaises(BlockingIOError):
                                fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
                            self.assertTrue((root / "cleanup-entry").exists(), "returned without retiring surviving descendants")
                            self.assertEqual(int((root / "cleanup-entry").read_text()), expected_child)
                            self.assertIsNone(owner.poll())
                            self.assertFalse((root / "result.json").exists())
                            self.assertFalse((source / "target/release/gust-build.json").exists())
                            (root / "allow-retirement").touch()
                            public, errors = owner.communicate(timeout=5)
                            self.assertEqual(owner.returncode, 0)
                            fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
                        expected = expected_child if caller == "gate" else (0 if ending == "0" else 1)
                        self.assertEqual(json.loads((root / "result.json").read_text())["code"], expected)
                        self.assertEqual((source / "target/release/gust-build.json").exists(), caller == "builder" and ending == "0")
                        for pid in pids:
                            state = Path(f"/proc/{pid}/stat")
                            self.assertTrue(not state.exists() or state.read_text().split()[2] == "Z")
                        self.assertIsNone(sentinel.poll())
                        self.assertNotIn(str(root), public + errors)
                    finally:
                        (root / "allow-retirement").touch()
                        if owner.poll() is None:
                            owner.kill(); owner.wait()
                        owner.communicate(timeout=5)
                        if pids:
                            try:
                                os.killpg(pids[0], signal.SIGKILL)
                            except ProcessLookupError:
                                pass
                        sentinel.terminate(); sentinel.wait()

    def test_builder_cancellation_retires_owned_group_before_private_lease_release(self):
        for cancellation, startup, cleanup in ((signal.SIGINT, False, "ordinary"),
                (signal.SIGTERM, False, "ordinary"), (signal.SIGTERM, True, "ordinary"),
                (signal.SIGTERM, False, "second-signal"), (signal.SIGTERM, False, "retirement")):
            with self.subTest(signal=cancellation, startup=startup, cleanup=cleanup), tempfile.TemporaryDirectory(prefix="gust-builder-cancel-") as temporary:
                root = Path(temporary)
                source = root / "source"
                (source / "packaging").mkdir(parents=True)
                for name in ("build-artifact.py", "qt-contract.py", "owned-process.py"):
                    shutil.copyfile(ROOT / "packaging" / name, source / "packaging" / name)
                (source / "Cargo.toml").write_text("[workspace]\n")
                (source / "Cargo.lock").write_text("# synthetic source\n")
                (source / "ui").mkdir()
                (source / "ui/Main.qml").write_text("// synthetic QML\n")
                (source / "crates/app").mkdir(parents=True)
                (source / "crates/app/ui").symlink_to("../../ui", target_is_directory=True)
                cargo = root / "cargo"
                cargo.write_text("""#!/usr/bin/env python3
import os, signal, subprocess, sys, time
from pathlib import Path
base = Path(os.environ['FIXTURE'])
def stopping(signum, frame):
    (base / 'stopping').touch()
    time.sleep(0.3)
    sys.exit(0)
signal.signal(signal.SIGTERM, stopping)
signal.signal(signal.SIGINT, stopping)
child = subprocess.Popen([sys.executable, '-c', '''import signal, sys, time
from pathlib import Path
signal.signal(signal.SIGTERM, signal.SIG_IGN)
signal.signal(signal.SIGINT, signal.SIG_IGN)
Path(sys.argv[1]).touch()
time.sleep(30)
''', str(base / 'ready')])
while not (base / 'ready').exists(): time.sleep(0.01)
(base / 'pids').write_text(str(os.getpid()) + ' ' + str(child.pid))
time.sleep(30)
""")
                cargo.chmod(0o755)
                wrapper = root / "invoke.py"
                wrapper.write_text("""import importlib.util, os, signal, sys, time
from pathlib import Path
base = Path(sys.argv[1])
spec = importlib.util.spec_from_file_location('builder', base / 'source/packaging/build-artifact.py')
module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
module.qt.selected = lambda: {'version': '6.11.2', 'qmake': '/synthetic/qmake'}
if sys.argv[3] == 'second-signal':
    original_cleanup = module.owned.terminate_owned_group
    def interrupted_cleanup(child):
        (base / 'cleanup-entry').touch()
        # The former first handler transition was outside its cleanup try.
        # Inject there deterministically; cleanup now changes no handlers.
        os.kill(os.getpid(), signal.SIGTERM)
        return original_cleanup(child)
    module.owned.terminate_owned_group = interrupted_cleanup
if sys.argv[3] == 'retirement':
    original_live = module.owned.live_group
    original_clock = module.owned.time.monotonic
    def uncertain_retirement(group):
        (base / 'retirement-pending').touch()
        return not (base / 'retirement-release').exists() or original_live(group)
    module.owned.live_group = uncertain_retirement
    # Advance beyond the former 10-second deadline as soon as observation
    # becomes uncertain, without waiting or changing another process's clock.
    module.owned.time.monotonic = lambda: original_clock() + (100 if (base / 'retirement-pending').exists() else 0)
if sys.argv[2] == 'startup':
    original = module.owned.subprocess.Popen
    def interrupted_start(*args, **kwargs):
        child = original(*args, **kwargs)
        deadline = time.monotonic() + 5
        while not (base / 'pids').exists() and time.monotonic() < deadline: time.sleep(0.01)
        os.kill(os.getpid(), signal.SIGTERM)
        return child
    module.owned.subprocess.Popen = interrupted_start
sys.exit(module.main(build_lock=base / 'lease'))
""")
                sentinel = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
                owner = subprocess.Popen([sys.executable, str(wrapper), str(root), "startup" if startup else "wait", cleanup],
                    env=dict(os.environ, PATH=str(root) + os.pathsep + os.environ["PATH"], FIXTURE=str(root)),
                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                pids = []
                try:
                    deadline = time.monotonic() + 5
                    while not (root / "pids").exists() and time.monotonic() < deadline:
                        time.sleep(0.01)
                    pids = list(map(int, (root / "pids").read_text().split()))
                    if not startup:
                        owner.send_signal(cancellation)
                    marker = "cleanup-entry" if cleanup == "second-signal" else (
                        "retirement-pending" if cleanup == "retirement" else "stopping")
                    deadline = time.monotonic() + 5
                    while not (root / marker).exists() and time.monotonic() < deadline:
                        time.sleep(0.01)
                    self.assertTrue((root / marker).exists())
                    if cleanup == "retirement":
                        time.sleep(0.1)
                    with (root / "lease").open("a") as lease:
                        with self.assertRaises(BlockingIOError):
                            fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
                        self.assertIsNone(owner.poll())
                        if cleanup == "retirement":
                            (root / "retirement-release").touch()
                        public, errors = owner.communicate(timeout=5)
                        self.assertEqual(owner.returncode, 130)
                        fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
                        for pid in pids:
                            state = Path(f"/proc/{pid}/stat")
                            self.assertTrue(not state.exists() or state.read_text().split()[2] == "Z")
                    self.assertIsNone(sentinel.poll())
                    self.assertNotIn(str(root), public + errors)
                    self.assertFalse((source / "target/release/gust-build.json").exists())
                finally:
                    (root / "retirement-release").touch()
                    if owner.poll() is None:
                        owner.kill(); owner.wait()
                    if pids:
                        try:
                            os.killpg(pids[0], signal.SIGKILL)
                        except ProcessLookupError:
                            pass
                    sentinel.terminate(); sentinel.wait()


if __name__ == "__main__":
    unittest.main(verbosity=2)
