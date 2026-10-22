#!/usr/bin/env python3
"""Check connection packaging metadata without sourcing/building/installing it.

Run: python3 tests/check-setup-packaging.py
Only Python's standard library is needed; no device or network access occurs.
"""

from pathlib import Path
import json
import os
import re
import shlex
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
MANIFESTS = (ROOT / "PKGBUILD", ROOT / "docs/release/PKGBUILD")
REQUIRED = {
    "openssh", "sshfs", "fuse3", "rclone", "gvfs", "gvfs-afc",
    "gvfs-smb", "gvfs-nfs", "gvfs-mtp", "gvfs-gphoto2", "gvfs-dnssd", "usbmuxd", "polkit",
}
# Baseline feature helpers are required; optional richer codecs/converters
# remain explicit capability prerequisites rather than implied qualification.
FEATURES = {
    "wl-clipboard", "xdg-terminal-exec", "xdg-utils", "zoxide", "bubblewrap",
    "util-linux", "imagemagick", "ffmpegthumbnailer", "ffmpeg",
    "pipewire-audio", "poppler", "libarchive", "udisks2", "xdg-desktop-portal",
    "tar", "zstd", "shared-mime-info", "curl", "openssl", "mpv", "glib2",
}
# Tailscale is a network service with its own account; Gust lists its
# devices when the user already runs it and never installs it for them.
USER_CHOSEN = {"tailscale", "libraw", "perl-image-exiftool", "libreoffice-fresh"}


def shell_array(source: str, name: str) -> list[str]:
    """Read literal package arrays, never execute the PKGBUILD."""
    match = re.search(rf"^{name}=\((.*?)\)[ \t]*(?:#.*)?$", source,
                      re.MULTILINE | re.DOTALL)
    if not match:
        raise AssertionError(f"missing {name} array")
    return shlex.split(match.group(1), comments=True)


def package_names(entries: list[str]) -> set[str]:
    return {re.split(r"[<>=]", entry.split(":", 1)[0], maxsplit=1)[0]
            for entry in entries}


def check_no_install_side_effects(source: str) -> None:
    # Comments describe explicit user actions and must not be treated as commands.
    lines = []
    for line in source.replace("\\\n", " ").splitlines():
        lexer = shlex.shlex(line, posix=False)
        lexer.whitespace_split = True
        lines.append(" ".join(lexer))
    active = "\n".join(lines)
    if re.search(r"^\s*install\s*=", active, re.MULTILINE):
        raise AssertionError("package must not register an install script")
    functions = set(re.findall(r"^\s*([a-zA-Z_]\w*)\s*\(\)\s*\{", active,
                               re.MULTILINE))
    if functions - {"prepare", "build", "check", "package"}:
        raise AssertionError("package must not define install/lifecycle hooks")
    if re.search(r"--(?:default|file-dialogs)\b|"
                 r"(?:^|[;&|])\s*(?:systemctl|pacman|pkexec|usbmuxd|"
                 r"idevicepair|ifuse|sshfs|rclone|mount|umount)\b|"
                 r"\bgio\s+mount\b", active, re.MULTILINE):
        raise AssertionError("package must not change defaults, services, pairing or mounts")
    match = re.search(r"^package\(\)\s*\{(.*?)^\}", active,
                      re.MULTILINE | re.DOTALL)
    if not match:
        raise AssertionError("missing package function")
    for line in match.group(1).splitlines():
        # One fixed compatibility condition allows older source snapshots.
        # It cannot evaluate arbitrary input or execute another command.
        if line.strip() in {
            'if [[ -f "crates/ops/src/bin/gust-archive-helper.rs" ]]; then', 'fi'
        }:
            continue
        tokens = shlex.split(line)
        if not tokens:
            continue
        if re.search(r"\$\(|`|[;&|<>]", line):
            raise AssertionError("package staging must not execute embedded commands")
        if tokens[0] == "cd":
            continue
        if tokens[0] != "install" or not tokens[-1].startswith("$pkgdir/usr/"):
            raise AssertionError("package function may only stage files under $pkgdir/usr")
        if ".." in Path(tokens[-1]).parts or any(
            part in {"hooks", "systemd"} for part in Path(tokens[-1]).parts
        ) or tokens[-1].endswith(".install"):
            raise AssertionError("package must not stage install/service hooks")


