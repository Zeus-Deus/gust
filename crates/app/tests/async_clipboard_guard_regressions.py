#!/usr/bin/env python3
"""VM-only async clipboard boundary regressions, with child-only I/O faults.

No Qt, Rust compiler, generated executable, desktop or network is invoked.
Every subprocess gets a unique owned source copy and durable exit/log binding.
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
import traceback

SOURCES = [
    "crates/app/tests/async_clipboard_regressions.py",
    "crates/app/tests/tab_model_regressions.py",
    "crates/app/tests/async_clipboard_guard_regressions.py",
    "crates/app/src/bridge.rs", "crates/app/src/clipboard.rs",
    "crates/app/tests/clipboard_bridge_scaffold.rs.in",
    "ui/Main.qml", "ui/ConflictDialog.qml", "ui/tests/tst_async_clipboard.qml.in",
]
MODES = [("normal", [], None), ("-O", ["-O"], None),
         ("-OO", ["-OO"], None), ("env1", [], "1"), ("env2", [], "2")]
SITE = r'''
import json, os, pathlib, subprocess, sys
if pathlib.Path(sys.argv[0]).name == "async_clipboard_regressions.py":
    def deny_spawn(*args, **kwargs):
        raise RuntimeError("Child seam forbids all real subprocess creation")
    subprocess.Popen = deny_spawn  # Fail closed even if later seam setup fails.
    c = json.loads(os.environ["GUST_ASYNC_GUARD_CHILD"])
    fault = c["fault"]
    def event(kind, **fields):
        with open(c["trace"], "a") as stream:
            stream.write(json.dumps(dict(kind=kind, **fields)) + "\n")
    event("child", optimize=sys.flags.optimize, real_uid=os.getuid(), real_euid=os.geteuid())
    os.getuid = lambda: 0 if fault == "root" else 1001 if fault == "other_uid" else 1000
    os.geteuid = lambda: 0 if fault == "effective_root" else 1001 if fault == "other_euid" else 1000
    def detector(argv, *args, **kwargs):
        if argv != ["systemd-detect-virt"]:
            raise RuntimeError("unexpected child check_output")
        event("detector", timeout=kwargs.get("timeout"))
        if fault == "detector_error":
            raise subprocess.CalledProcessError(7, argv, output="synthetic detector error")
        if fault == "detector_missing":
            raise FileNotFoundError("synthetic missing detector")
        if fault == "detector_timeout":
            raise subprocess.TimeoutExpired(argv, kwargs.get("timeout", 0))
        return "none\n" if fault == "non_kvm" else "kvm\n"
    subprocess.check_output = detector
    boot_reads = [0]
    for method in ("read_text", "read_bytes", "write_text", "write_bytes", "mkdir", "resolve"):
        original = getattr(pathlib.Path, method)
        def wrap(path, *args, _method=method, _original=original, **kwargs):
            name = str(path)
            if name == "/proc/sys/kernel/random/boot_id" and _method == "read_text":
                boot_reads[0] += 1
                if fault == "boot_mismatch" and boot_reads[0] > 1:
                    return "00000000-0000-0000-0000-000000000000\n"
            for scope in ("source", "output"):
                if name == c[scope] or name.startswith(c[scope] + "/"):
                    event(scope + "_action", method=_method, path=name)
            return _original(path, *args, **kwargs)
        setattr(pathlib.Path, method, wrap)
    def runner(argv, *args, **kwargs):
        first = argv[0]
        if first == "rustc":
            name = "compile"
        elif first == str(pathlib.Path(c["output"]) / "bridge_tests"):
            name = "tests"
        elif first == "/usr/lib/qt6/bin/qmltestrunner":
            name = "qml_tests"
        elif first == "rustfmt":
            name = "rustfmt"
        else:
            raise RuntimeError("unexpected child runner; no real subprocess permitted")
        env = kwargs["env"]
        event("runner", name=name, argv=argv, env=env)
        if name == "compile":
            pathlib.Path(argv[argv.index("-o") + 1]).write_bytes(b"SYNTHETIC NONEXECUTABLE COMPILER OUTPUT\n")
        if fault.startswith("mutation") and name in ("compile", "qml_tests", "rustfmt", "tests"):
            if fault != "mutation_tests" or name == "tests":
                path = pathlib.Path(c["source"]) / c.get("mutation_rel", "ui/Main.qml")
                path.write_bytes(path.read_bytes() + b"\n// owned synthetic guard mutation\n")
                event("mutation", path=str(path))
        code = 23 if fault == "runner_error" else 24 if fault == "tests_error" and name == "tests" else 0
        kwargs["stdout"].write(("SYNTHETIC_RUNNER " + name + " exit=" + str(code) + "\n").encode())
        return subprocess.CompletedProcess(argv, code)
    subprocess.run = runner
'''


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_json(path, data):
    path.write_text(json.dumps(data, indent=2) + "\n")


def real_preflight(boot):
    # The controller's real identity is never patched by a child seam.
    probe = subprocess.run(["systemd-detect-virt"], capture_output=True, text=True, timeout=5)
    actual = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
    if os.getuid() != 1000 or os.geteuid() != 1000 or probe.returncode != 0 or probe.stdout.strip() != "kvm" or actual != boot:
        raise RuntimeError("Independent authorized UID/EUID/KVM/boot preflight failed")
    return {"uid": os.getuid(), "euid": os.geteuid(), "virt": probe.stdout.strip(),
            "detector_exit": probe.returncode, "boot_id": actual}


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--source", type=Path, required=True)
    ap.add_argument("--output", type=Path, required=True)
    ap.add_argument("--expected-boot", required=True)
    ap.add_argument("--group", choices=["pilot", "admission", "integrity", "placeholders", "rust_anchors", "qml_anchors", "controls", "all"], default="all")
    args = ap.parse_args()
    identity = real_preflight(args.expected_boot)
    compile(SITE, "owned-sitecustomize.py", "exec")  # A syntax error must precede any child.
    source = args.source.resolve()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    retained = out / "input-source"
    for rel in SOURCES:
        dst = retained / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source / rel, dst)
    inputs = {rel: sha(retained / rel) for rel in SOURCES}
    write_json(out / "preflight.json", identity)
    rows = []

    def case(branch, mode, fault, edit=None, mutation_rel=None):
        label, flags, optimize = mode
        owned = out / f"{len(rows) + 1:03d}-{branch}-{label}-{fault}"
        owned.mkdir()
        copy = owned / "source"
        shutil.copytree(retained, copy)
        if edit:
            edit(copy)
        before = {rel: sha(copy / rel) for rel in SOURCES}
        shutil.copytree(copy, owned / "source-before")
        evidence = owned / "evidence"
        if fault == "fresh_output":
            evidence.mkdir()
            (evidence / "sentinel").write_text("must survive\n")
        seam = owned / "seam"
        seam.mkdir()
        (seam / "sitecustomize.py").write_text(SITE)
        trace = owned / "trace.jsonl"
        config = {"source": str(copy), "output": str(evidence), "trace": str(trace), "fault": fault}
        if mutation_rel:
            config["mutation_rel"] = mutation_rel
        env = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "HOME": str(owned), "TMPDIR": str(owned),
               "PYTHONDONTWRITEBYTECODE": "1", "PYTHONPATH": str(seam),
               "GUST_ASYNC_GUARD_CHILD": json.dumps(config), "DISPLAY": "forbidden-desktop",
               "WAYLAND_DISPLAY": "forbidden-desktop", "DBUS_SESSION_BUS_ADDRESS": "forbidden-bus"}
        if optimize is not None:
            env["PYTHONOPTIMIZE"] = optimize
        argv = [sys.executable, *flags, str(copy / SOURCES[0]), "--source", str(copy), "--output", str(evidence)]
        if branch == "qml":
            argv.append("--qml")
        elif branch == "format":
            argv.append("--format")
        started = datetime.now(timezone.utc).isoformat()
        proc = subprocess.run(argv, cwd=copy, env=env, capture_output=True, text=True, timeout=15)
        (owned / "stdout.log").write_text(proc.stdout)
        (owned / "stderr.log").write_text(proc.stderr)
        (owned / "exit.txt").write_text(str(proc.returncode) + "\n")
        after = {rel: sha(copy / rel) for rel in SOURCES}
        events = [json.loads(line) for line in trace.read_text().splitlines()] if trace.exists() else []
        kinds = [e["kind"] for e in events]
        failures = []
        def require(condition, message):
            if not condition:
                try:
                    raise AssertionError(message)
                except AssertionError:
                    failures.append(traceback.format_exc())
        child = [e for e in events if e["kind"] == "child"]
        expected_opt = 1 if label in ("-O", "env1") else 2 if label in ("-OO", "env2") else 0
        require(len(child) == 1 and child[0]["optimize"] == expected_opt, "Actual entrypoint optimization not exercised")
        admission = fault in {"root", "other_uid", "effective_root", "other_euid", "non_kvm", "detector_error", "detector_missing", "detector_timeout"}
        if admission:
            require(proc.returncode != 0, "Admission refusal returned success")
            require(not {"source_action", "output_action", "runner"}.intersection(kinds), "Rejected admission performed source/output/runner actions")
            require(not evidence.exists() and before == after, "Rejected admission wrote evidence/source")
            uid_fault = fault in {"root", "other_uid", "effective_root", "other_euid"}
            require(kinds.count("detector") == (0 if uid_fault else 1), "Admission detector ordering/count mismatch")
            for event in events:
                if event["kind"] == "detector":
                    require(isinstance(event["timeout"], (int, float)) and 0 < event["timeout"] <= 5, "Admission detector lacks a finite five-second bound")
            boundary_errors = ("AssertionError", "VM QA", "CalledProcessError", "FileNotFoundError", "TimeoutExpired")
            require(any(name in proc.stderr for name in boundary_errors), "Refusal did not name the admission boundary")
        elif fault in {"mutation", "mutation_helper", "mutation_entry", "mutation_tests", "boot_mismatch"}:
            receipt_path = evidence / "receipt.json"
            require(proc.returncode != 0, "Successful runner mutation/boot mismatch returned success")
            require(receipt_path.exists(), "Integrity failure lost its receipt")
            if receipt_path.exists():
                receipt = json.loads(receipt_path.read_text())
                checks = receipt["checks"]
                require(bool(checks) and checks[-1]["exit_code"] == 0, "Integrity seam did not follow a successful runner")
                if fault == "boot_mismatch":
                    require(checks[-1]["boot_id_after"] != receipt["boot_id"], "Boot mismatch seam was not observed")
                else:
                    rel = mutation_rel or "ui/Main.qml"
                    require(after[rel] != before[rel], "Mutation did not change owned source")
                    require(checks[-1]["source_unchanged"] is False, "Mutation receipt failed to deny source integrity")
                    require(checks[-1].get("source_hashes_after", {}).get(rel) == after[rel], "Receipt lost exact source-after hash")
                if "harness_hashes" in receipt:
                    for path, digest in receipt["harness_hashes"].items():
                        require(digest == before[str(Path(path).relative_to(copy))], "Actual imported harness helper binding mismatch")
        elif fault == "fresh_output":
            require(proc.returncode != 0 and "runner" not in kinds, "Fresh output guard did not refuse before runner")
            require(sorted(p.name for p in evidence.iterdir()) == ["sentinel"] and (evidence / "sentinel").read_text() == "must survive\n", "Fresh output sentinel changed")
        elif fault.startswith("placeholder_") or fault.startswith("anchor_"):
            require(proc.returncode != 0, "Malformed/missing/duplicate source boundary returned success")
            require("runner" not in kinds, "Rejected source boundary launched a runner")
            require(before == after, "Source validation mutated its input")
        elif fault in {"allowed", "runner_error", "tests_error", "legacy", "optional_absent", "inline_legacy"}:
            runners = [e for e in events if e["kind"] == "runner"]
            expected = ["qml_tests"] if branch == "qml" else ["rustfmt"] if branch == "format" else ["compile", "tests"]
            if fault == "runner_error":
                expected = expected[:1]
            require([e["name"] for e in runners] == expected, "Productive runner ordering/count changed")
            code = 23 if fault == "runner_error" else 24 if fault == "tests_error" else 0
            require(proc.returncode == code, "Runner success/failure exit was not propagated")
            require(before == after, "Productive runner changed source")
            for runner in runners:
                actual_env = runner["env"]
                common = {"PATH", "LANG", "HOME", "TMPDIR", "GUST_TEST_SOURCE"}
                allowed = common | ({"QT_QPA_PLATFORM", "QSG_RHI_BACKEND"} if branch == "qml" else {"WAYLAND_DISPLAY"})
                require(set(actual_env) == allowed, "Runner inherited non-allowlisted environment")
                require(actual_env.get("QT_QPA_PLATFORM") == "offscreen" if branch == "qml" else actual_env.get("WAYLAND_DISPLAY") == "synthetic-no-desktop-io", "Runner desktop isolation changed")
                require(Path(actual_env["HOME"]).is_relative_to(evidence) and Path(actual_env["TMPDIR"]).is_relative_to(evidence), "Runner HOME/TMPDIR escaped owned evidence")
            path = evidence / "receipt.json"
            require(path.exists(), "Productive runner lost receipt")
            if path.exists():
                receipt = json.loads(path.read_text())
                require(all(c["source_unchanged"] for c in receipt["checks"]), "Unchanged successful/failing runner failed integrity")
                if fault == "legacy":
                    require(receipt["legacy_gate_adapter"] is True, "Legacy ReadGate adapter semantics changed")
        else:
            raise RuntimeError("Unknown case; add an explicit oracle before execution")
        row = {"case": owned.name, "branch": branch, "mode": label, "fault": fault, "argv": argv,
               "exit": proc.returncode, "started_at": started, "finished_at": datetime.now(timezone.utc).isoformat(),
               "source_before": before, "source_after": after, "assertion_failures": failures,
               "status": "FAIL" if failures else "PASS", "events": events,
               "artifact_hashes": {str(p.relative_to(owned)): sha(p) for p in owned.rglob("*") if p.is_file()}}
        write_json(owned / "case.json", row)
        rows.append(row)
        print(json.dumps({"case": row["case"], "status": row["status"], "exit": row["exit"]}), flush=True)
        for failure in failures:
            print(failure, flush=True)

    def run_group(group):
        if group == "pilot":
            case("rust", MODES[1], "other_uid")
        elif group == "admission":
            for branch in ("rust", "qml"):
                for mode in MODES:
                    for fault in ("root", "other_uid", "effective_root", "other_euid", "non_kvm", "detector_error", "detector_missing", "detector_timeout"):
                        case(branch, mode, fault)
        elif group == "integrity":
            for branch in ("rust", "qml"):
                for mode in MODES:
                    for fault, rel in [("mutation", "ui/Main.qml"), ("mutation_helper", SOURCES[1]),
                                       ("mutation_entry", SOURCES[0]), ("boot_mismatch", None)]:
                        case(branch, mode, fault, mutation_rel=rel)
                    if branch == "rust":
                        case(branch, mode, "mutation_tests")
        elif group == "placeholders":
            for branch in ("rust", "qml"):
                for mode in MODES:
                    for marker in ("    // PRODUCTION_METHODS", "// TEST_GATE_FACTORY"):
                        for variant in ("missing", "duplicate"):
                            def edit_placeholder(copy, marker=marker, variant=variant):
                                path = copy / "crates/app/tests/clipboard_bridge_scaffold.rs.in"
                                text = path.read_text()
                                if text.count(marker) != 1:
                                    raise RuntimeError("Test fixture placeholder baseline changed")
                                path.write_text(text.replace(marker, "// removed required marker") if variant == "missing" else text + "\n" + marker + "\n")
                            case(branch, mode, "placeholder_" + marker.strip().split()[-1] + "_" + variant, edit=edit_placeholder)
        elif group == "rust_anchors":
            for branch in ("rust", "qml"):
                for mode in MODES:
                    for variant in ("missing", "declaration_only", "duplicate", "malformed", "unclosed"):
                        def edit(copy, variant=variant):
                            path = copy / "crates/app/src/bridge.rs"
                            text = path.read_text()
                            anchor = "    fn clip_copy("
                            start = text.rindex(anchor)
                            end = text.index("\n    }", start) + len("\n    }")
                            body = text[start:end]
                            if variant == "missing":
                                text = text.replace(anchor, "    fn removed_clip_copy(")
                            elif variant == "declaration_only":
                                text = text[:start] + text[end:]
                            elif variant == "duplicate":
                                text += "\n" + body + "\n"
                            elif variant == "malformed":
                                brace = text.index("{", start)
                                text = text[:brace] + ";" + text[brace + 1:]
                            else:
                                text = text[:end - 1] + " " + text[end:]
                            path.write_text(text)
                        case(branch, mode, "anchor_rust_" + variant, edit=edit)
        elif group == "qml_anchors":
            specs = [
                ("ui/Main.qml", "    function transferStarted(", "function"),
                ("ui/Main.qml", "    ConflictDialog {", "object"),
                ("ui/Main.qml", "    ConfirmDialog {", "object"),
                ("ui/Main.qml", "        onAnswered:", "conflict_handler"),
                ("ui/Main.qml", "        onCancelled:", "conflict_handler"),
                ("ui/Main.qml", "        onClosed:", "conflict_handler"),
                ("ui/Main.qml", "        onObjectRemoved:", "handler"),
                ("ui/Main.qml", "            case Qt.Key_Escape:", "delimiter"),
                ("ui/Main.qml", "                if (chooserBar.active", "delimiter"),
                ("ui/ConflictDialog.qml", "    function show(", "function"),
            ]
            for mode in MODES:
                for index, (rel, anchor, kind) in enumerate(specs):
                    for variant in ("missing", "duplicate", "malformed"):
                        def edit_qml(copy, rel=rel, anchor=anchor, kind=kind, variant=variant):
                            path = copy / rel
                            text = path.read_text()
                            scope_start = text.index("    ConflictDialog {") if kind == "conflict_handler" else 0
                            start = text.index(anchor, scope_start)
                            line_end = text.index("\n", start)
                            line = text[start:line_end]
                            if variant == "missing":
                                text = text[:start] + text[start:].replace(anchor, anchor.replace(":", "_removed:") if ":" in anchor else anchor.replace("function ", "function removed_") if "function " in anchor else "    RemovedObject {", 1)
                            elif variant == "duplicate":
                                text = text[:start] + line + "\n" + text[start:]
                            elif "{" in line:
                                text = text[:start] + line.replace("{", ";", 1) + text[line_end:]
                            elif kind == "delimiter":
                                text = text[:start] + line.replace(":", ";", 1).replace("if (", "if ;(", 1) + text[line_end:]
                            else:
                                text = text[:start] + line + ";" + text[line_end:]
                            path.write_text(text)
                        case("qml", mode, f"anchor_qml_{index}_{variant}", edit=edit_qml)
        elif group == "controls":
            for branch in ("rust", "qml", "format"):
                for mode in MODES:
                    for fault in ("allowed", "runner_error", "fresh_output"):
                        case(branch, mode, fault)
                    if branch == "rust":
                        case(branch, mode, "tests_error")
                        def legacy_edit(copy):
                            path = copy / "crates/app/src/clipboard.rs"
                            path.write_text(path.read_text().replace("pub fn with_generation", "pub fn legacy_with_generation"))
                        case(branch, mode, "legacy", edit=legacy_edit)
        else:
            raise RuntimeError("Unknown regression group")

    groups = ("admission", "integrity", "placeholders", "rust_anchors", "qml_anchors", "controls") if args.group == "all" else (args.group,)
    for group in groups:
        run_group(group)
    final_identity = real_preflight(args.expected_boot)
    receipt = {"scope": "Synthetic child boundary qualification only; no real Rust/Qt/GUI", "input_hashes": inputs,
               "preflight": identity, "postflight": final_identity, "group": args.group, "cases": rows,
               "counts": {"total": len(rows), "passed": sum(r["status"] == "PASS" for r in rows),
                          "failed": sum(r["status"] == "FAIL" for r in rows), "skipped": 0}}
    write_json(out / "receipt.json", receipt)
    print(json.dumps(receipt["counts"]), flush=True)
    return 1 if receipt["counts"]["failed"] else 0


if __name__ == "__main__":
    sys.exit(main())
