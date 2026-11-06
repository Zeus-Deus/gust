#!/usr/bin/env python3
"""Exercise production Stage 2 views and exact Main.qml interaction guards.

Snapshots ui/ from --source into a new --output directory, derives the Tokens
singleton from the real ui/Tokens.qml (only the Rust ThemeTokens object is
replaced by fixed values), extracts exact production functions/bindings from
ui/Main.qml into the Main fixture, and runs qmltestrunner offscreen/software.
"""
from pathlib import Path
import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys

THEME = '''    property QtObject tokens: QtObject {
        property int revision: 0
        property int icons_rev: 0
        property string name: "fixture"
        property bool is_light: false
        function color(k) { return "#808080" }
        function metric(k) {
            return ({ font_base: 12, spacing_scale: 1, selected_background_alpha: 0.2,
                      normal_border_alpha: 0.5, border_width: 1 })[k] || 0
        }
        function icon_look(k) { return "" }
        function reload() {}
        function watch_theme() {}
    }'''
THEME_SOURCE = '''    property ThemeTokens tokens: ThemeTokens {
        Component.onCompleted: watch_theme()
    }'''

MAIN_FUNCTIONS = ["selectionSettling", "askBulkRename", "askBulkPattern", "askRename", "askOwner",
                  "askTrash", "openContextMenu", "popupContextMenu", "chooserAccept", "activateRow"]
TARGET_FUNCTIONS = ["navigateTarget", "rowTarget", "hereTarget", "rowGone", "openPathInPane", "newTab",
                    "openRowInBackgroundTab", "armSplitTab", "noteSplitTabMoved", "splitPane", "sendToOtherPane",
                    "lastSegment", "firstTabOf", "paneModel", "paneViewMode", "paneTabs", "tabModelAt"]


def mask(text):
    """Blank comments and strings, keeping offsets, so braces inside them don't count."""
    return re.sub(r'//[^\n]*|/\*[\s\S]*?\*/|"(?:\\.|[^"\\\n])*"|\'(?:\\.|[^\'\\\n])*\'',
                  lambda m: "".join("\n" if c == "\n" else " " for c in m[0]), text)


def block(text, marker):
    masked = mask(text)
    starts = [m.start() for m in re.finditer(re.escape(marker), text)]
    if len(starts) != 1:
        raise ValueError(f"expected one {marker!r}, found {len(starts)}")
    start = starts[0]
    depth = 0
    for at in range(masked.index("{", start), len(masked)):
        if masked[at] == "{":
            depth += 1
        elif masked[at] == "}":
            depth -= 1
            if depth == 0:
                return text[start:at + 1]
    raise ValueError(f"unclosed {marker!r}")


