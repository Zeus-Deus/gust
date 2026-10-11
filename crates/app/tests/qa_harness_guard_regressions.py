#!/usr/bin/env python3
"""VM-only subprocess QA guard tests; synthetic runner, no desktop or Qt I/O.

The real UID/KVM/boot preflight is independent of all child fault seams.
Every case retains literal source copies, stdout/stderr, trace and actual exit.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys


SOURCES = [
    "Justfile", "ui/Main.qml", "ui/Pane.qml", "ui/ColumnsView.qml",
    "ui/tests/tst_tab_model.qml.in", "ui/tests/tst_pane_focus.qml.in",
    "crates/app/tests/tab_model_regressions.py",
    "crates/app/tests/pane_focus_regressions.py",
    "crates/app/tests/qa_harness_guard_regressions.py",
]
MODES = [("normal", [], None), ("-O", ["-O"], None),
         ("-OO", ["-OO"], None), ("env1", [], "1"), ("env2", [], "2")]
# These seams exist only in an owned child's PYTHONPATH, not in either harness.
SITE = r'''
import json, os, pathlib, subprocess, sys
config = json.loads(os.environ["GUST_GUARD_CHILD"])
entry = pathlib.Path(sys.argv[0]).name
if entry in ("tab_model_regressions.py", "pane_focus_regressions.py"):
    target = "tabs" if entry.startswith("tab_") else "panes"
    fault = config["fault"] if target == config["target"] else "allowed"
    source = config["source"]
    output = config["output"] + ("/" + target if config["recipe"] else "")
    def event(kind, **fields):
        with open(config["trace"], "a") as stream:
            stream.write(json.dumps(dict(entry=target, kind=kind, **fields)) + "\n")
    event("child", optimize=sys.flags.optimize, uid=os.getuid(), euid=os.geteuid())
    uid = 0 if fault == "root" else 1001 if fault == "other_uid" else 1000
    euid = 0 if fault == "effective_root" else 1001 if fault == "other_euid" else 1000
    os.getuid = lambda: uid
    os.geteuid = lambda: euid
    original_output = subprocess.check_output
    def check_output(argv, *args, **kwargs):
        if argv == ["systemd-detect-virt"]:
            event("detector", fault=fault)
            if fault == "detector_error":
                raise subprocess.CalledProcessError(7, argv, output="probe error")
            if fault == "detector_missing":
                raise FileNotFoundError("synthetic missing detector")
            return "none\n" if fault == "non_kvm" else "kvm\n"
        raise RuntimeError("unexpected subprocess check_output")
    subprocess.check_output = check_output
    for method in ("resolve", "read_text", "read_bytes", "write_text", "write_bytes", "mkdir"):
        original = getattr(pathlib.Path, method)
        def wrap(path, *args, _method=method, _original=original, **kwargs):
            name = str(path)
            if name == source or name.startswith(source + "/") or (config["recipe"] and name == "."):
                event("source_action", method=_method, path=name)
            if name == output or name.startswith(output + "/"):
                event("output_action", method=_method, path=name)
            return _original(path, *args, **kwargs)
        setattr(pathlib.Path, method, wrap)
    def run(argv, *args, **kwargs):
        if argv[0] != "/usr/lib/qt6/bin/qmltestrunner":
            raise RuntimeError("unexpected subprocess run")
        env = kwargs["env"]
        expected = {"PATH", "LANG", "HOME", "TMPDIR", "XDG_RUNTIME_DIR", "QT_QPA_PLATFORM",
                    "QT_QUICK_BACKEND", "QSG_RENDER_LOOP", "QT_FORCE_STDERR_LOGGING",
                    "QT_ASSUME_STDERR_HAS_CONSOLE"}
        if set(env) != expected or env["QT_QPA_PLATFORM"] != "offscreen" or env["QT_QUICK_BACKEND"] != "software":
            raise RuntimeError("runner environment escaped the existing allowlist")
        event("runner", argv=argv, env=env)
        if fault == "mutation":
            path = pathlib.Path(source) / "ui/Main.qml"
            path.write_text(path.read_text() + "\n// owned guard-test mutation\n")
            event("mutation", path=str(path))
        code = 23 if fault == "runner_error" else 0
        return subprocess.CompletedProcess(argv, code, "SYNTHETIC_QT_RUNNER exit=" + str(code) + "\n", "")
    subprocess.run = run
'''


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def real_preflight(expected_boot):
    if os.getuid() != 1000 or os.geteuid() != 1000:
        raise RuntimeError("Regression runner requires real/effective UID 1000")
    probe = subprocess.run(["systemd-detect-virt"], capture_output=True, text=True, timeout=5)
    boot = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
    if probe.returncode != 0 or probe.stdout.strip() != "kvm" or boot != expected_boot:
        raise RuntimeError("Independent authorized KVM/boot preflight failed")
    return {"uid": os.getuid(), "euid": os.geteuid(), "virt": probe.stdout.strip(),
            "detector_exit": probe.returncode, "boot_id": boot}


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--source", type=Path, required=True)
    ap.add_argument("--output", type=Path, required=True)
    ap.add_argument("--expected-boot", required=True)
    ap.add_argument("--group", choices=["guards", "extraction", "all"], default="all")
    args = ap.parse_args()
    identity = real_preflight(args.expected_boot)  # Never under a child seam.
    source = args.source.resolve()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    inputs = {name: sha(source / name) for name in SOURCES}
    retained = out / "input-source"
    for name in SOURCES:
        dst = retained / name
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source / name, dst)
    write_json(out / "preflight.json", identity)
    rows = []

    def case(target, mode, fault, recipe=False, edit=None, oracle=None):
        label, flags, optimize = mode
        number = len(rows) + 1
        name = f"{number:03d}-{'recipe' if recipe else 'direct'}-{target}-{label}-{fault}"
        owned = out / name
        owned.mkdir()
        copy = owned / "source"
        for rel in SOURCES:
            dest = copy / rel
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(retained / rel, dest)
        if edit:
            edit(copy)
        before = {rel: sha(copy / rel) for rel in SOURCES}
        before_dir = owned / "source-before"
        shutil.copytree(copy, before_dir)
        evidence = owned / "evidence"
        target_output = evidence / target if recipe else evidence
        if fault == "fresh_output":
            target_output.mkdir(parents=True)
            (target_output / "sentinel").write_text("must survive\n")
        site = owned / "seam"
        site.mkdir()
        (site / "sitecustomize.py").write_text(SITE)
        trace = owned / "trace.jsonl"
        config = {"source": str(copy), "output": str(evidence), "target": target,
                  "trace": str(trace), "fault": fault, "recipe": recipe}
        env = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "HOME": str(owned),
               "TMPDIR": str(owned), "PYTHONDONTWRITEBYTECODE": "1", "PYTHONPATH": str(site),
               "GUST_GUARD_CHILD": json.dumps(config), "GUST_MODEL_QA_OUT": str(evidence),
               "WAYLAND_DISPLAY": "forbidden-guard-child-desktop",
               "DISPLAY": "forbidden-guard-child-desktop"}
        if optimize is not None:
            env["PYTHONOPTIMIZE"] = optimize
        entry = "tab_model_regressions.py" if target == "tabs" else "pane_focus_regressions.py"
        argv = (["/usr/bin/just", "--justfile", str(copy / "Justfile"), "qa-model-routing"]
                if recipe else [sys.executable, *flags, str(copy / "crates/app/tests" / entry),
                                "--source", str(copy), "--output", str(evidence)])
        if fault == "counterfactual_marker":
            argv.append("--counterfactual-refresh")
        started = datetime.now(timezone.utc).isoformat()
        proc = subprocess.run(argv, cwd=copy, env=env, capture_output=True, text=True, timeout=20)
        (owned / "stdout.log").write_text(proc.stdout)
        (owned / "stderr.log").write_text(proc.stderr)
        (owned / "exit.txt").write_text(str(proc.returncode) + "\n")
        after = {rel: sha(copy / rel) for rel in SOURCES}
        events = [json.loads(line) for line in trace.read_text().splitlines()] if trace.exists() else []
        scoped = [event for event in events if event["entry"] == target]
        problems = []
        def check(condition, message):
            if not condition:
                problems.append(message)
        child = [event for event in scoped if event["kind"] == "child"]
        expected_opt = 1 if label in ("-O", "env1") else 2 if label in ("-OO", "env2") else 0
        check(len(child) == 1 and child[0]["optimize"] == expected_opt,
              "actual entrypoint/optimization not exercised")
        kinds = [event["kind"] for event in scoped]
        admission = fault in {"root", "other_uid", "effective_root", "other_euid", "non_kvm", "detector_error", "detector_missing"}
        if admission:
            check(proc.returncode != 0, "admission refusal returned success")
            check("source_action" not in kinds and "output_action" not in kinds and "runner" not in kinds,
                  "admission refusal followed source/output/runner effects")
            check(not target_output.exists(), "admission refusal created evidence")
            check(after == before, "admission refusal changed source")
            if fault in {"non_kvm", "detector_error", "detector_missing"}:
                check(kinds.count("detector") == 1, "virtualization detector did not execute exactly once")
            else:
                check("detector" not in kinds, "UID refusal did not precede detector")
            # Avoid a false green caused by a missing source/import/dependency.
            check("AssertionError" in proc.stderr or "VM QA" in proc.stderr,
                  "refusal did not name the admission boundary")
        elif fault == "fresh_output":
            check(proc.returncode != 0 and "runner" not in kinds, "fresh output guard did not refuse before runner")
            check((target_output / "sentinel").read_text() == "must survive\n", "fresh output sentinel changed")
            check(sorted(p.name for p in target_output.iterdir()) == ["sentinel"], "fresh output acquired new files")
        elif fault == "mutation":
            receipt_path = target_output / "receipt.json"
            check(receipt_path.exists(), "source mutation did not retain its receipt")
            if receipt_path.exists():
                receipt = json.loads(receipt_path.read_text())
                check(receipt["source_unchanged"] is False and receipt["exit"] == 0,
                      "mutation receipt lost unchanged=false/successful runner exit")
                check(receipt["source_hashes"]["ui/Main.qml"] == before["ui/Main.qml"], "mutation receipt input hash mismatch")
            check(proc.returncode != 0, "source mutation returned success after successful Qt runner")
            check(after["ui/Main.qml"] != before["ui/Main.qml"], "mutation seam never changed owned source")
            check(kinds.count("runner") == 1 and kinds.count("mutation") == 1, "mutation boundary not exercised")
        elif fault == "runner_error":
            check(proc.returncode != 0 and kinds.count("runner") == 1, "failed runner was not propagated")
            receipt_path = target_output / "receipt.json"
            check(receipt_path.exists() and json.loads(receipt_path.read_text())["exit"] == 23,
                  "runner's actual failure exit not retained")
        elif fault == "allowed":
            check(proc.returncode == 0 and kinds.count("detector") == 1 and kinds.count("runner") == 1,
                  "approved unchanged KVM control failed")
            check(after == before, "approved unchanged control changed source")
            outputs = [evidence / "tabs", evidence / "panes"] if recipe else [evidence]
            for output in outputs:
                receipt_path = output / "receipt.json"
                check(receipt_path.exists() and json.loads(receipt_path.read_text())["source_unchanged"] is True,
                      "approved control missing unchanged=true receipt")
        elif oracle:
            qml = target_output / "tst_pane_focus.qml"
            check(proc.returncode == 0 and kinds.count("runner") == 1, "bounded extraction control failed")
            check(qml.exists() and oracle(qml.read_text()), "extraction silently sampled a foreign handler")
        else:
            check(proc.returncode != 0 and "runner" not in kinds,
                  "invalid fixture/handler was accepted by runner")
            check(after == before, "invalid fixture/handler changed source during run")
        row = {"name": name, "target": target, "mode": label, "fault": fault, "recipe": recipe,
               "argv": argv, "cwd": str(copy), "started_at": started, "exit": proc.returncode,
               "passed": not problems, "problems": problems, "source_hashes_before": before,
               "source_hashes_after": after, "seam_sha256": sha(site / "sitecustomize.py"),
               "stdout_sha256": sha(owned / "stdout.log"), "stderr_sha256": sha(owned / "stderr.log"),
               "trace_sha256": sha(trace) if trace.exists() else None}
        write_json(owned / "result.json", row)
        rows.append(row)
        write_json(out / "cases.json", rows)
        print(("PASS " if not problems else "FAIL ") + name + " exit=" + str(proc.returncode) +
              (" " + "; ".join(problems) if problems else ""), flush=True)

    def fixture_edit(target, duplicate):
        def edit(copy):
            path = copy / ("ui/tests/tst_tab_model.qml.in" if target == "tabs" else "ui/tests/tst_pane_focus.qml.in")
            key = "@FUNCTIONS@" if target == "tabs" else "@CURRENT_CLICK@"
            text = path.read_text()
            if text.count(key) != 1:
                raise RuntimeError("bad regression fixture input")
            path.write_text(text.replace(key, key + "\n" + key if duplicate else ""))
        return edit

    if args.group in ("guards", "all"):
        for target in ("tabs", "panes"):
            for mode in MODES:
                for fault in ("root", "other_uid", "effective_root", "other_euid", "non_kvm",
                              "detector_error", "detector_missing", "allowed", "mutation", "runner_error", "fresh_output"):
                    case(target, mode, fault)
                case(target, mode, "missing_placeholder", edit=fixture_edit(target, False))
                case(target, mode, "multiple_placeholders", edit=fixture_edit(target, True))
        # Execute the actual combined Justfile recipe; target=panes allows tabs
        # first, then proves the second entrypoint independently rejects.
        for target in ("tabs", "panes"):
            for mode in (MODES[0], MODES[3], MODES[4]):
                for fault in ("root", "non_kvm", "detector_error", "allowed", "mutation", "fresh_output"):
                    case(target, mode, fault, recipe=True)

    if args.group in ("extraction", "all"):
        def decoy(copy):
            path = copy / "ui/Main.qml"
            text = path.read_text()
            anchor = "                id: paneArea\n"
            if text.count(anchor) != 1:
                raise RuntimeError("paneArea anchor changed")
            path.write_text(text.replace(anchor, anchor + '                onSelectionTouched: { foreignAncestor() }\n'))
        def remove_handler(side):
            def edit(copy):
                path = copy / "ui/Main.qml"
                text = path.read_text()
                handler = f'onSelectionTouched: {{ win.focusPane({side}); win.focusList(); win.qaDump("select") }}'
                if text.count(handler) != 1:
                    raise RuntimeError("handler anchor changed")
                text = text.replace(handler, "// selection handler deliberately absent")
                text += '\nItem { onSelectionTouched: { foreignLater() } }\n'
                path.write_text(text)
            return edit
        def duplicate_handler(side):
            def edit(copy):
                path = copy / "ui/Main.qml"
                text = path.read_text()
                handler = f'onSelectionTouched: {{ win.focusPane({side}); win.focusList(); win.qaDump("select") }}'
                if text.count(handler) != 1:
                    raise RuntimeError("handler anchor changed")
                path.write_text(text.replace(handler, handler + '\n onSelectionTouched: { foreignDuplicate() }'))
            return edit
        def duplicate_pane(copy):
            path = copy / "ui/Main.qml"
            path.write_text(path.read_text() + '\nPane { id: paneA; onSelectionTouched: { foreignPane() } }\n')
        for mode in MODES:
            case("panes", mode, "ancestor_decoy", edit=decoy,
                 oracle=lambda qml: "foreignAncestor" not in qml and 'win.focusPane(0); win.focusList(); win.qaDump("select")' in qml)
            for side in (0, 1):
                case("panes", mode, f"missing_handler_{side}", edit=remove_handler(side))
                case("panes", mode, f"multiple_handlers_{side}", edit=duplicate_handler(side))
            case("panes", mode, "multiple_paneA", edit=duplicate_pane)
    final_identity = real_preflight(args.expected_boot)
    unchanged = inputs == {name: sha(source / name) for name in SOURCES}
    passed = sum(row["passed"] for row in rows)
    result = {"scope": "QA guard subprocesses with synthetic Qt runner; no Qt/native/visual acceptance",
              "preflight": identity, "preflight_after": final_identity, "input_hashes": inputs,
              "input_unchanged": unchanged, "group": args.group, "total": len(rows),
              "passed": passed, "failed": len(rows) - passed, "skipped": 0,
              "exit": 0 if passed == len(rows) and unchanged else 1}
    write_json(out / "receipt.json", result)
    print(json.dumps(result, indent=2))
    return result["exit"]


if __name__ == "__main__":
    sys.exit(main())
