#!/usr/bin/env python3
"""Guest-only focused regressions; not a full Qt/application build.

Extract exact bridge methods into a Rust scaffold that replaces external I/O,
not admission logic. Preserve source/region/generated hashes and every exit.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys

from tab_model_regressions import require_vm, replace_once
import tab_model_regressions as guards


def sha(data):
    return hashlib.sha256(data).hexdigest()


def unique_index(text, anchor):
    if text.count(anchor) != 1:
        raise ValueError(f"Source requires exactly one anchor: {anchor}")
    return text.index(anchor)


def block(source, start):
    # Anchors are unique and these bounded methods have balanced Rust/QML braces.
    opening = source.index("{", start)
    header = source[start:opening]
    if header.lstrip().startswith("fn "):
        if not re.fullmatch(r"\s*fn \w+\([^{};]*\)\s*(?:->[^{};]+)?\s*", header):
            raise ValueError("Malformed Rust implementation anchor")
    elif "\n" in header.rstrip() or ";" in header:
        raise ValueError("Source anchor requires its own braced header")
    indent = len(header) - len(header.lstrip(" "))
    boundary = re.search(r"(?m)^ {" + str(indent) + r"}(?:fn|function)\b|^ {0," + str(max(0, indent - 1)) + r"}\}", source[opening + 1:])
    limit = opening + 1 + boundary.start() if boundary else len(source)
    depth = 0
    for i in range(opening, limit):
        if source[i] == "{":
            depth += 1
        elif source[i] == "}":
            depth -= 1
            if depth == 0:
                return source[start:i + 1]
    raise ValueError("Unclosed source region before the next declaration/owner boundary")


def qml_scaffold(source, receipt):
    main_qml = (source / "ui/Main.qml").read_text()
    dialog_qml = (source / "ui/ConflictDialog.qml").read_text()
    def region(text, anchor, name, base_line=0):
        start = unique_index(text, anchor)
        result = block(text, start)
        receipt["regions"].append({"name": name, "line": base_line + text[:start].count("\n") + 1, "sha256": sha(result.encode())})
        return result
    functions = []
    for name in ["transferStarted", "pasteFinished", "showNextConflict", "queueConflict", "drainConflicts", "forgetConflictOwner", "conflictOwnerLive"]:
        anchor = "    function " + name + "("
        if name in ("transferStarted", "pasteFinished", "showNextConflict") or anchor in main_qml:
            functions.append(region(main_qml, anchor, "Main." + name))
    handlers = []
    conflict_start = unique_index(main_qml, "    ConflictDialog {")
    confirm_start = unique_index(main_qml, "    ConfirmDialog {")
    conflict_section = block(main_qml, conflict_start)
    if conflict_start + len(conflict_section) > confirm_start:
        raise ValueError("ConflictDialog must precede the ConfirmDialog boundary")
    conflict_base = main_qml[:conflict_start].count("\n")
    for handler, params in [("onAnswered", "choice, all"), ("onCancelled", ""), ("onClosed", "")]:
        anchor = "        " + handler + ":"
        start = unique_index(conflict_section, anchor)
        if handler == "onClosed" and "{" not in conflict_section[start:].split("\n")[0]:
            original = conflict_section[start:].split("\n")[0]
            if not original.split(":", 1)[1].strip() or original.split(":", 1)[1].strip().startswith(";"):
                raise ValueError("Malformed inline onClosed anchor")
            receipt["regions"].append({"name": "Main.ConflictDialog." + handler, "line": conflict_base + conflict_section[:start].count("\n") + 1, "sha256": sha(original.encode())})
            body = "{ " + original.split(":", 1)[1].strip() + " }"
        else:
            original = region(conflict_section, anchor, "Main.ConflictDialog." + handler, conflict_base)
            body = original[original.index("{"):]
        handlers.append("    function handle" + handler[2:] + "(" + params + ") " + body)
    anchor = "        onObjectRemoved:"
    start = unique_index(main_qml, anchor)
    if "{" in main_qml[start:].split("\n")[0]:
        original = region(main_qml, anchor, "Main.tabs.onObjectRemoved")
        removed_body = original[original.index("{"):]
    else:
        original = main_qml[start:].split("\n")[0]
        if not original.split(":", 1)[1].strip() or original.split(":", 1)[1].strip().startswith(";") or re.search(r"=>\s*(?:;|$)", original):
            raise ValueError("Malformed inline onObjectRemoved anchor")
        receipt["regions"].append({"name": "Main.tabs.onObjectRemoved", "line": main_qml[:start].count("\n") + 1, "sha256": sha(original.encode())})
        removed_body = "{ " + original.split(":", 1)[1].strip() + " }"
    handlers.append("    function handleObjectRemoved(index, object) " + removed_body)
    escape_start = unique_index(main_qml, "            case Qt.Key_Escape:")
    escape_end = unique_index(main_qml, "                if (chooserBar.active")
    if escape_end <= escape_start:
        raise ValueError("Malformed Escape source boundaries")
    if not re.fullmatch(r"\s*if \([^{};]+\) \{", main_qml[escape_end:].split("\n", 1)[0]):
        raise ValueError("Malformed Escape end anchor")
    escape = main_qml[escape_start:escape_end]
    escape = escape[escape.index("\n") + 1:]
    receipt["regions"].append({"name": "Main.Escape_job_read_branches", "sha256": sha(escape.encode())})
    dialog_functions = []
    for name in ["show", "close", "pick", "cancel"]:
        method = region(dialog_qml, "    function " + name + "(", "ConflictDialog." + name)
        # Exact function body; root is bound to this scaffold dialog below.
        dialog_functions.append(method)
    return '''import QtQuick
import QtTest
TestCase {
    id: testCase
    name: "AsyncClipboard"
    QtObject { id: hints; property string message: "" }
    QtObject { id: jobTimer; property int starts: 0; function start() { starts++ } }
    QtObject { id: quickLook; property bool open: false; function hide() { open = false } }
    QtObject { id: settingsPanel; property bool open: false; function hide() { open = false } }
    QtObject { id: keepButton; function forceActiveFocus() {} }
    QtObject { id: tabList; readonly property int count: win.liveModels.length }
    QtObject { id: tabs; function objectAt(index) { return win.liveModels[index] || null } }
    QtObject {
        id: dirModel
        property bool running: false
        property bool reading: false
        property int jobCancels: 0
        property int readCancels: 0
        function job_running() { return running }
        function clipboard_busy() { return reading }
        function cancel_job() { jobCancels++ }
        function cancel_clip_paste() { readCancels++; reading = false }
    }
    QtObject {
        id: win
        property var conflictModel: null
        property var conflictQueue: []
        property var liveModels: []
        property string alertText: ""
        function qaDump(tag) {}
        function focusList() {}
        function refreshActiveModel() {}
        function pasteImage(model) { throw new Error("unexpected image fallback") }

''' + "\n".join(functions + handlers) + "\n    function handleEscape(event) {\n" + escape + '''    }
    }
    QtObject {
        id: conflictDialog
        property var root: conflictDialog
        property var clash: null
        property bool visible: false
        property bool applyAll: false
        signal answered(string choice, bool all)
        signal cancelled()
        signal closed()
        onAnswered: (choice, all) => win.handleAnswered(choice, all)
        onCancelled: win.handleCancelled()
        onClosed: win.handleClosed()
''' + "\n".join(dialog_functions) + "\n    }\n" + (source / "ui/tests/tst_async_clipboard.qml.in").read_text() + "\n}\n"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--source", type=Path, required=True)
    ap.add_argument("--output", type=Path, required=True)
    ap.add_argument("--filter", default="p1_")
    ap.add_argument("--qml", action="store_true")
    ap.add_argument("--format", action="store_true")
    args = ap.parse_args()
    require_vm()
    virt = "kvm"  # require_vm performed the bounded, fail-closed detector call.
    args.output.mkdir(parents=True, exist_ok=False)
    receipt = {
        "qualification": "exact-method Rust bridge scaffold; Qt transport/desktop I/O/job execution are seams",
        "uid": os.getuid(), "euid": os.geteuid(), "virt": virt,
        "boot_id": Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
        "source": str(args.source), "source_hashes": {}, "regions": [], "checks": [],
    }
    for rel in ["crates/app/src/bridge.rs", "crates/app/src/clipboard.rs", "ui/Main.qml", "ui/ConflictDialog.qml", "crates/app/tests/clipboard_bridge_scaffold.rs.in", "crates/app/tests/async_clipboard_regressions.py", "crates/app/tests/tab_model_regressions.py"]:
        receipt["source_hashes"][rel] = sha((args.source / rel).read_bytes())
    # Bind the bytes actually executing/imported, not only --source's copies.
    receipt["harness_hashes"] = {str(path): sha(path.read_bytes()) for path in
                                 [Path(__file__).resolve(), Path(guards.__file__).resolve()]}
    if args.qml:
        rel = "ui/tests/tst_async_clipboard.qml.in"
        receipt["source_hashes"][rel] = sha((args.source / rel).read_bytes())
    bridge = (args.source / "crates/app/src/bridge.rs").read_text()
    names = ["clip_copy", "clip_cut", "copy_path", "clip_clear", "paste_into", "clipboard_busy", "cancel_clip_paste", "apply_clip_paste", "begin_transfer", "launch", "set_clip", "clear_clip", "duplicate_row", "paste_image"]
    if "    fn forget_clip(" in bridge:
        names.append("forget_clip")
    methods = []
    for name in names:
        anchor = "    fn " + name + "("
        # Permit one FFI declaration, but require exactly one implementation.
        implementations = []
        declarations = 0
        for match in re.finditer(re.escape(anchor), bridge):
            at = match.start()
            terminator = re.search(r"[{};]", bridge[at:])
            if terminator is None:
                raise ValueError(f"Malformed Rust source anchor: {name}")
            end = at + terminator.start()
            header = bridge[at:end]
            if not re.fullmatch(r"\s*fn \w+\([^{};]*\)\s*(?:->[^{};]+)?\s*", header):
                raise ValueError(f"Malformed Rust source anchor: {name}")
            if terminator.group() == ";":
                declarations += 1
            elif terminator.group() == "{":
                implementations.append(at)
            else:
                raise ValueError(f"Malformed Rust source anchor: {name}")
        if len(implementations) != 1 or declarations > 1:
            raise ValueError(f"Rust source requires exactly one implementation and at most one declaration: {name}")
        start = implementations[0]
        method = block(bridge, start)
        receipt["regions"].append({"name": name, "line": bridge[:start].count("\n") + 1, "sha256": sha(method.encode())})
        methods.append(method)
    helper = (args.source / "crates/app/src/clipboard.rs").read_bytes()
    (args.output / "clipboard.rs").write_bytes(helper)
    legacy = b"pub fn with_generation" not in helper
    factory = "fn gate_pair() -> (clipboard::ReadGate, clipboard::ReadGate) {\n"
    if legacy:
        factory += "    (clipboard::ReadGate::default(), clipboard::ReadGate::default())\n"
    else:
        factory += "    let generation = Arc::new(std::sync::atomic::AtomicU64::new(0));\n    (clipboard::ReadGate::with_generation(generation.clone()), clipboard::ReadGate::with_generation(generation))\n"
    factory += "}\n"
    receipt["legacy_gate_adapter"] = legacy
    scaffold = (args.source / "crates/app/tests/clipboard_bridge_scaffold.rs.in").read_text()
    scaffold = replace_once(scaffold, "    // PRODUCTION_METHODS", "\n\n".join(methods))
    scaffold = replace_once(scaffold, "// TEST_GATE_FACTORY", factory)
    (args.output / "bridge_scaffold.rs").write_text(scaffold)
    receipt["generated_sha256"] = sha(scaffold.encode())
    fixture = args.output / "fixture"
    fixture.write_bytes(b"unchanged source\n")
    for directory in ("home", "tmp"):
        (args.output / directory).mkdir(mode=0o700)
    env = {
        "PATH": "/usr/bin:/bin", "LANG": "C.UTF-8",
        "HOME": str(args.output / "home"), "TMPDIR": str(args.output / "tmp"),
        "WAYLAND_DISPLAY": "synthetic-no-desktop-io", "GUST_TEST_SOURCE": str(fixture),
    }
    def check(name, argv):
        started = datetime.now(timezone.utc).isoformat()
        log = args.output / (name + ".log")
        with log.open("wb") as stream:
            result = subprocess.run(argv, env=env, stdout=stream, stderr=subprocess.STDOUT)
        row = {"name": name, "argv": argv, "exit_code": result.returncode, "log": str(log), "log_sha256": sha(log.read_bytes()), "started_at": started, "finished_at": datetime.now(timezone.utc).isoformat()}
        row["boot_id_after"] = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
        def after_hash(path):
            try:
                return sha(path.read_bytes())
            except OSError:
                return None  # Retain a failed read in the integrity receipt.
        row["source_hashes_after"] = {rel: after_hash(args.source / rel) for rel in receipt["source_hashes"]}
        row["harness_hashes_after"] = {path: after_hash(Path(path)) for path in receipt["harness_hashes"]}
        row["source_unchanged"] = (row["source_hashes_after"] == receipt["source_hashes"]
                                   and row["harness_hashes_after"] == receipt["harness_hashes"])
        receipt["checks"].append(row)
        (args.output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
        print(json.dumps(row), flush=True)
        print(log.read_text(), flush=True)
        if row["boot_id_after"] != receipt["boot_id"] or not row["source_unchanged"]:
            raise RuntimeError("QA boot or source changed during runner execution")
        return result.returncode
    if args.format:
        receipt["qualification"] = "rustfmt check of the exact selected production Rust files only"
        return check("rustfmt", ["rustfmt", "--edition", "2021", "--check", str(args.source / "crates/app/src/clipboard.rs"), str(args.source / "crates/app/src/bridge.rs")])
    if args.qml:
        receipt["qualification"] = "exact Main/ConflictDialog JavaScript in Qt QML engine; visual objects/native DirModels are seams"
        qml = qml_scaffold(args.source, receipt)
        (args.output / "tst_async_clipboard.qml").write_text(qml)
        receipt["qml_generated_sha256"] = sha(qml.encode())
        env.pop("WAYLAND_DISPLAY", None)
        env["QT_QPA_PLATFORM"] = "offscreen"
        env["QSG_RHI_BACKEND"] = "software"
        return check("qml_tests", ["/usr/lib/qt6/bin/qmltestrunner", "-input", str(args.output / "tst_async_clipboard.qml")])
    code = check("compile", ["rustc", "--edition=2021", "--test", str(args.output / "bridge_scaffold.rs"), "-o", str(args.output / "bridge_tests")])
    if not code:
        receipt["test_binary_sha256"] = sha((args.output / "bridge_tests").read_bytes())
        code = check("tests", [str(args.output / "bridge_tests"), args.filter, "--nocapture"])
    (args.output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    return code


if __name__ == "__main__":
    sys.exit(main())