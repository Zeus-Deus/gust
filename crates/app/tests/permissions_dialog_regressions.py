#!/usr/bin/env python3
"""Run the actual PermissionsDialog in Qt against a recording stand-in model.

Proves the card's own logic: real bits shown, mixed bits kept, only changed
bits asked for, Cancel/Escape/unchanged never apply, refusals stay visible,
keyboard stays inside, and nothing is lost or out of reach in the smallest
window at the largest zoom and density. The worker, set_modes and undo are
proven by the Rust tests in gust-ops attrs and gust-app permissions_json_tests. Theme tokens and
FileIcon are stubs; ScrollIndicator is the real one.
"""
from pathlib import Path
import argparse
import hashlib
import json
import shutil
import subprocess
import sys


TOKENS = """pragma Singleton
import QtQuick
QtObject {
    readonly property color background: "#101010"
    readonly property color backgroundDark: "#0a0a0a"
    readonly property color backgroundLighter: "#202020"
    readonly property color menuBackground: "#111111"
    readonly property color foreground: "#eeeeee"
    readonly property color foregroundDim: "#999999"
    readonly property color foregroundBright: "#ffffff"
    readonly property color border: "#666666"
    readonly property color accent: "#ccaa55"
    readonly property color warn: "#ddaa33"
    readonly property color danger: "#ff6666"
    readonly property color selectedBackground: "#444444"
    readonly property real selectedAlpha: 0.2
    readonly property real borderAlpha: 1
    readonly property int borderWidth: 1
    readonly property int radius: 0
    // Writable, and derived as in ui/Tokens.qml, so a test can zoom and
    // change density the way Ctrl+= and Settings do.
    property real fontBase: 12
    property real spacingScale: 1
    readonly property real fontCaption: Math.round(fontBase * 0.84)
    readonly property real fontSmall: Math.round(fontBase * 0.92)
    readonly property real fontBody: fontBase
    readonly property real fontSubtitle: Math.round(fontBase * 1.083)
    readonly property string fontFamily: "monospace"
    readonly property real spaceXs: Math.round(3 * spacingScale)
    readonly property real spaceSm: Math.round(4 * spacingScale)
    readonly property real spaceMd: Math.round(6 * spacingScale)
    readonly property real spaceLg: Math.round(8 * spacingScale)
    readonly property real spaceXl: Math.round(10 * spacingScale)
    readonly property real spaceXxl: Math.round(14 * spacingScale)
    readonly property real rowHeight: Math.round(26 * spacingScale * (fontBase / 12))
    readonly property real controlHeight: Math.round(28 * spacingScale * (fontBase / 12))
    function iconLook(k) { return k }
    function withAlpha(c, a) { return Qt.rgba(c.r, c.g, c.b, a) }
}
"""

FILE_ICON = """import QtQuick
Item { property string look: ""; property real size: 32; width: size; height: size }
"""


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
    names = ["ui/PermissionsDialog.qml", "ui/ScrollIndicator.qml",
             "ui/tests/tst_permissions_dialog.qml",
             "crates/app/tests/permissions_dialog_regressions.py"]
    hashes = {n: hashlib.sha256((source / n).read_bytes()).hexdigest() for n in names}
    for name in names[:3]:
        shutil.copyfile(source / name, output / Path(name).name)
    (output / "FileIcon.qml").write_text(FILE_ICON)
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
            "-input", str(output / "tst_permissions_dialog.qml")]
    p = subprocess.run(argv, env=env, capture_output=True, text=True, timeout=60)
    log = p.stdout + p.stderr
    (output / "run.log").write_text(log)
    unchanged = all(hashlib.sha256((source / n).read_bytes()).hexdigest() == h
                    for n, h in hashes.items())
    receipt = {"argv": argv, "exit": p.returncode, "source_hashes": hashes,
               "source_unchanged": unchanged,
               "scope": "actual PermissionsDialog logic and keyboard; model, theme and FileIcon synthetic"}
    (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(log, end="")
    return p.returncode if unchanged else 1


if __name__ == "__main__":
    sys.exit(main())
