#!/usr/bin/env python3
"""Run separately named release gates; missing prerequisites never mean PASS.

Raw evidence is private and outside the checkout. --required rejects every
SKIP, including a busy lease. No gate installs packages or provisions services.
"""

import argparse
import csv
import fcntl
import json
import os
from pathlib import Path
import re
import signal
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
GATES = ("static", "library-unit", "libraries", "app", "model-seams", "models", "installed",
         "dependencies", "native", "themes")


def terminate_owned_group(child):
    # Cancellation must finish cleanup before the lease is released. Ignore
    # further terminal/cancel signals only during this bounded cleanup.
    handlers = {sig: signal.signal(sig, signal.SIG_IGN)
                for sig in (signal.SIGINT, signal.SIGTERM)}
    try:
        try:
            os.killpg(child.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            pass
        # The group leader can exit before an uncooperative descendant.
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        child.wait()
    finally:
        for sig, handler in handlers.items():
            signal.signal(sig, handler)


def cancel_on_signal(signum, frame):
    raise KeyboardInterrupt


class Gates:
    def __init__(self, args):
        self.args = args
        self.output = args.output.resolve()
        if (self.output.is_relative_to(ROOT) or self.output.exists()
                or any((parent / ".git").exists() for parent in self.output.parents)):
            raise ValueError("output must be fresh and outside the checkout")
        self.output.mkdir(parents=True, mode=0o700)
        self.results = []
        self.installed_verified = None
        self.build_lock = Path("/tmp/gust-build.lock")

    def result(self, name, status, reason=""):
        self.results.append({"gate": name, "status": status, "reason": reason})

    def command(self, name, command, *, build=False, env=None):
        lock = None
        try:
            if build:
                lock = self.build_lock.open("a")
                try:
                    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError:
                    return 75
            with (self.output / (name + ".log")).open("w") as log:
                child_env = dict(os.environ if env is None else env)
                if build:
                    child_env.setdefault("CARGO_BUILD_JOBS", "2")
                child = subprocess.Popen(command, cwd=ROOT, env=child_env, stdout=log,
                                         stderr=subprocess.STDOUT, start_new_session=True)
                try:
                    child.wait(timeout=1800)
                except subprocess.TimeoutExpired:
                    terminate_owned_group(child)
                    return 1
                except BaseException:
                    terminate_owned_group(child)
                    raise
            return child.returncode
        except FileNotFoundError:
            return 77
        except subprocess.TimeoutExpired:
            return 1
        finally:
            if lock:
                lock.close()

    def invoke(self, name, command, **kwargs):
        code = self.command(name, command, **kwargs)
        self.result(name, "PASS" if code == 0 else "SKIP" if code in {75, 77} else "FAIL",
                    "" if code == 0 else "lease busy" if code == 75 else
                    "prerequisite unavailable" if code == 77 else "check failed")
        return code

    def cargo(self, gate):
        if not shutil.which("cargo"):
            self.result(gate, "SKIP", "Cargo unavailable")
            return
        if gate == "app" and not shutil.which("qmake6"):
            self.result(gate, "SKIP", "Qt build tools unavailable")
            return
        args = ["--app"] if gate == "app" else []
        if gate == "library-unit":
            args += ["--package-unit"]
        if self.args.required:
            args += ["--required"]
        code = self.command(gate, [str(ROOT / "tests/run-tests.sh"), *args], build=True)
        if code in {75, 77}:
            self.result(gate, "SKIP", "build prerequisite or lease unavailable")
            return
        summary = {}
        try:
            summary = json.loads((self.output / (gate + ".log")).read_text())
            status = "FAIL" if code or summary["status"] != "pass" else (
                "SKIP" if summary["skipped"] or summary["ignored"] else "PASS")
        except (OSError, ValueError, KeyError):
            status = "FAIL"
        self.result(gate, status, "" if status == "PASS" else "see private test accounting")
        if status != "PASS":
            self.results[-1]["test_summary"] = {key: summary[key] for key in (
                "cargo_exit", "passed", "failed", "skipped", "ignored", "tests_observed", "diagnostics")
                if key in summary}

    def models(self, portable=False):
        scripts = (("model-connections", "connection_regressions.py", False, []),
                   ("model-phone-worker", "phone_state_regressions.py", False, []),
                   ("model-tabs", "tab_model_regressions.py", True, []),
                   ("model-panes", "pane_focus_regressions.py", True, []),
                   ("model-insertion", "tab_model_regressions.py", True, ["--insertion"]))
        try:
            virt = subprocess.check_output(["systemd-detect-virt"], text=True,
                                           stderr=subprocess.DEVNULL, timeout=5).strip()
        except (OSError, subprocess.SubprocessError):
            virt = "unknown"
        for name, script, vm_only, extra in scripts:
            if portable and vm_only:
                continue
            if name == "model-phone-worker" and not shutil.which("rustc"):
                self.result(name, "SKIP", "Rust compiler unavailable")
            elif name != "model-phone-worker" and not Path("/usr/lib/qt6/bin/qmltestrunner").is_file():
                self.result(name, "SKIP", "Qt test runner unavailable")
            elif vm_only and (virt != "kvm" or os.getuid() != 1000 or os.geteuid() != 1000):
                self.result(name, "SKIP", "existing harness requires KVM and UID 1000")
            else:
                self.invoke(name, [sys.executable, str(ROOT / "crates/app/tests" / script),
                    "--source", str(ROOT), "--output", str(self.output / name), *extra],
                    build=True)

    def native(self, name, output, theme_state=None, journeys=None):
        # The UI-owned runner acquires its own QA lease. Never nest that lock.
        runner = self.args.native_runner
        root = self.args.installed_root
        if os.environ.get("GUST_DEDICATED_QA") != "1":
            self.result(name, "SKIP", "dedicated synthetic desktop not selected")
            return
        if root is None or not (root / "usr/bin/gust").is_file() or not runner.is_file():
            self.result(name, "SKIP", "installed artifact or UI-owned runner unavailable")
            return
        # The UI-owned runner creates a private headless labwc session and
        # admits its own tools. Requiring the host's Wayland/Hyprland session
        # would reject the isolated path and encourage unsafe host coupling.
        if self.installed_verified is None:
            self.installed_verified = self.command("native-artifact-preflight", [sys.executable,
                str(ROOT / "packaging/check-package.py"), "--root", str(root),
                "--output", str(self.output / "native-artifact-preflight")]) == 0
        if not self.installed_verified:
            self.result(name, "FAIL", "installed artifact validation failed")
            return
        env = dict(os.environ, GUST_BIN=str((root / "usr/bin/gust").resolve()))
        if theme_state:
            env["GUST_OMARCHY_STATE"] = str(theme_state)
        code = self.command(name, [str(runner.resolve()), str(output), *(journeys or [])], env=env)
        if code in {75, 77}:
            self.result(name, "SKIP", "native prerequisite or QA lease unavailable")
            return
        try:
            with (output / "results.tsv").open() as results:
                rows = list(csv.DictReader(results, delimiter="\t"))
            statuses = [row["status"] for row in rows]
            if not statuses or any(s not in {"PASS", "FAIL", "SKIP"} for s in statuses):
                raise ValueError("missing journey accounting")
            if journeys and set(journeys) != {row["journey"] for row in rows}:
                raise ValueError("requested journey not accounted for")
            status = "FAIL" if code or "FAIL" in statuses else "SKIP" if "SKIP" in statuses else "PASS"
            warnings = output / "qml-warnings.txt"
            if warnings.is_symlink() or not warnings.is_file() or warnings.stat().st_size:
                status = "FAIL"
        except (OSError, KeyError, ValueError):
            status = "FAIL"
        self.result(name, status, "" if status == "PASS" else "native journey incomplete")

    def themes(self):
        directory = self.args.themes_dir
        if not directory or not directory.is_dir() or not shutil.which("magick"):
            self.result("themes", "SKIP", "theme set or image validator unavailable")
            return
        themes = sorted(p for p in directory.iterdir() if p.is_dir())
        if not themes:
            self.result("themes", "SKIP", "theme set empty")
        for index, theme in enumerate(themes):
            name = "theme-" + str(index + 1)
            colors = theme / "colors.toml"
            if not colors.is_file():
                self.result(name, "SKIP", "theme color tokens unavailable")
                continue
            state = self.output / name / "state"
            (state / "current").mkdir(parents=True)
            shutil.copytree(theme, state / "current/theme")
            (state / "current/theme.name").write_text(theme.name + "\n")
            journey = self.output / name / "journey"
            self.native(name, journey, state, ["launch"])
            if self.results[-1]["status"] != "PASS":
                continue
            # Check the actual window-region capture against this theme, not
            # merely the existence of a screenshot or a zero process exit.
            match = re.search(r'^background\s*=\s*"(#[0-9a-fA-F]{6})"', colors.read_text(), re.MULTILINE)
            result = subprocess.run(["magick", str(journey / "launch.png"), "-colors", "24",
                "-format", "%c", "histogram:info:-"], capture_output=True, text=True, timeout=30)
            if not match or result.returncode or match[1].lower() not in result.stdout.lower():
                self.results[-1].update(status="FAIL", reason="theme background missing from window capture")

    def run(self):
        for gate in self.args.gate or ("static", "libraries", "app", "models", "installed",
                                      "dependencies", "native", "themes"):
            if gate in {"libraries", "library-unit", "app"}:
                self.cargo(gate)
            elif gate == "static":
                for name, script in (("runner-contracts", "tests/test-release-runners.py"),
                    ("artifact-contracts", "tests/test-package-artifacts.py"),
                    ("source-archive-contracts", "tests/test-source-archive.py"),
                    ("package-metadata", "tests/check-setup-packaging.py"),
                    ("dependency-metadata", "packaging/check-dependencies.py")):
                    self.invoke(name, [sys.executable, str(ROOT / script)])
            elif gate in {"models", "model-seams"}:
                self.models(portable=gate == "model-seams")
            elif gate == "installed":
                if self.args.installed_root is None:
                    self.result(gate, "SKIP", "installed root not supplied")
                else:
                    self.invoke(gate, [sys.executable, str(ROOT / "packaging/check-package.py"),
                        "--root", str(self.args.installed_root), "--output", str(self.output / "installed")])
            elif gate == "dependencies":
                self.invoke(gate, [sys.executable, str(ROOT / "packaging/check-dependencies.py"), "--installed"])
            elif gate == "native":
                self.native(gate, self.output / "native")
            elif gate == "themes":
                self.themes()
        (self.output / "summary.json").write_text(json.dumps(self.results, indent=2) + "\n")
        context = {"schema": 1,
            "head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
                if (ROOT / ".git").exists() else "source-archive",
            "branch": subprocess.check_output(["git", "branch", "--show-current"], cwd=ROOT, text=True).strip()
                if (ROOT / ".git").exists() else None,
            "scope": "selected gates only; no inferred benchmark/hardware/provider qualification"}
        (self.output / "context.json").write_text(json.dumps(context, indent=2) + "\n")
        for result in self.results:
            print(json.dumps(result, sort_keys=True))
        return int(any(r["status"] == "FAIL" or (self.args.required and r["status"] != "PASS")
                       for r in self.results))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gate", choices=GATES, action="append")
    parser.add_argument("--required", action="store_true")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--installed-root", type=Path)
    parser.add_argument("--native-runner", type=Path, default=ROOT / "tests/ui-flea-journeys.sh")
    parser.add_argument("--themes-dir", type=Path)
    args = parser.parse_args()
    if args.installed_root:
        args.installed_root = args.installed_root.resolve()
    try:
        signal.signal(signal.SIGTERM, cancel_on_signal)
        return Gates(args).run()
    except KeyboardInterrupt:
        print(json.dumps({"status": "FAIL", "reason": "gate cancelled; owned processes stopped"}))
        return 130
    except (OSError, ValueError, subprocess.SubprocessError):
        print(json.dumps({"status": "FAIL", "reason": "gate admission or execution failed"}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
