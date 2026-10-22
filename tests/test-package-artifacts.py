#!/usr/bin/env python3
"""Prove that real staged runtime drift and helper omissions are rejected."""

import importlib.util
import hashlib
import json
from pathlib import Path
import shutil
import tempfile
import unittest

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
        self.assertEqual(len(package.runtime_files(self.source)), 7)

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
        self.assertEqual(len(package.verify(staged, self.source, "x86_64", require_receipt=False)), 7)
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
                  "executables": {"gust": hashlib.sha256(
                      (self.source / "target/release/gust").read_bytes()).hexdigest()}}
        (self.source / "target/release/gust-build.json").write_text(json.dumps(record))
        self.assertEqual(len(package.verify(self.stage, self.source, "x86_64")), 6)
        outside = self.root / "different-qml"
        outside.mkdir()
        (outside / "Main.qml").write_text("// redirected input\n")
        link.unlink()
        link.symlink_to(outside, target_is_directory=True)
        with self.assertRaises(ValueError):
            package.verify(self.stage, self.source, "x86_64")


if __name__ == "__main__":
    unittest.main(verbosity=2)
