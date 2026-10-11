#!/usr/bin/env python3
"""Qt MouseArea/signal regression; external DirModel I/O and visuals replaced."""
from pathlib import Path
import argparse
import json
import os
import re
import subprocess
import sys

from tab_model_regressions import block, digest, replace_once, require_vm


def pane_selection_handler(window, *, left):
    # Mask comments/strings without changing offsets. Only the selected Pane's
    # own properties are eligible: never borrow an ancestor, child or sibling.
    code = re.sub(
        r'//[^\n]*|/\*[\s\S]*?\*/|"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])*\'|`(?:\\.|[^`\\])*`',
        lambda match: "".join("\n" if c == "\n" else " " for c in match[0]),
        window,
    )
    candidates = []
    for match in re.finditer(r"\bPane\s*\{", code):
        start = match.start()
        bounded = block(code[start:], match[0])
        own = list(bounded)
        depth = 0
        for at, char in enumerate(bounded):
            if char == "{":
                depth += 1
            if depth != 1 or char in "{}":
                own[at] = " "
            if char == "}":
                depth -= 1
        properties = "".join(own)
        selected = (re.search(r"\bid\s*:\s*paneA\b", properties) is not None
                    if left else re.search(r"\bsourceComponent\s*:\s*$", code[:start]) is not None)
        if selected:
            candidates.append((start, bounded, properties))
    if len(candidates) != 1:
        raise ValueError("Expected exactly one paneA Pane" if left else "Expected exactly one sourceComponent Pane")
    start, bounded, properties = candidates[0]
    handlers = list(re.finditer(r"\bonSelectionTouched\s*:", properties))
    if len(handlers) != 1:
        raise ValueError("Selected Pane requires exactly one onSelectionTouched handler")
    handler_start = handlers[0].start()
    body_start = handlers[0].end()
    if not bounded[body_start:].lstrip().startswith("{"):
        raise ValueError("Selected Pane requires a braced onSelectionTouched handler")
    handler = block(bounded[handler_start:], handlers[0][0])
    return window[start + handler_start : start + handler_start + len(handler)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    require_vm()
    source = args.source.resolve()
    output = args.output.resolve()
    if output.exists():
        raise RuntimeError("Use a new evidence directory for every run")
    output.mkdir(parents=True, mode=0o700)
    relative = [
        "ui/ColumnsView.qml", "ui/Pane.qml", "ui/Main.qml",
        "ui/tests/tst_pane_focus.qml.in",
        "crates/app/tests/pane_focus_regressions.py",
        "crates/app/tests/tab_model_regressions.py",
    ]
    hashes = {name: digest((source / name).read_bytes()) for name in relative}
    columns = (source / "ui/ColumnsView.qml").read_text()
    pane = (source / "ui/Pane.qml").read_text()
    window = (source / "ui/Main.qml").read_text()
    current = block(columns[columns.index("// Current column."):], "onClicked:")
    parent = block(columns[columns.index("id: pMouse"):], "onClicked:")
    peek = block(columns[columns.index("id: kMouse"):], "onClicked:")
    forward_lines = pane[pane.index("ColumnsView {"):].splitlines()
    forward = next(line.split(":", 1)[1].strip() for line in forward_lines if "onSelectionTouched:" in line)
    left = pane_selection_handler(window, left=True)
    right = pane_selection_handler(window, left=False)
    names = ["paneTabs", "paneModel", "paneViewMode", "refreshPanes", "refreshPaneTabCount", "refreshActiveModel", "refreshTabLabels", "focusPane"]
    functions = "\n".join(block(window, "function " + name + "(") for name in names)
    replacements = {
        "@CURRENT_CLICK@": current, "@PARENT_CLICK@": parent, "@PEEK_CLICK@": peek,
        "@PANE_FORWARD@": forward, "@MAIN_LEFT_HANDLER@": left[left.index("{") + 1 : -1],
        "@MAIN_RIGHT_HANDLER@": right[right.index("{") + 1 : -1], "@FUNCTIONS@": functions,
    }
    fixture = (source / "ui/tests/tst_pane_focus.qml.in").read_text()
    for key, value in replacements.items():
        fixture = replace_once(fixture, key, value)
    qml = output / "tst_pane_focus.qml"
    qml.write_text(fixture)
    (output / "ColumnsView.qml").write_text(columns)
    for directory in ["home", "runtime", "tmp"]:
        (output / directory).mkdir(mode=0o700)
    env = {
        "PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "HOME": str(output / "home"),
        "TMPDIR": str(output / "tmp"), "XDG_RUNTIME_DIR": str(output / "runtime"),
        "QT_QPA_PLATFORM": "offscreen", "QT_QUICK_BACKEND": "software", "QSG_RENDER_LOOP": "basic",
        "QT_FORCE_STDERR_LOGGING": "1", "QT_ASSUME_STDERR_HAS_CONSOLE": "1",
    }
    argv = ["/usr/lib/qt6/bin/qmltestrunner", "-input", str(qml)]
    process = subprocess.run(argv, env=env, capture_output=True, text=True, timeout=45)
    log = process.stdout + process.stderr
    (output / "run.log").write_text(log)
    after = {name: digest((source / name).read_bytes()) for name in relative}
    unchanged = hashes == after
    receipt = {
        "argv": argv, "exit": process.returncode, "uid": os.getuid(),
        "boot": Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
        "source_hashes": hashes, "source_unchanged": unchanged,
        "source_hashes_after": after,
        "qml_sha256": digest(qml.read_bytes()), "log_sha256": digest(log.encode()),
        "scope": "actual Qt mouse clicks, exact Columns handlers and Pane/Main signal routing; external model I/O, whole renderer and CXX transport replaced",
    }
    (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(log, end="")
    print(json.dumps(receipt, indent=2))
    if not unchanged:
        raise RuntimeError("QA source changed during runner execution")
    return process.returncode


if __name__ == "__main__":
    sys.exit(main())
