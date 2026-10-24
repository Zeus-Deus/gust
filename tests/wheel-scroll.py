#!/usr/bin/env python3
"""Exercise shipped wheel handling with Qt Quick Test and PySide6 events."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


def pixel_events(root):
    from PySide6.QtCore import QCoreApplication, QObject, QPoint, QPointF, Qt
    from PySide6.QtGui import QGuiApplication, QInputDevice, QPointingDevice, QWheelEvent
    from PySide6.QtQml import QQmlApplicationEngine
    from PySide6.QtTest import QTest

    app = QGuiApplication([])
    engine = QQmlApplicationEngine()
    warnings = []
    engine.warnings.connect(lambda items: warnings.extend(str(item) for item in items))
    scene = root / "WheelInput.qml"
    scene.write_text("""import QtQuick
Window {
    visible: true
    width: 800; height: 700
    Flickable {
        objectName: "view"
        id: view
        anchors.fill: parent
        contentHeight: 100000
        boundsBehavior: Flickable.StopAtBounds
        WheelScroll { view: view }
    }
}
""")
    engine.load(str(scene))
    assert engine.rootObjects(), "wheel input scene did not load"
    window = engine.rootObjects()[0]
    view = window.findChild(QObject, "view")
    QTest.qWait(20)
    mouse = QPointingDevice("wheel", 1, QInputDevice.DeviceType.Mouse,
                            QPointingDevice.PointerType.Generic,
                            QInputDevice.Capability.Position, 1, 3)
    pad = QPointingDevice("pad", 2, QInputDevice.DeviceType.TouchPad,
                          QPointingDevice.PointerType.Finger,
                          QInputDevice.Capability.Position, 5, 0)
    # Wayland's measured mouse stream has a pixel delta only on every eighth
    # fractional event. Both it and the classic stream represent five notches.
    cases = [
        ("mouse pixels and angles", mouse, Qt.NoScrollPhase,
         [(-120, -15)] * 5, 390),
        ("mixed high-resolution mouse", mouse, Qt.NoScrollPhase,
         [(-15, -15 if i % 8 == 0 else 0) for i in range(40)], 390),
        ("Wayland wheel reported as touchpad", pad, Qt.NoScrollPhase,
         [(-120, -15)] * 5, 390),
        ("Wayland high-resolution wheel reported as touchpad", pad, Qt.NoScrollPhase,
         [(-15, -15 if i % 8 == 0 else 0) for i in range(40)], 390),
        ("touchpad pixels", pad, Qt.ScrollUpdate,
         [(-120, -2)] * 10, 20),
        ("gesture reported as mouse", mouse, Qt.ScrollUpdate,
         [(-120, -2)] * 10, 20),
        ("pixel-only device", mouse, Qt.NoScrollPhase,
         [(0, -2)] * 10, 20),
    ]
    for name, device, phase, deltas, expected in cases:
        view.setProperty("contentY", 1000.0)
        for angle, pixels in deltas:
            event = QWheelEvent(QPointF(200, 200), QPointF(200, 200),
                                QPoint(0, pixels), QPoint(0, angle),
                                Qt.NoButton, Qt.NoModifier, phase, False,
                                Qt.MouseEventNotSynthesized, device)
            QCoreApplication.sendEvent(window, event)
            app.processEvents()
        QTest.qWait(150)
        travel = view.property("contentY") - 1000
        assert abs(travel - expected) < 0.01, (name, travel, expected)
        print(f"PASS {name}: {travel:g}px", flush=True)
    assert not warnings, warnings
    window.close()


def main():
    repo = Path(__file__).resolve().parent.parent
    runner = shutil.which("qmltestrunner6")
    if runner is None:
        candidate = Path("/usr/lib/qt6/bin/qmltestrunner")
        if candidate.is_file():
            runner = str(candidate)
    if runner is None:
        raise SystemExit("Qt6 qmltestrunner is required")
    with tempfile.TemporaryDirectory(prefix="gust-wheel-scroll-") as temp:
        root = Path(temp)
        shutil.copy2(repo / "ui/WheelScroll.qml", root)
        shutil.copy2(repo / "tests/qml/tst_wheel_scroll.qml", root)
        # Only theme metrics are fixtures; input delivery and the handler are real.
        (root / "qmldir").write_text("singleton Tokens 1.0 Tokens.qml\n")
        (root / "Tokens.qml").write_text(
            "pragma Singleton\nimport QtQuick\nQtObject {\n"
            "property real rowHeight: 26\nproperty int animFast: 120\n}\n"
        )
        env = os.environ.copy()
        env.update(QT_QPA_PLATFORM="offscreen", QT_QUICK_BACKEND="software",
                   QT_FORCE_STDERR_LOGGING="1", HOME=str(root / "home"),
                   XDG_CONFIG_HOME=str(root / "config"),
                   XDG_CACHE_HOME=str(root / "cache"),
                   XDG_DATA_HOME=str(root / "data"),
                   XDG_STATE_HOME=str(root / "state"))
        subprocess.run([runner, "-input", str(root)], env=env, check=True)
        subprocess.run([sys.executable, __file__, "--pixel-events", str(root)],
                       env=env, check=True)


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--pixel-events":
        pixel_events(Path(sys.argv[2]))
    else:
        main()
