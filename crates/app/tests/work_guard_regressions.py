#!/usr/bin/env python3
"""Run the exact Main.qml close, tab-close and disconnect guards in Qt,
the exact preference startup handoff, and actual InterruptedNotice, JobDrawer, SettingsPanel and HintStrip.

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
    readonly property color background: "#111111"
    readonly property color foregroundDim: "#aaaaaa"
    readonly property real selectedAlpha: 0.2
    readonly property real borderAlpha: 0.5
    readonly property int rowHeight: 26
    readonly property int spaceXs: 4
    readonly property int spaceXl: 24
    readonly property int fontCaption: 12
    readonly property int fontSubtitle: 18
    readonly property color menuBackground: "#111111"
    readonly property color foreground: "#eeeeee"
    readonly property color foregroundBright: "#ffffff"
    readonly property color border: "#666666"
    readonly property color accent: "#ccaa55"
    readonly property color warn: "#ddaa33"
    readonly property color selectedBackground: "#444444"
    readonly property color selectedText: "#000000"
    readonly property color backgroundDark: "#0a0a0a"
    readonly property color muted: "#888888"
    readonly property color danger: "#ff6666"
    readonly property color ok: "#66cc66"
    readonly property int borderWidth: 1
    readonly property int radius: 0
    readonly property int animFast: 0
    readonly property int spaceMd: 12
    readonly property int spaceXxl: 32
    readonly property real spacingScale: 1
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
             "ui/JobDrawer.qml", "ui/tests/tst_job_drawer.qml",
             "ui/SettingsPanel.qml", "ui/tests/tst_settings_panel.qml",
             "ui/tests/tst_preferences_startup.qml.in", "ui/tests/tst_preferences_notice.qml.in",
             "ui/HintStrip.qml",
             "crates/app/tests/work_guard_regressions.py",
             "crates/app/tests/tab_model_regressions.py"]
    hashes = {n: hashlib.sha256((source / n).read_bytes()).hexdigest() for n in names}

    text = (source / "ui/Main.qml").read_text()
    confirm = text[text.index("        id: confirm\n"):]
    functions = [block(text, "function " + n + "(") for n in
                 ["busyModels", "runningJobModels", "closeTab", "placeWorkBlocker", "refuseBusyDisconnect", "askRun",
                  "revisionAtLeast", "prefsClose", "sessionChanged", "saveSession", "prefsLaunchUntouched", "startWithPrefs", "restoreSession"]]
    timers = [block(text, "Timer {\n        id: " + n + "\n") for n in ["closeAfterCancel", "closeTabAfterCancel", "prefsCloseDeadline", "sessionSaveTimer"]]
    fixture = (source / "ui/tests/tst_work_guard.qml.in").read_text()
    fixture = replace_once(fixture, "@CLOSING_HANDLER@", body(text, "onClosing: (close) =>"))
    fixture = replace_once(fixture, "@CONFIRM_ACCEPTED@", body(confirm, "onAccepted:"))
    fixture = replace_once(fixture, "@CONFIRM_REJECTED@", body(confirm, "onRejected:"))
    fixture = replace_once(fixture, "@CONFIRM_CLOSED@", body(confirm, "onClosed:"))
    hooks = [line.strip() for line in text.splitlines()
             if line.strip().startswith("readonly property string prefsError:")]
    if len(hooks) != 1:
        raise ValueError("Expected the production prefsError binding")
    # The full preferences/startup surface is outside this close-guard seam.
    # Bind the exact error expression to the same synthetic owner snapshot.
    hooks.insert(0, "readonly property var prefs: JSON.parse(dirModel.settings_json())")
    fixture = replace_once(fixture, "@FUNCTIONS@", "\n".join(functions + hooks))
    fixture = replace_once(fixture, "@TIMERS@", "\n".join(timers))
    if "Qt.quit()" not in fixture:
        raise ValueError("Expected the production quit call in the extracted guards")
    fixture = fixture.replace("Qt.quit()", "win.quitCalls += 1")
    qml = output / "tst_work_guard.qml"
    qml.write_text(fixture)
    startup = (source / "ui/tests/tst_preferences_startup.qml.in").read_text()
    startup = replace_once(startup, "@START_WITH_PREFS@", block(text, "function startWithPrefs("))
    startup = replace_once(startup, "@PREFS_LAUNCH_UNTOUCHED@", block(text, "function prefsLaunchUntouched("))
    startup = replace_once(startup, "@CAPTURE_LAUNCH@", block(text, "win.prefsLaunch ="))
    (output / "tst_preferences_startup.qml").write_text(startup)
    notice = (source / "ui/tests/tst_preferences_notice.qml.in").read_text()
    bindings_start = text.index("    property int prefsTick: 0\n")
    bindings_end = text.index("    function prefsNotified()")
    bindings = block(text, "function revisionAtLeast(") + "\n" + text[bindings_start:bindings_end]
    settings = text[text.index("        id: settingsPanel\n"):]
    alerts = [line.strip() for line in text.splitlines() if line.strip().startswith("alert: win.alertText")]
    if len(alerts) != 1:
        raise ValueError("Expected the production hint alert binding")
    notice = replace_once(notice, "@PREFS_BINDINGS@", bindings)
    notice = replace_once(notice, "@HINT_ALERT@", alerts[0][len("alert: "):])
    notice = replace_once(notice, "@SETTINGS_CHANGED@", body(settings, "onChanged: (key, value) =>"))
    notice = replace_once(notice, "@SETTINGS_RETRY@", body(settings, "onRetryRequested:"))
    (output / "tst_preferences_notice.qml").write_text(notice)
    for name in ["ui/InterruptedNotice.qml", "ui/tests/tst_interrupted_notice.qml",
                 "ui/JobDrawer.qml", "ui/tests/tst_job_drawer.qml",
                 "ui/SettingsPanel.qml", "ui/tests/tst_settings_panel.qml", "ui/HintStrip.qml"]:
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
    argv = ["/usr/lib/qt6/bin/qmltestrunner", "-import", str(output), "-input", str(output), "-v1"]
    p = subprocess.run(argv, env=env, capture_output=True, text=True, timeout=60)
    log = p.stdout + p.stderr
    (output / "run.log").write_text(log)
    unchanged = all(hashlib.sha256((source / n).read_bytes()).hexdigest() == h
                    for n, h in hashes.items())
    receipt = {"argv": argv, "exit": p.returncode, "source_hashes": hashes,
               "source_unchanged": unchanged,
               "scope": "exact Main.qml close/tab-close/disconnect guards and preference startup/capture, actual InterruptedNotice/JobDrawer/SettingsPanel/HintStrip and exact preferences notice/settings handlers; "
                        "models and theme synthetic, Qt.quit counted"}
    (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(log, end="")
    return p.returncode if unchanged else 1


if __name__ == "__main__":
    sys.exit(main())
