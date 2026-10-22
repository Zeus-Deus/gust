#!/usr/bin/env python3
"""Fail on unmapped literal subprocess helpers or missing Arch providers.

This catches literal Command::new/thumbnail Helper additions. Computed argv,
configured applications and transitive backend helpers still need review.
--installed additionally checks Arch package and runtime helper availability.
"""

import argparse
import importlib.util
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("setup_packaging", ROOT / "tests/check-setup-packaging.py")
setup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(setup)


def check(source=ROOT):
    policy = json.loads((source / "packaging/helper-packages.json").read_text())
    known = set().union(*(section.keys() for section in policy.values()))
    found = set()
    for path in (source / "crates").glob("*/src/**/*.rs"):
        production = path.read_text().split("#[cfg(test)]")[0]
        found.update(re.findall(r'Command::new\("([a-zA-Z0-9_-]+)"\)', production))
        found.update(re.findall(r'binary: "([a-zA-Z0-9_-]+)"', production))
    recipe = (source / "PKGBUILD").read_text()
    required = setup.package_names(setup.shell_array(recipe, "depends"))
    optional = setup.package_names(setup.shell_array(recipe, "optdepends"))
    if found - known:
        raise ValueError("unmapped production helper")
    if set(policy["required"].values()) - required:
        raise ValueError("runtime helper package missing from depends")
    if set(policy["optional"].values()) - optional:
        raise ValueError("optional helper package missing from optdepends")
    return policy, required


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--installed", action="store_true")
    args = parser.parse_args()
    try:
        policy, required = check()
        if args.installed:
            if not shutil.which("pacman"):
                print(json.dumps({"status": "skip", "reason": "Arch package database unavailable"}))
                return 77
            # Retain declared versions; package-name presence cannot qualify Qt.
            requirements = setup.shell_array((ROOT / "PKGBUILD").read_text(), "depends")
            result = subprocess.run(["pacman", "-T", *requirements],
                                    capture_output=True, text=True, timeout=30)
            if result.returncode or any(not shutil.which(name) for name in policy["required"]):
                raise ValueError("installed runtime dependency missing")
    except (OSError, ValueError, AssertionError, subprocess.SubprocessError):
        print(json.dumps({"status": "fail", "reason": "dependency metadata/runtime drift"}))
        return 1
    print(json.dumps({"status": "pass", "helper_contracts": sum(map(len, policy.values())),
                      "installed_dependencies_checked": args.installed}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
