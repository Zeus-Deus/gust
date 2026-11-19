#!/usr/bin/env python3
"""Exercise the shipped path bar with a path longer than the bar.

Requires Qt6 Quick Test. Runs offscreen with its own deterministic theme
tokens; the application and the user's settings are untouched.
"""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

TOKENS = '''pragma Singleton
import QtQuick
QtObject {
    readonly property color accent: "#ffaa00"
    readonly property color barBackground: "#202020"
    readonly property color foreground: "#eeeeee"
    readonly property color foregroundBright: "#ffffff"
    readonly property color foregroundDim: "#aaaaaa"
    readonly property int borderWidth: 1
    readonly property int radius: 0
    readonly property int controlHeight: 28
    readonly property int spaceSm: 4
    readonly property int spaceLg: 8
    readonly property int fontBody: 12
    readonly property string fontFamily: "monospace"
    function withAlpha(c, a) { return Qt.rgba(c.r, c.g, c.b, a) }
}
'''


def main():
    repo = Path(__file__).resolve().parent.parent
    runner = shutil.which("qmltestrunner6")
    if runner is None:
        candidate = Path("/usr/lib/qt6/bin/qmltestrunner")
        if candidate.is_file():
            runner = str(candidate)
    if runner is None:
        raise SystemExit("Qt6 qmltestrunner is required")
    with tempfile.TemporaryDirectory(prefix="gust-breadcrumbs-") as temp:
        root = Path(temp)
        module = root / "imports/com/gust/app"
        module.mkdir(parents=True)
        (module / "qmldir").write_text("module com.gust.app\nsingleton Tokens 1.0 Tokens.qml\n")
        (module / "Tokens.qml").write_text(TOKENS)
        tests = root / "tests"
        tests.mkdir()
        test = (repo / "tests/qml/tst_breadcrumbs.qml").read_text()
        (tests / "tst_breadcrumbs.qml").write_text(test.replace("@UI_URL@", (repo / "ui").as_uri()))
        env = os.environ.copy()
        env.update(QT_QPA_PLATFORM="offscreen", QT_QUICK_BACKEND="software",
                   QT_FORCE_STDERR_LOGGING="1", HOME=str(root / "home"),
                   XDG_CONFIG_HOME=str(root / "config"),
                   XDG_CACHE_HOME=str(root / "cache"),
                   XDG_DATA_HOME=str(root / "data"),
                   XDG_STATE_HOME=str(root / "state"))
        result = subprocess.run(
            [runner, "-import", str(root / "imports"), "-input", str(tests)],
            env=env, check=False)
        raise SystemExit(result.returncode)


if __name__ == "__main__":
    main()
