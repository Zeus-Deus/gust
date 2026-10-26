#!/usr/bin/env python3
"""Exercise shipped wheel handling with Qt Quick Test and PySide6 events."""
import math
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
        property bool wheelAnimating: wheel.scrollAnimation.running
        WheelScroll { id: wheel; view: view }
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
         [(-24, -2)] * 10, 50),
        ("gesture reported as mouse", mouse, Qt.ScrollUpdate,
         [(-24, -2)] * 10, 50),
        ("pixel-only device", mouse, Qt.NoScrollPhase,
         [(0, -2)] * 10, 50),
    ]

    def send(device, phase, angle=0, pixels=0, *, x_angle=0, x_pixels=0,
             inverted=False):
        event = QWheelEvent(QPointF(200, 200), QPointF(200, 200),
                            QPoint(x_pixels, pixels), QPoint(x_angle, angle),
                            Qt.NoButton, Qt.NoModifier, phase, inverted,
                            Qt.MouseEventSynthesizedBySystem if phase != Qt.NoScrollPhase
                            else Qt.MouseEventNotSynthesized, device)
        QCoreApplication.sendEvent(window, event)
        app.processEvents()
        return event

    def expect_travel(name, start, expected):
        travel = view.property("contentY") - start
        assert abs(travel - expected) < 0.01, (name, travel, expected)
        print(f"PASS {name}: {travel:g}px", flush=True)

    for name, device, phase, deltas, expected in cases:
        view.setProperty("contentY", 1000.0)
        for angle, pixels in deltas:
            send(device, phase, angle, pixels)
        QTest.qWait(150)
        expect_travel(name, 1000, expected)

    # Wayland already applies the compositor's scroll factor. Preserve that
    # proportionality and fractional content positions with GTK's conversion.
    for pixels in (1, 2, 4):
        view.setProperty("contentY", 1000.0)
        send(pad, Qt.ScrollBegin)
        send(pad, Qt.ScrollUpdate, -12 * pixels, -pixels)
        expect_travel(f"scaled gesture {pixels}", 1000, pixels * 2.5)
        assert not view.property("wheelAnimating")
        send(pad, Qt.ScrollEnd)
        QTest.qWait(150)
        expect_travel(f"gesture ends without added coasting {pixels}", 1000, pixels * 2.5)

    # Reproduce Qt Wayland's per-frame rounding with carried pixel error.
    # Small deltas still have angles on frames where pixels round to zero.
    for delta in (0.2, 1.2, 4.0):
        view.setProperty("contentY", 1000.0)
        send(pad, Qt.ScrollBegin)
        remainder = 0.0
        total_pixels = 0
        for _ in range(60):
            pixels = math.floor(delta + remainder + 0.5)
            remainder += delta - pixels
            total_pixels += pixels
            event = send(pad, Qt.ScrollUpdate, -math.floor(delta * 12 + 0.5), -pixels)
            assert event.isAccepted()
            assert not view.property("wheelAnimating")
        send(pad, Qt.ScrollEnd)
        expect_travel(f"Wayland rounded stream {delta}", 1000, total_pixels * 2.5)

    view.setProperty("contentY", 1000.0)
    send(pad, Qt.ScrollBegin)
    send(pad, Qt.ScrollUpdate, -24, -2)
    # Subpixel frames can round to zero pixels while angleDelta stays nonzero.
    send(pad, Qt.ScrollUpdate, -5, 0)
    send(pad, Qt.ScrollUpdate, -5, 0, x_angle=-3)
    send(pad, Qt.ScrollMomentum, -12, -1)
    send(pad, Qt.ScrollMomentum, -5, 0)
    send(pad, Qt.ScrollEnd)
    QTest.qWait(150)
    expect_travel("zero-pixel frames and momentum stay on the gesture scale", 1000, 7.5)
    assert not view.property("wheelAnimating")

    send(pad, Qt.ScrollUpdate, 12, 1, inverted=True)
    expect_travel("reversal preserves the delivered natural-scroll direction", 1000, 5)

    for phase in (Qt.ScrollBegin, Qt.ScrollEnd):
        view.setProperty("contentY", 1000.0)
        send(mouse, Qt.NoScrollPhase, -120)
        assert view.property("wheelAnimating")
        send(pad, phase)
        position = view.property("contentY")
        assert not view.property("wheelAnimating")
        QTest.qWait(150)
        expect_travel(f"{phase.name} cancels pending wheel travel", position, 0)

    view.setProperty("contentY", 1000.0)
    event = send(pad, Qt.ScrollUpdate, x_angle=-24, x_pixels=-2)
    assert not event.isAccepted(), "horizontal-only gesture was consumed"
    QTest.qWait(150)
    expect_travel("horizontal-only gesture leaves the vertical list alone", 1000, 0)

    bottom = view.property("contentHeight") - view.property("height")
    view.setProperty("contentY", bottom - 1)
    send(pad, Qt.ScrollUpdate, -24, -2)
    expect_travel("gesture clamps at the bottom", bottom, 0)
    send(pad, Qt.ScrollUpdate, 24, 2)
    expect_travel("gesture reverses immediately at the bottom", bottom, -5)
    view.setProperty("contentY", 1.0)
    send(pad, Qt.ScrollUpdate, 24, 2)
    expect_travel("gesture clamps at the top", 0, 0)
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
