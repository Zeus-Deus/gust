#!/usr/bin/env python3
"""Display-free Qt preview navigation + static queued-relist admission contract.

Executes source-exact QML methods, key branch, bindings and hold notification.
DirModel/watcher/renderer are boundary doubles, not native Rust or desktop QA.
The queued apply_live guard is checked statically, never simulated as Rust proof.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys


def sha(data):
    return hashlib.sha256(data).hexdigest()


def matched(pattern, text):
    match = re.search(pattern, text, re.S)
    if match is None:
        raise ValueError(f"Missing source binding: {pattern}")
    return match.group()


def block(text, marker):
    start = text.index(marker)
    brace = text.index("{", start)
    depth = 0
    for at in range(brace, len(text)):
        if text[at] == "{":
            depth += 1
        elif text[at] == "}":
            depth -= 1
            if depth == 0:
                return text[start:at + 1]
    raise ValueError(f"Unclosed source block: {marker}")


FIXTURE = r'''
import QtQuick
import QtTest
TestCase {
    id: test
    name: "PreviewNavigation"
    property var quickLook: root
    property var dirModel: model
    QtObject { id: palette; property bool visible: false }
    QtObject { id: confirm; property bool visible: false }
    QtObject { id: connectionRecovery; property bool visible: false }
    QtObject { id: settingsPanel; property bool open: false }
    QtObject { id: trashPanel; property bool open: false }
    QtObject { id: recentPanel; property bool open: false }
    QtObject { id: connectPanel; property bool open: false }
    QtObject { id: deviceSharing; property bool open: false }
    QtObject { id: contextMenu; property bool open: false }
    QtObject { id: conflictDialog; property bool visible: false }
    QtObject { id: prompt; property bool visible: false; property bool claimingFocus: false }
    QtObject { id: tabList; property int count: 1 }
    QtObject { id: tabs; function objectAt(i) { return model } }
    QtObject {
        id: win
        @HOLD@
        @HOLD_HANDLER@
        function qaDump(label) {}
        function activateRow(row) { fail("Unexpected external activation") }
        function key(event) {
            @KEYS@
            return false
        }
    }
    QtObject { id: audioPoll; function stop() {} function start() {} }
    QtObject {
        id: model
        property var names: []
        property var pending: null
        property int cursor: 0
        property bool held: false
        property string peek_visual: ""
        property var pageRequests: []
        property int visualRequests: 0
        function relist(next) {
            const old = names[cursor]
            names = next
            cursor = Math.max(0, names.indexOf(old))
        }
        // Explicit boundary double of Contents hold/release. Already-queued
        // native completion is NOT exercised here; see the static guard check.
        function watch(next) { if (held) pending = next; else relist(next) }
        function hold_live_updates(hold) {
            held = hold
            if (!hold && pending !== null) {
                const next = pending
                pending = null
                relist(next)
            }
        }
        // All these fixtures are regular, browse-eligible files. This does
        // not reimplement or qualify Rust's media/program/link filtering.
        function preview_neighbor(row, delta) {
            if (row < 0 || row >= names.length) return row
            return Math.max(0, Math.min(names.length - 1, row + delta))
        }
        function focus_row(row) { cursor = row }
        function peek_row(row) {
            return JSON.stringify({ok: true, name: names[row], kind: "File",
                                   kindWords: /\.pdf$/i.test(names[row]) ? "PDF document" : "Image"})
        }
        function request_peek_visual(row) { peek_visual = ""; visualRequests++ }
        function request_peek_page(row, page) { pageRequests = pageRequests.concat([[row, page]]) }
        function audio_stop() {}
        function hash_cancel() {}
        function audio_seek(pos) {}
        function audio_toggle() {}
    }
    Item {
        id: root
        property bool open: false
        property var info: null
        property int row: -1
        property real imageZoom: 1
        property var model: test.dirModel
        property var audio: null
        signal closed()
        @PROPERTIES@
        @METHODS@
    }
    function init() {
        root.hide()
        model.names = ["a.png", "b.png", "c.png"]
        model.pending = null
        model.cursor = 0
        model.peek_visual = ""
        model.pageRequests = []
        model.visualRequests = 0
        verify(!win.liveHold)
    }
    function showAt(row) {
        model.cursor = row
        root.show(model.peek_row(row), row)
    }
    function press(key) {
        const event = {key: key, modifiers: Qt.NoModifier, accepted: false}
        win.key(event)
        verify(event.accepted, "Preview must consume the key")
    }
    function test_watch_data() {
        return [
            {tag: "remove-earlier-left", next: ["b.png", "c.png"], key: Qt.Key_Left, expected: "b.png", row: 1},
            {tag: "remove-earlier-right", next: ["b.png", "c.png"], key: Qt.Key_Right, expected: "c.png", row: 2},
            {tag: "insert-earlier-left", next: ["0.png", "a.png", "b.png", "c.png"], key: Qt.Key_Left, expected: "b.png", row: 1},
            {tag: "insert-earlier-right", next: ["0.png", "a.png", "b.png", "c.png"], key: Qt.Key_Right, expected: "c.png", row: 2}
        ]
    }
    function test_watch(data) {
        showAt(2)
        model.watch(data.next)
        press(data.key)
        compare(root.info.name, data.expected, "Browse must use the held visible order")
        compare(root.row, data.row)
        compare(model.cursor, data.row)
        compare(model.names, ["a.png", "b.png", "c.png"], "Sibling change must wait")
        verify(model.held)
        root.hide()
        verify(!model.held)
        compare(model.names, data.next, "Closing must release the pending change")
        compare(model.pending, null)
    }
    function test_pdf_data() {
        let rows = []
        for (const state of ["loading", "single", "failed"])
            for (const key of [Qt.Key_Left, Qt.Key_Right, Qt.Key_PageUp, Qt.Key_PageDown])
                rows.push({tag: state + "-" + key, state: state, key: key})
        return rows
    }
    function test_pdf(data) {
        // No invented MIME field: use the real peek name/kind/kindWords shape.
        model.names = ["before.png", "sanitised·name.PdF", "after.png"]
        showAt(1)
        if (data.state === "single") model.peek_visual = JSON.stringify({image: "fixture", page: 1, pages: 1})
        if (data.state === "failed") model.peek_visual = JSON.stringify({error: "Cannot preview"})
        press(data.key)
        compare(root.info.name, "sanitised·name.PdF", "PDF owns arrows even without multiple pages")
        compare(root.row, 1)
        compare(model.cursor, 1)
        compare(model.visualRequests, 1, "Do not request a neighbouring preview")
        compare(model.pageRequests, [], "No page requests without a possible page turn")
    }
    function test_pdf_pages_and_clamps() {
        model.names = ["before.png", "a.pdf", "after.png"]
        showAt(1)
        model.peek_visual = JSON.stringify({image: "fixture", page: 1, pages: 2})
        press(Qt.Key_Left)
        compare(model.pageRequests, [])
        press(Qt.Key_Right)
        compare(model.pageRequests, [[1, 2]])
        model.peek_visual = JSON.stringify({image: "fixture", page: 2, pages: 2})
        press(Qt.Key_Right)
        compare(model.pageRequests, [[1, 2]])
        press(Qt.Key_PageUp)
        compare(model.pageRequests, [[1, 2], [1, 1]])
        compare(root.info.name, "a.pdf")
        compare(model.cursor, 1)
    }
    function test_non_pdf_browses() {
        model.names = ["a.png", "not.pdf.png", "z.txt"]
        showAt(1)
        press(Qt.Key_Right)
        compare(root.info.name, "z.txt")
        compare(root.row, 2)
        compare(model.cursor, 2)
        compare(model.pageRequests, [])
        press(Qt.Key_Right)
        compare(root.info.name, "z.txt")
        root.hide()
        verify(!root.turnPage(1))
        verify(!root.browse(1))
    }
    function test_other_overlay_keeps_hold_after_preview_closes() {
        showAt(0)
        prompt.visible = true
        root.hide()
        verify(win.liveHold)
        verify(model.held)
        prompt.visible = false
        verify(!win.liveHold)
        verify(!model.held)
    }
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--runner", type=Path, help="Qt 6 qmltestrunner")
    args = parser.parse_args()
    source, output = args.source.resolve(), args.output.resolve()
    if output.exists():
        raise RuntimeError("Use a new evidence directory")
    runner = str(args.runner) if args.runner else shutil.which("qmltestrunner6")
    if not runner and Path("/usr/lib/qt6/bin/qmltestrunner").is_file():
        runner = "/usr/lib/qt6/bin/qmltestrunner"
    if not runner:
        runner = shutil.which("qmltestrunner")
    if not runner:
        raise RuntimeError("Qt 6 qmltestrunner required")
    paths = ["ui/Main.qml", "ui/QuickLook.qml", "crates/app/src/bridge.rs"]
    snapshots = {p: (source / p).read_bytes() for p in paths}
    main_qml, quick, bridge = (snapshots[p].decode() for p in paths)
    hold = matched(r"readonly property bool liveHold:.*?(?=\n    onLiveHoldChanged:)", main_qml)
    keys_start = main_qml.index("if (quickLook.open && event.modifiers === Qt.ShiftModifier")
    keys_end = main_qml.index("// Preset-aware chords", keys_start)
    methods = [block(quick, "function " + n + "(") for n in
               ["show", "browse", "zoomBy", "turnPage", "togglePlay", "seekBy", "hide"]]
    properties = [block(quick, "readonly property var visual:")]
    for name, kind in [("page", "int"), ("pages", "int"), ("isAudio", "bool")]:
        properties.append(matched(r"readonly property " + kind + " " + name + r":[^\n]*", quick))
    bindings = {"HOLD": hold, "HOLD_HANDLER": block(main_qml, "onLiveHoldChanged:"),
                "KEYS": main_qml[keys_start:keys_end], "PROPERTIES": "\n".join(properties),
                "METHODS": "\n".join(methods)}
    fixture = FIXTURE
    for key, value in bindings.items():
        assert fixture.count("@" + key + "@") == 1
        fixture = fixture.replace("@" + key + "@", value)
    # Source-level admission check, explicitly not a native execution claim.
    apply_live = block(bridge, "fn apply_live(")
    guard = re.search(r"if self\.as_ref\(\)\.live_hold\s*\{\s*self\.as_mut\(\)\.rust_mut\(\)\.live_dirty = true;\s*return;\s*\}", apply_live)
    queued_guard = bool(guard and apply_live.index("live_busy = false;") < guard.start()
                        < apply_live.index("self.as_mut().rust_mut().thumb_cancel.cancel();"))
    output.mkdir(parents=True, mode=0o700)
    for path, data in snapshots.items():
        target = output / "source" / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    (output / "test.py").write_bytes(Path(__file__).read_bytes())
    (output / "bindings.json").write_text(json.dumps(bindings, indent=2) + "\n")
    qml = output / "tst_preview_navigation.qml"
    qml.write_text(fixture)
    for name in ["home", "tmp", "runtime", "config", "data", "cache"]:
        (output / name).mkdir(mode=0o700)
    env = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "HOME": str(output / "home"),
           "TMPDIR": str(output / "tmp"), "XDG_RUNTIME_DIR": str(output / "runtime"),
           "XDG_CONFIG_HOME": str(output / "config"), "XDG_DATA_HOME": str(output / "data"),
           "XDG_CACHE_HOME": str(output / "cache"), "QT_QPA_PLATFORM": "offscreen",
           "QT_QUICK_BACKEND": "software", "QSG_RENDER_LOOP": "basic",
           "QT_FORCE_STDERR_LOGGING": "1", "QT_ASSUME_STDERR_HAS_CONSOLE": "1"}
    argv = [runner, "-input", str(qml)]
    result = subprocess.run(argv, env=env, cwd=output, capture_output=True, timeout=45)
    (output / "run.stdout").write_bytes(result.stdout)
    (output / "run.stderr").write_bytes(result.stderr)
    log = (result.stdout + result.stderr).decode("utf-8", errors="backslashreplace")
    (output / "run.log").write_text(log)
    unchanged = all((source / p).read_bytes() == b for p, b in snapshots.items())
    exit_code = 0 if result.returncode == 0 and queued_guard and unchanged else 1
    receipt = {"source": str(source), "argv": argv, "qt_exit": result.returncode,
               "exit": exit_code, "source_hashes": {p: sha(b) for p, b in snapshots.items()},
               "test_sha256": sha(Path(__file__).read_bytes()), "source_unchanged": unchanged,
               "queued_apply_live_guard_static": queued_guard,
               "qml_sha256": sha(qml.read_bytes()), "log_sha256": sha(log.encode()),
               "scope": (__doc__ or "").strip(), "env": env}
    (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(log, end="")
    print("STATIC queued apply_live hold admission: " + ("PASS" if queued_guard else "FAIL"))
    print(json.dumps(receipt, indent=2))
    return exit_code


if __name__ == "__main__":
    sys.exit(main())
