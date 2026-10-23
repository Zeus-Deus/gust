#!/usr/bin/env python3
"""Run the exact Main.qml close, tab-close and disconnect guards in Qt,
and the actual InterruptedNotice shown after an interrupted run.

Models are synthetic: their work is plain properties, so this proves the
window logic, not DirModel's job handle or a real mount. `Qt.quit()` is the
one substituted call (it would end the test runner); it is counted instead.
Theme tokens are a stub module.
"""
from pathlib import Path
import argparse
import hashlib
import json
import shutil
import subprocess
import sys

from tab_model_regressions import block, replace_once


TOKENS = """pragma Singleton
import QtQuick
QtObject {
    readonly property color menuBackground: "#111111"
    readonly property color foreground: "#eeeeee"
    readonly property color foregroundBright: "#ffffff"
    readonly property color border: "#666666"
    readonly property color accent: "#ccaa55"
    readonly property color warn: "#ddaa33"
    readonly property color selectedBackground: "#444444"
    readonly property int borderWidth: 1
    readonly property int rowPaddingX: 12
    readonly property int controlHeight: 32
    readonly property int spaceSm: 8
    readonly property int spaceLg: 16
    readonly property int fontSmall: 13
    readonly property int fontBody: 15
    readonly property string fontFamily: "monospace"
    function withAlpha(c, a) { return Qt.rgba(c.r, c.g, c.b, a) }
}
"""


def body(text, marker):
    found = block(text, marker)
    return found[found.index("{") + 1 : -1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    source = args.source.resolve()
    output = args.output.resolve()
    if output.exists():
        raise RuntimeError("Use a new evidence directory for every run")
    output.mkdir(parents=True, mode=0o700)
    names = ["ui/Main.qml", "ui/tests/tst_work_guard.qml.in",
             "ui/InterruptedNotice.qml", "ui/tests/tst_interrupted_notice.qml",
             "crates/app/tests/work_guard_regressions.py",
             "crates/app/tests/tab_model_regressions.py"]
    hashes = {n: hashlib.sha256((source / n).read_bytes()).hexdigest() for n in names}

    text = (source / "ui/Main.qml").read_text()
    confirm = text[text.index("        id: confirm\n"):]
    functions = [block(text, "function " + n + "(") for n in
                 ["busyModels", "runningJobModels", "closeTab", "placeWorkBlocker", "refuseBusyDisconnect"]]
    timers = [block(text, "Timer {\n        id: " + n + "\n") for n in ["closeAfterCancel", "closeTabAfterCancel"]]
    fixture = (source / "ui/tests/tst_work_guard.qml.in").read_text()
    fixture = replace_once(fixture, "@CLOSING_HANDLER@", body(text, "onClosing: (close) =>"))
    fixture = replace_once(fixture, "@CONFIRM_ACCEPTED@", body(confirm, "onAccepted:"))
    fixture = replace_once(fixture, "@CONFIRM_REJECTED@", body(confirm, "onRejected:"))
    fixture = replace_once(fixture, "@FUNCTIONS@", "\n".join(functions))
    fixture = replace_once(fixture, "@TIMERS@", "\n".join(timers))
    if "Qt.quit()" not in fixture:
        raise ValueError("Expected the production quit call in the extracted guards")
    fixture = fixture.replace("Qt.quit()", "win.quitCalls += 1")
    qml = output / "tst_work_guard.qml"
    qml.write_text(fixture)
    for name in ["ui/InterruptedNotice.qml", "ui/tests/tst_interrupted_notice.qml"]:
        shutil.copyfile(source / name, output / Path(name).name)
    module = output / "com/gust/app"
    module.mkdir(parents=True)
    (module / "qmldir").write_text("module com.gust.app\nsingleton Tokens 1.0 Tokens.qml\n")
    (module / "Tokens.qml").write_text(TOKENS)

    for directory in ["home", "runtime", "tmp"]:
        (output / directory).mkdir(mode=0o700)
    env = {
        "PATH": "/usr/bin:/bin", "LANG": "C.UTF-8",
        "HOME": str(output / "home"), "TMPDIR": str(output / "tmp"),
        "XDG_RUNTIME_DIR": str(output / "runtime"),
        "QT_QPA_PLATFORM": "offscreen", "QT_QUICK_BACKEND": "software",
        "QT_FORCE_STDERR_LOGGING": "1", "QT_ASSUME_STDERR_HAS_CONSOLE": "1",
    }
    argv = ["/usr/lib/qt6/bin/qmltestrunner", "-import", str(output), "-input", str(output)]
    p = subprocess.run(argv, env=env, capture_output=True, text=True, timeout=60)
    log = p.stdout + p.stderr
    (output / "run.log").write_text(log)
    unchanged = all(hashlib.sha256((source / n).read_bytes()).hexdigest() == h
                    for n, h in hashes.items())
    receipt = {"argv": argv, "exit": p.returncode, "source_hashes": hashes,
               "source_unchanged": unchanged,
               "scope": "exact Main.qml close/tab-close/disconnect guards and actual InterruptedNotice; "
                        "models and theme synthetic, Qt.quit counted"}
    (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(log, end="")
    return p.returncode if unchanged else 1


if __name__ == "__main__":
    sys.exit(main())
