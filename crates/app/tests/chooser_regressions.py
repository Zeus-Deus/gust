#!/usr/bin/env python3
"""Run the exact Main.qml Open/Save dialog flow in Qt against a stand-in model.

Inserts the window's real chooser state, functions, Ctrl+Return shortcut,
change-signal wiring and ChooserBar/ChooserFooter instances into a fixture, with
the actual ChooserBar, ChooserFooter, ConfirmDialog and HintStrip. Proves the
button's words, the replace confirmation, stale answers, double accepts and
which keys accept. The worker, the stat and the result file are proven by the
Rust tests in crates/core/tests/chooser.rs. Theme tokens are a stub, the model
is synthetic and `Qt.quit()` is counted.
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
    readonly property color background: "#101010"
    readonly property color backgroundDark: "#0a0a0a"
    readonly property color barBackground: "#0c0c0c"
    readonly property color menuBackground: "#111111"
    readonly property color foreground: "#eeeeee"
    readonly property color foregroundDim: "#999999"
    readonly property color foregroundBright: "#ffffff"
    readonly property color border: "#666666"
    readonly property color accent: "#ccaa55"
    readonly property color danger: "#ff6666"
    readonly property real borderAlpha: 1
    readonly property int borderWidth: 1
    readonly property int radius: 0
    readonly property real spacingScale: 1
    readonly property int fontCaption: 10
    readonly property int fontSmall: 11
    readonly property int fontBody: 12
    readonly property int fontSubtitle: 13
    readonly property string fontFamily: "monospace"
    readonly property int spaceXs: 3
    readonly property int spaceSm: 4
    readonly property int spaceMd: 6
    readonly property int spaceLg: 8
    readonly property int spaceXl: 10
    readonly property int spaceXxl: 14
    readonly property int rowPaddingX: 12
    readonly property int controlHeight: 28
    function withAlpha(c, a) { return Qt.rgba(c.r, c.g, c.b, a) }
}
"""

FUNCTIONS = ["refreshChooserTarget", "chooserAccept", "chooserChecked", "chooserForgetCheck",
             "chooserAskReplace", "chooserReplaceConfirmed", "chooserAnswer", "chooserCancel",
             "chooserActivate", "chooserReturn", "chooserFits", "chooserMenu"]


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
    copies = ["ui/ChooserBar.qml", "ui/ChooserFooter.qml", "ui/ConfirmDialog.qml", "ui/HintStrip.qml"]
    names = ["ui/Main.qml", "ui/tests/tst_chooser_bar.qml.in",
             "crates/app/tests/chooser_regressions.py",
             "crates/app/tests/tab_model_regressions.py"] + copies
    hashes = {n: hashlib.sha256((source / n).read_bytes()).hexdigest() for n in names}

    text = (source / "ui/Main.qml").read_text()
    state = text[text.index("    property string chooserMode:"):text.index("    // --- tabs ------")]
    hints = block(text, "readonly property var chooserHints:")
    refused = text[text.index("    readonly property var chooserRefused:"):]
    refused = refused[:refused.index("]\n") + 2]
    wiring = [block(text, "Shortcut {\n        sequences"),
              block(text, "Connections {\n        target: win.chooserMode")]
    bars = [block(text, "ChooserBar {\n            id: chooserBar"),
            block(text, "ChooserFooter {\n            id: chooserFooter")]
    confirm = text[text.index("        id: confirm\n"):]
    fixture = (source / "ui/tests/tst_chooser_bar.qml.in").read_text()
    fixture = replace_once(fixture, "@STATE@", state + hints + "\n" + refused)
    fixture = replace_once(fixture, "@FUNCTIONS@",
                           "\n".join(block(text, "function " + n + "(") for n in FUNCTIONS))
    fixture = replace_once(fixture, "@WIRING@", "\n".join(wiring))
    fixture = replace_once(fixture, "@BARS@", "\n".join(bars))
    fixture = replace_once(fixture, "@CONFIRM_CLOSED@", block(confirm, "onClosed:"))
    accepted = block(confirm, "onAccepted:")
    rejected = block(confirm, "onRejected:")
    guard = 'if (confirm.purpose === "chooser-replace") {'
    fixture = replace_once(fixture, "@CONFIRM_ACCEPTED@", block(accepted, guard))
    fixture = replace_once(fixture, "@CONFIRM_REJECTED@", block(rejected, guard))
    if "Qt.quit()" not in fixture:
        raise ValueError("Expected the production quit call in the extracted flow")
    fixture = fixture.replace("Qt.quit()", "win.quitCalls += 1")
    (output / "tst_chooser_bar.qml").write_text(fixture)
    for name in copies:
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
    argv = ["/usr/lib/qt6/bin/qmltestrunner", "-import", str(output),
            "-input", str(output / "tst_chooser_bar.qml")]
    p = subprocess.run(argv, env=env, capture_output=True, text=True, timeout=120)
    log = p.stdout + p.stderr
    (output / "run.log").write_text(log)
    unchanged = all(hashlib.sha256((source / n).read_bytes()).hexdigest() == h
                    for n, h in hashes.items())
    receipt = {"argv": argv, "exit": p.returncode, "source_hashes": hashes,
               "source_unchanged": unchanged,
               "scope": "exact Main.qml dialog flow and actual ChooserBar/ChooserFooter/ConfirmDialog/"
                        "HintStrip; model and theme synthetic, Qt.quit counted"}
    (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(log, end="")
    return p.returncode if unchanged else 1


if __name__ == "__main__":
    sys.exit(main())
