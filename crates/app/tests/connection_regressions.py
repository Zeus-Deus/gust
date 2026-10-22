#!/usr/bin/env python3
"""Exercise the actual ConnectPanel with Qt; only theme/model I/O are fixtures."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

from tab_model_regressions import block


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    source = args.source.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    module = output / "com/gust/app"
    module.mkdir(parents=True)
    (module / "qmldir").write_text("module com.gust.app\nsingleton Tokens 1.0 Tokens.qml\n")
    (module / "Tokens.qml").write_text('''pragma Singleton
import QtQuick
QtObject {
    readonly property color background: "#151515"
    readonly property color backgroundDark: "#111111"
    readonly property color menuBackground: "#111111"
    readonly property color foreground: "#eeeeee"
    readonly property color foregroundDim: "#aaaaaa"
    readonly property color foregroundBright: "#ffffff"
    readonly property color border: "#666666"
    readonly property color accent: "#ccaa55"
    readonly property color danger: "#ff6666"
    readonly property color selectedBackground: "#444444"
    readonly property real selectedAlpha: 0.4
    readonly property real borderAlpha: 0.5
    readonly property int borderWidth: 1
    readonly property int rowHeight: 26
    readonly property int rowPaddingX: 12
    readonly property int controlHeight: 32
    readonly property int spaceXs: 4
    readonly property int spaceSm: 8
    readonly property int spaceLg: 16
    readonly property int spaceXl: 24
    readonly property int spaceXxl: 32
    readonly property int spaceMd: 12
    readonly property int radius: 0
    readonly property real spacingScale: 1
    readonly property int fontCaption: 12
    readonly property int fontSmall: 13
    readonly property int fontBody: 15
    readonly property int fontSubtitle: 20
    readonly property string fontFamily: "monospace"
    function withAlpha(c, a) { return Qt.rgba(c.r, c.g, c.b, a) }
}
''')
    names = ["ui/ConnectPanel.qml", "ui/WheelScroll.qml", "ui/ConfirmDialog.qml", "ui/Main.qml",
             "ui/tests/tst_connections.qml", "ui/tests/tst_connection_controller.qml.in",
             "crates/app/tests/connection_regressions.py", "crates/app/tests/tab_model_regressions.py"]
    hashes = {}
    for name in names:
        data = (source / name).read_bytes()
        hashes[name] = hashlib.sha256(data).hexdigest()
        if name.endswith(".qml") and name != "ui/Main.qml":
            shutil.copyfile(source / name, output / Path(name).name)
    controller = (source / "ui/tests/tst_connection_controller.qml.in").read_text()
    main_source = (source / "ui/Main.qml").read_text()
    controller = controller.replace("@PLACES_ARRIVED@", block(main_source, "function placesArrived("))
    functions = ["connectPlace", "tailnetUserEntered", "placeDone", "beginPlaceFeedback", "finishPlaceFeedback", "startTailnetAttempt", "shareAddressEntered", "sharePasswordEntered", "connectShare", "forgetConnectionOwner", "installSupport", "openPhone", "requireSupport"]
    controller = controller.replace("@CONNECTION_FUNCTIONS@", "\n".join(
        block(main_source, "function " + name + "(") for name in functions if "function " + name + "(" in main_source))
    controller = controller.replace("@CONNECT_HANDLER@", block(main_source, "onConnectRequested:"))
    controller = controller.replace("@ACTION_HANDLER@", block(main_source, "onActionRequested: (a) => { connectPanel" ) if "onActionRequested: (a) => { connectPanel" in main_source
                                    else block(main_source[main_source.index("    ConnectPanel {"):], "onActionRequested:"))
    controller = controller.replace("@CLOUD_BRANCH@", block(main_source, 'else if (a === "cloud-setup")'))
    removed = block(main_source, "onObjectRemoved:")
    controller = controller.replace("@OBJECT_REMOVED@", removed[removed.index("{") + 1:-1])
    (output / "tst_connection_controller.qml").write_text(controller)
    for name in ["home", "runtime", "tmp"]:
        (output / name).mkdir(mode=0o700)
    env = {"PATH": "/usr/bin:/bin", "HOME": str(output / "home"),
           "XDG_RUNTIME_DIR": str(output / "runtime"), "TMPDIR": str(output / "tmp"),
           "LANG": "C.UTF-8", "QT_QPA_PLATFORM": "offscreen", "QT_QUICK_BACKEND": "software",
           "QT_FORCE_STDERR_LOGGING": "1", "QT_ASSUME_STDERR_HAS_CONSOLE": "1"}
    argv = ["/usr/lib/qt6/bin/qmltestrunner", "-import", str(output), "-input", str(output)]
    p = subprocess.run(argv, env=env, capture_output=True, text=True, timeout=60)
    log = p.stdout + p.stderr
    (output / "run.log").write_text(log)
    receipt = {"argv": argv, "exit": p.returncode, "source_hashes": hashes,
               "source_unchanged": all(hashlib.sha256((source / n).read_bytes()).hexdigest() == h for n, h in hashes.items()),
               "scope": "actual ConnectPanel object, Qt keyboard and signals; theme and external model I/O substituted"}
    (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(log, end="")
    return p.returncode if receipt["source_unchanged"] else 1


if __name__ == "__main__":
    sys.exit(main())
