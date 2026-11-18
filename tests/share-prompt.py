#!/usr/bin/env python3
"""Send real Qt key events through the production share prompt."""
import os
from pathlib import Path
import shutil
import tempfile

os.environ.update(QT_QPA_PLATFORM="offscreen", QT_QUICK_BACKEND="software")

from PySide6.QtCore import QEvent, QPointF, Qt, QUrl
from PySide6.QtGui import QGuiApplication, QKeyEvent
from PySide6.QtQuick import QQuickItem, QQuickView
from PySide6.QtTest import QTest


def main():
    app = QGuiApplication([])
    ui = Path(__file__).resolve().parents[1] / "ui"
    with tempfile.TemporaryDirectory(prefix="gust-share-prompt-") as temp:
        root = Path(temp)
        module = root / "com/gust/app"
        module.mkdir(parents=True)
        for name in ("TextPrompt.qml", "Tokens.qml"):
            shutil.copyfile(ui / name, module / name)
        (module / "qmldir").write_text(
            "module com.gust.app\n"
            "singleton Tokens 1.0 Tokens.qml\n"
            "ThemeTokens 1.0 ThemeTokens.qml\n"
            "TextPrompt 1.0 TextPrompt.qml\n"
        )
        (module / "ThemeTokens.qml").write_text("""import QtQuick
QtObject {
    property int revision: 0
    property int icons_rev: 0
    property string name: "fixture"
    property bool is_light: false
    function color(key) { return "#808080" }
    function metric(key) { return ({font_base: 12, spacing_scale: 1})[key] || 0 }
    function watch_theme() {}
}""")
        fixture = root / "Share.qml"
        fixture.write_text("""import QtQuick
import com.gust.app
Item {
    width: 640
    height: 400
    property int submissions: 0
    property string submitted: ""
    function openAddress() {
        submissions = 0
        prompt.mode = "share-address"
        prompt.openWith("Address", "smb://nas/public")
    }
    TextPrompt {
        id: prompt
        anchors.fill: parent
        property string mode: ""
        allowEmpty: mode === "share-user"
        onAccepted: (text) => {
            submissions += 1
            submitted = text
            if (mode === "share-address") {
                mode = "share-user"
                openWith("Username", "")
            }
        }
    }
}""")
        view = QQuickView()
        view.engine().addImportPath(temp)
        view.setSource(QUrl.fromLocalFile(str(fixture)))
        assert view.status() == QQuickView.Ready, view.errors()
        view.show()
        assert QTest.qWaitForWindowActive(view)
        flow = view.rootObject()

        def key_event(key, event_type=QEvent.KeyPress, repeat=False):
            event = QKeyEvent(event_type, key, Qt.NoModifier, "\r", repeat)
            app.sendEvent(view, event)

        def enter_username_step(key):
            flow.openAddress()
            QTest.qWait(150)
            key_event(key)
            assert flow.property("submissions") == 1
            assert flow.property("submitted") == "smb://nas/public"
            key_event(key, repeat=True)
            key_event(key, repeat=True)
            assert flow.property("submissions") == 1, f"{key} repeat selected Guest"
            key_event(key, QEvent.KeyRelease)

        def check_guest():
            assert flow.property("submissions") == 2
            assert flow.property("submitted") == ""

        for key in (Qt.Key_Return, Qt.Key_Enter):
            enter_username_step(key)
            key_event(key)
            key_event(key, QEvent.KeyRelease)
            check_guest()

        enter_username_step(Qt.Key_Return)
        ok = next(item for item in flow.findChildren(QQuickItem) if item.property("text") == "OK")
        center = ok.mapToScene(QPointF(ok.width() / 2, ok.height() / 2))
        QTest.mouseClick(view, Qt.LeftButton, Qt.NoModifier, center.toPoint())
        check_guest()
        view.close()
    print("PASS: held Return and keypad Enter stay on Username; fresh keys and OK choose Guest")


if __name__ == "__main__":
    main()
