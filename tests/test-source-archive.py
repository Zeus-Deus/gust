#!/usr/bin/env python3
"""Exercise declared source bytes, extraction containment and tooling admission."""

import hashlib
import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("source_check", ROOT / "packaging/check-source.py")
source = importlib.util.module_from_spec(spec)
spec.loader.exec_module(source)
REVISION = "a" * 40
PREFIX = "gust-" + REVISION
REQUIRED = ("tests/run-tests.py", "packaging/build-artifact.py", "packaging/check-package.py",
            "packaging/check-source.py", "packaging/runtime-files.json", "crates/ops/src/bin/gust-archive-helper.rs")


def fixture(*, omit=None, extra=None, link="../../ui"):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w:gz") as archive:
        files = {"Cargo.toml": b'[workspace.package]\nversion = "0.1.0"\n', "ui/Main.qml": b"// fixture\n"}
        files.update({name: b"# synthetic source\n" for name in REQUIRED if name != omit})
        if extra:
            files[extra] = b"sentinel replacement"
        for name, content in files.items():
            member = tarfile.TarInfo(PREFIX + "/" + name)
            member.size = len(content)
            archive.addfile(member, io.BytesIO(content))
        member = tarfile.TarInfo(PREFIX + "/crates/app/ui")
        member.type = tarfile.SYMTYPE
        member.linkname = link
        archive.addfile(member)
    data = output.getvalue()
    return data, {"revision": REVISION, "sha256": hashlib.sha256(data).hexdigest(), "version": "0.1.0"}


class SourceArchiveTests(unittest.TestCase):
    def test_correct_bytes_extract_with_canonical_qml_link(self):
        data, record = fixture()
        with tempfile.TemporaryDirectory(prefix="gust-source-test-") as temporary:
            root = source.extract(data, Path(temporary), record)
            self.assertEqual((root / "crates/app/ui/Main.qml").read_bytes(), b"// fixture\n")
            self.assertTrue((root / REQUIRED[-1]).is_file())

    def test_changed_remote_bytes_cannot_satisfy_declared_checksum(self):
        data, record = fixture()
        with self.assertRaises(ValueError):
            source.validate(data + b"changed", record)

    def test_archive_requires_builder_runner_and_helper(self):
        for missing in REQUIRED:
            with self.subTest(missing=missing):
                data, record = fixture(omit=missing)
                with self.assertRaises((KeyError, ValueError)):
                    source.validate(data, record)

    def test_version_metadata_must_match_archive(self):
        data, record = fixture()
        record["version"] = "0.2.0"
        with self.assertRaises(ValueError):
            source.validate(data, record)

    def test_traversal_and_unaccounted_links_are_refused_before_extraction(self):
        with tempfile.TemporaryDirectory(prefix="gust-source-containment-") as temporary:
            root = Path(temporary)
            sentinel = root / "keep"
            sentinel.write_text("preserve outside data")
            for options in ({"extra": "../keep"}, {"extra": "crates/app/ui/Main.qml"}, {"link": "../../../outside"}):
                with self.subTest(options=options):
                    data, record = fixture(**options)
                    destination = root / "out"
                    with self.assertRaises(ValueError):
                        source.extract(data, destination, record)
                    self.assertFalse(destination.exists())
                    self.assertEqual(sentinel.read_text(), "preserve outside data")

    def test_recipe_must_use_exact_immutable_url_and_consistent_version(self):
        recipe = (ROOT / "PKGBUILD").read_text()
        self.assertTrue(source.metadata(recipe)["url"].startswith("https://codeload.github.com/Zeus-Deus/gust/tar.gz/"))
        for mutated in (recipe.replace("codeload.github.com", "example.invalid"),
                        recipe.replace("pkgver=0.1.0", "pkgver=invalid")):
            with self.assertRaises(ValueError):
                source.metadata(mutated)


if __name__ == "__main__":
    unittest.main(verbosity=2)