def digest(data):
    return hashlib.sha256(data).hexdigest()


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--source", type=Path, required=True)
    ap.add_argument("--output", type=Path, required=True)
    ap.add_argument("--only", choices=["views", "main", "pane_focus", "targets", "qa_probe", "menu", "release"], default=None)
    args = ap.parse_args()
    source, out = args.source.resolve(), args.output.resolve()
    if out.exists():
        raise SystemExit("use a new evidence directory")
    out.mkdir(parents=True, mode=0o700)
    ui_files = sorted(p for p in (source / "ui").glob("*.qml"))
    stage2 = sorted((source / "ui/tests/stage2").iterdir())
    tracked = [p for p in ui_files + stage2 if p.is_file()]
    tracked += [source / name for name in ("ui/tests/tst_pane_focus.qml.in",
        "crates/app/tests/pane_focus_regressions.py", "crates/app/tests/tab_model_regressions.py")]
    before = {str(p.relative_to(source)): digest(p.read_bytes()) for p in tracked}

    # The ui files become module com.gust.app, as in production, so files
    # that use Tokens without importing the module (WheelScroll) resolve.
    module = out / "imports/com/gust/app"
    module.mkdir(parents=True)
    ui = module
    for p in ui_files:
        if p.name != "Tokens.qml":
            shutil.copy2(p, ui / p.name)
    tokens = (source / "ui/Tokens.qml").read_text()
    if tokens.count(THEME_SOURCE) != 1:
        raise SystemExit("Tokens.qml ThemeTokens block changed; update the runner")
    tokens = tokens.replace(THEME_SOURCE, THEME).replace("import com.gust.app\n", "")
    (module / "Tokens.qml").write_text(tokens)
    types = "".join(f"{p.stem} 1.0 {p.name}\n" for p in ui_files if p.name != "Tokens.qml")
    (module / "qmldir").write_text("module com.gust.app\nsingleton Tokens 1.0 Tokens.qml\n" + types)

    tests = out / "tests"
    tests.mkdir()
    shutil.copy2(source / "ui/tests/stage2/FakeDirModel.qml", tests / "FakeDirModel.qml")
    views = (source / "ui/tests/stage2/tst_stage2_views.qml.in").read_text()
    (tests / "tst_stage2_views.qml").write_text(views)

    main_qml = (source / "ui/Main.qml").read_text()
    hold = re.search(r"readonly property bool liveHold:.*?(?=\n    onLiveHoldChanged:)", main_qml, re.S)
    if not hold:
        raise SystemExit("liveHold binding not found")
    bindings = {
        "@FUNCTIONS@": "\n".join(block(main_qml, "    function " + n + "(") for n in MAIN_FUNCTIONS),
        "@HOLD@": hold[0],
        "@HOLD_HANDLER@": block(main_qml, "onLiveHoldChanged:"),
    }
    fixture = (source / "ui/tests/stage2/tst_stage2_main.qml.in").read_text()
    for key, value in bindings.items():
        if fixture.count(key) != 1:
            raise SystemExit(f"fixture placeholder {key}")
        fixture = fixture.replace(key, value)
    (tests / "tst_stage2_main.qml").write_text(fixture)
    split_state = re.search(r'    property var splitMadeModel: null\n.*?property string splitMadeAt: ""\n', main_qml, re.S)
    if not split_state:
        raise SystemExit("split state not found")
    target_bindings = {
        "@FUNCTIONS@": "\n".join(block(main_qml, "    function " + n + "(") for n in TARGET_FUNCTIONS),
        "@SPLIT_STATE@": split_state[0],
        "@OPEN_ROW_BRANCH@": block(main_qml, 'else if (a === "open-tab" || a === "open-other-pane")'),
    }
    targets = (source / "ui/tests/stage2/tst_stage2_targets.qml.in").read_text()
    for key, value in target_bindings.items():
        if targets.count(key) != 1:
            raise SystemExit(f"targets placeholder {key}")
        targets = targets.replace(key, value)
    (tests / "tst_stage2_targets.qml").write_text(targets)
    bindings.update({"targets" + k: v for k, v in target_bindings.items()})
    qa_state = re.search(r'    Timer \{ id: qaSettle;[^\n]*\n.*?    Timer \{ id: qaPending;[^\n]*\n', main_qml, re.S)
    if not qa_state:
        raise SystemExit("qa probe state not found")
    qa_state = "\n".join(line for line in qa_state[0].splitlines() if not line.strip().startswith("//")) + "\n"
    qa_bindings = {
        "@QA_STATE@": qa_state,
        "@QA_FUNCTIONS@": "\n".join(block(main_qml, "    function " + n + "(") for n in ["qaDump", "qaTakePending", "qaFlush"]),
    }
    probe = (source / "ui/tests/stage2/tst_stage2_qa_probe.qml.in").read_text()
    for key, value in qa_bindings.items():
        if probe.count(key) != 1:
            raise SystemExit(f"qa probe placeholder {key}")
        probe = probe.replace(key, value)
    (tests / "tst_stage2_qa_probe.qml").write_text(probe)
    bindings.update({"qa" + k: v for k, v in qa_bindings.items()})
    pane_a = main_qml[main_qml.index("                    id: paneA\n"):main_qml.index("                    id: paneBLoader\n")]
    menu_start = main_qml.index("    function openContextMenu(")
    menu_block = block(main_qml, "    function menuIntentSettled(")
    menu_end = main_qml.index(menu_block) + len(menu_block)
    menu_bindings = {
        "@HOLD@": hold[0],
        "@HOLD_HANDLER@": block(main_qml, "onLiveHoldChanged:"),
        "@DIRMODEL_HANDLER@": block(main_qml, "onDirModelChanged:"),
        "@SELECTION_SETTLING@": block(main_qml, "    function selectionSettling("),
        "@MENU@": main_qml[menu_start:menu_end],
        "@PANE_CONTEXT@": block(pane_a, "onContextRequested:"),
        "@PANE_SELECTION@": block(pane_a, "onSelectionTouched:"),
    }
    menu = (source / "ui/tests/stage2/tst_stage2_menu.qml.in").read_text()
    for key, value in menu_bindings.items():
        if menu.count(key) != 1:
            raise SystemExit(f"menu placeholder {key}")
        menu = menu.replace(key, value)
    (tests / "tst_stage2_menu.qml").write_text(menu)
    bindings.update({"menu" + k: v for k, v in menu_bindings.items()})
    drop_start = main_qml.index("    /// Where a right-button drag was released")
    drop_block = block(main_qml, "    function openDropMenu(")
    drop_end = main_qml.index(drop_block) + len(drop_block)
    release_bindings = {
        "@DROP_COPY_BRANCH@": block(main_qml, 'else if (a === "drop-copy" || a === "drop-move")'),
        "@DROP_LINK_BRANCH@": block(main_qml, 'else if (a === "drop-link")'),
        "@HOLD@": hold[0],
        "@HOLD_HANDLER@": block(main_qml, "onLiveHoldChanged:"),
        "@DROP_MENU@": main_qml[drop_start:drop_end],
        "@PANE_DROP_MENU@": block(pane_a, "onDropMenu:"),
        "@CONTEXT_MENU@": block(main_qml, "    ContextMenu {\n        id: contextMenu\n"),
        "@COPY_BRANCH@": main_qml[main_qml.index('        else if (a === "copy")'):].split("\n", 1)[0].strip(),
        "@ASK_RUN@": block(main_qml, "    function askRun("),
        "@POPUP_MENU@": block(main_qml, "    function popupContextMenu("),
        "@BULK@": "\n".join([re.search(r'    property string bulkPattern: ""\n    property string bulkLines: ""\n    property string bulkSummary: ""\n', main_qml)[0],
                             block(main_qml, "    function bulkPreview("),
                             block(main_qml, "    Timer {\n        id: bulkConfirmAfterPrompt\n"),
                             re.search(r"    property bool bulkConfirmPending: false\n", main_qml)[0]]),
        "@RUN_HANDLER@": (lambda h: h[h.index("{"):])(block(main_qml, "onRunRequested:")),
        "@RENAME_CARD@": block(main_qml, "    RenameCard {\n        id: renameCard\n"),
    }
    release = (source / "ui/tests/stage2/tst_stage2_release.qml.in").read_text()
    for key, value in release_bindings.items():
        if release.count(key) != 1:
            raise SystemExit(f"release placeholder {key}")
        release = release.replace(key, value)
    (tests / "tst_stage2_release.qml").write_text(release)
    bindings.update({"release" + k: v for k, v in release_bindings.items()})
    (out / "bindings.json").write_text(json.dumps(bindings, indent=2) + "\n")
    # Reuse the app runner's exact handler locator without invoking its VM/
    # desktop entry point. This runner exercises only offscreen boundary doubles.
    sys.dont_write_bytecode = True
    sys.path.insert(0, str(source / "crates/app/tests"))
    from pane_focus_regressions import pane_selection_handler
    from tab_model_regressions import block as app_block, replace_once
    columns = (source / "ui/ColumnsView.qml").read_text()
    pane = (source / "ui/Pane.qml").read_text()
    left = pane_selection_handler(main_qml, left=True)
    right = pane_selection_handler(main_qml, left=False)
    forward = next(line.split(":", 1)[1].strip()
                   for line in pane[pane.index("ColumnsView {"):].splitlines()
                   if "onSelectionTouched:" in line)
    names = ["paneTabs", "paneModel", "paneViewMode", "refreshPanes", "refreshPaneTabCount",
             "refreshActiveModel", "refreshTabLabels", "focusPane"]
    replacements = {
        "@CURRENT_CLICK@": app_block(columns[columns.index("// Current column."):], "onClicked:"),
        "@PARENT_CLICK@": app_block(columns[columns.index("id: pMouse"):], "onClicked:"),
        "@PEEK_CLICK@": app_block(columns[columns.index("id: kMouse"):], "onClicked:"),
        "@PANE_FORWARD@": forward,
        "@MAIN_LEFT_HANDLER@": left[left.index("{") + 1:-1],
        "@MAIN_RIGHT_HANDLER@": right[right.index("{") + 1:-1],
        "@FUNCTIONS@": "\n".join(app_block(main_qml, "function " + n + "(") for n in names),
    }
    focus = (source / "ui/tests/tst_pane_focus.qml.in").read_text()
    for key, value in replacements.items():
        focus = replace_once(focus, key, value)
    (tests / "tst_stage2_pane_focus.qml").write_text(focus)

    for d in ["home", "tmp", "runtime", "config", "data", "cache", "state"]:
        (out / d).mkdir(mode=0o700)
    env = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "HOME": str(out / "home"),
           "TMPDIR": str(out / "tmp"), "XDG_RUNTIME_DIR": str(out / "runtime"),
           "XDG_CONFIG_HOME": str(out / "config"), "XDG_DATA_HOME": str(out / "data"),
           "XDG_CACHE_HOME": str(out / "cache"), "XDG_STATE_HOME": str(out / "state"),
           "QT_QPA_PLATFORM": "offscreen", "QT_QUICK_BACKEND": "software",
           "QSG_RENDER_LOOP": "basic", "QT_FORCE_STDERR_LOGGING": "1",
           "QT_ASSUME_STDERR_HAS_CONSOLE": "1"}
    results = {}
    names = ["views", "main", "pane_focus", "targets", "qa_probe", "menu", "release"] if not args.only else [args.only]
    for name in names:
        qml = tests / f"tst_stage2_{name}.qml"
        argv = ["/usr/lib/qt6/bin/qmltestrunner", "-import", str(out / "imports"), "-input", str(qml)]
        proc = subprocess.run(argv, env=env, cwd=out, capture_output=True, timeout=60)
        log = (proc.stdout + proc.stderr).decode("utf-8", "backslashreplace")
        (out / f"{name}.log").write_text(log)
        results[name] = {"argv": argv, "exit": proc.returncode,
                         "totals": (re.findall(r"^Totals: .*$", log, re.M) or [""])[-1],
                         "warnings": len(re.findall(r"^(?:QWARN|WARNING)", log, re.M)),
                         "qml_sha256": digest(qml.read_bytes()), "log_sha256": digest(log.encode())}
        print(log, end="")
    after = {str(p.relative_to(source)): digest(p.read_bytes()) for p in tracked}
    receipt = {"source": str(source), "source_hashes": before, "source_unchanged": before == after,
               "runner_sha256": digest(Path(__file__).read_bytes()),
               "results": results,
               "scope": "actual Rows/Grid/Columns/Pane QML and exact Main.qml functions/bindings under "
                        "qmltestrunner offscreen software; fake model with real QAbstractItemModel resets; "
                        "not native DirModel, CXX transport, renderer or compositor"}
    (out / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps({k: (v["exit"], v["totals"], v["warnings"]) for k, v in results.items()}))
    ok = before == after and all(v["exit"] == 0 and v["warnings"] == 0 for v in results.values())
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
