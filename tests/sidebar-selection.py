#!/usr/bin/env python3
"""Exercise the actual sidebar with Qt; only model I/O and theme are fixtures."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    source = Path(__file__).resolve().parent.parent
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    module = output / "com/gust/app"
    module.mkdir(parents=True)
    (module / "qmldir").write_text("module com.gust.app\nsingleton Tokens 1.0 Tokens.qml\n")
    (module / "Tokens.qml").write_text('''pragma Singleton
import QtQuick
QtObject {
    readonly property color backgroundDark: "#151515"
    readonly property color foreground: "#eeeeee"
    readonly property color foregroundDim: "#aaaaaa"
    readonly property color accent: "#ffaa00"
    readonly property color border: "#666666"
    readonly property color danger: "#ff6666"
    readonly property real borderAlpha: 0.5
    readonly property int borderWidth: 1
    readonly property int rowHeight: 26
    readonly property int rowPaddingX: 12
    readonly property int spaceSm: 4
    readonly property int spaceLg: 8
    readonly property int spaceXl: 10
    readonly property int fontCaption: 10
    readonly property int fontSmall: 11
    readonly property int fontBody: 12
    readonly property int radius: 0
    readonly property int animFast: 120
    readonly property string fontFamily: "monospace"
    function withAlpha(c, a) { return Qt.rgba(c.r, c.g, c.b, a) }
}
''')
    for name in ["Sidebar.qml", "DragTarget.qml", "WheelScroll.qml"]:
        shutil.copyfile(source / "ui" / name, output / name)
    shutil.copyfile(source / "ui/tests/tst_sidebar.qml", output / "tst_sidebar.qml")
    (output / "home").mkdir()
    (output / "runtime").mkdir(mode=0o700)
    env = dict(os.environ, HOME=str(output / "home"),
               XDG_RUNTIME_DIR=str(output / "runtime"),
               QT_QPA_PLATFORM="offscreen", QT_QUICK_BACKEND="software")
    result = subprocess.run(["/usr/lib/qt6/bin/qmltestrunner", "-import", str(output),
                             "-input", str(output)], env=env, capture_output=True, text=True, timeout=60)
    log = result.stdout + result.stderr
    (output / "run.log").write_text(log)
    print(log, end="")
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