class SetupPackagingTests(unittest.TestCase):
    def test_release_recipe_is_an_exact_copy_of_canonical_recipe(self):
        self.assertEqual(MANIFESTS[0].read_bytes(), MANIFESTS[1].read_bytes())

    def test_source_is_pinned_to_real_repository_and_checksum(self):
        source = MANIFESTS[0].read_text()
        self.assertRegex(source, r"(?m)^_commit=[0-9a-f]{40}$")
        self.assertIn('url="https://github.com/Zeus-Deus/gust"', source)
        self.assertIn('https://codeload.github.com/Zeus-Deus/gust/tar.gz/$_commit', source)
        self.assertRegex(source, r"(?m)^sha256sums=\('[0-9a-f]{64}'\)$")
        self.assertNotIn("<owner>", source)

    def test_qt_qualification_floor_is_declared_in_both_recipes(self):
        for manifest in MANIFESTS:
            dependencies = shell_array(manifest.read_text(), "depends")
            self.assertIn("qt6-base>=6.11.2", dependencies)
            self.assertIn("qt6-declarative>=6.11.2", dependencies)

    def test_installed_dependency_gate_invokes_versioned_constraints(self):
        with tempfile.TemporaryDirectory(prefix="gust-dependency-test-") as temporary:
            root = Path(temporary)
            pacman = root / "pacman"
            pacman.write_text("""#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
Path(os.environ['CAPTURE']).write_text(json.dumps(sys.argv[1:]))
# Simulate an installed version failing the declared requirement. No runtime
# helper fixtures, especially absolute helper paths, are created or executed.
sys.exit(1)
""")
            pacman.chmod(0o755)
            capture = root / "arguments"
            result = subprocess.run([sys.executable, str(ROOT / "packaging/check-dependencies.py"),
                "--installed"], env=dict(os.environ, PATH=str(root) + os.pathsep + os.environ["PATH"],
                CAPTURE=str(capture)), capture_output=True, text=True, timeout=15)
            self.assertEqual(result.returncode, 1)
            self.assertEqual(json.loads(result.stdout)["status"], "fail")
            invoked = json.loads(capture.read_text())
            self.assertEqual(invoked[0], "-T")
            self.assertIn("qt6-base>=6.11.2", invoked)
            self.assertIn("qt6-declarative>=6.11.2", invoked)
            self.assertIn("glib2>=2.66", invoked)
            self.assertNotIn(str(root), result.stdout + result.stderr)

    def test_connection_helpers_are_required_in_both_manifests(self):
        for manifest in MANIFESTS:
            with self.subTest(manifest=str(manifest.relative_to(ROOT))):
                required = package_names(shell_array(manifest.read_text(), "depends"))
                self.assertFalse(REQUIRED - required,
                                 f"missing required helpers: {sorted(REQUIRED - required)}")

    def test_feature_helpers_are_required_in_both_manifests(self):
        for manifest in MANIFESTS:
            with self.subTest(manifest=str(manifest.relative_to(ROOT))):
                required = package_names(shell_array(manifest.read_text(), "depends"))
                self.assertFalse(FEATURES - required,
                                 f"missing feature helpers: {sorted(FEATURES - required)}")

    def test_optional_tools_are_explicit_capabilities(self):
        for manifest in MANIFESTS:
            with self.subTest(manifest=str(manifest.relative_to(ROOT))):
                optional = package_names(shell_array(manifest.read_text(), "optdepends"))
                self.assertEqual(optional, USER_CHOSEN)

    def test_required_helpers_are_not_duplicated_in_optional_dependencies(self):
        for manifest in MANIFESTS:
            with self.subTest(manifest=str(manifest.relative_to(ROOT))):
                source = manifest.read_text()
                required = package_names(shell_array(source, "depends"))
                optional = package_names(shell_array(source, "optdepends"))
                self.assertFalse(REQUIRED & optional,
                                 f"connection helpers remain optional: {sorted(REQUIRED & optional)}")
                self.assertFalse(required & optional,
                                 f"required/optional overlap: {sorted(required & optional)}")

    def test_packages_only_stage_files_without_install_side_effects(self):
        for manifest in MANIFESTS:
            with self.subTest(manifest=str(manifest.relative_to(ROOT))):
                check_no_install_side_effects(manifest.read_text())

    def test_side_effect_guard_rejects_hooks_and_automatic_actions(self):
        baseline = MANIFESTS[0].read_text()
        mutations = (
            baseline + "\ninstall=gust.install\n",
            baseline + "\npost_install() { true; }\n",
            baseline.replace('  cd "$pkgname-$_commit"', '  systemctl enable usbmuxd', 1),
            baseline.replace('  cd "$pkgname-$_commit"', '  gust --default on', 1),
            baseline.replace('  cd "$pkgname-$_commit"', '  gio mount afc://phone', 1),
            baseline.replace('  cd "$pkgname-$_commit"', '  idevicepair pair', 1),
            baseline.replace('"$pkgdir/usr/bin/gust"', '"/usr/bin/gust"'),
        )
        for index, mutation in enumerate(mutations):
            with self.subTest(mutation=index):
                with self.assertRaises(AssertionError):
                    check_no_install_side_effects(mutation)


if __name__ == "__main__":
    unittest.main(verbosity=2)
