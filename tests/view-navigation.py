#!/usr/bin/env python3
"""Measure cursor dependencies in the shipped views with a recording Qt model.

Only the model and ThemeTokens service are fixtures. Views, delegates and their
visual children load from the selected checkout. No filesystem/desktop services
are contacted. Native acceptance is a separate journey.
"""

import argparse
import json
import os
import re
import tempfile
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--source", type=Path, default=Path(__file__).resolve().parent.parent
    )
    parser.add_argument(
        "--observe",
        action="store_true",
        help="record baseline cost without enforcing the new bound",
    )
    args = parser.parse_args()
    os.environ.update(QT_QPA_PLATFORM="offscreen", QT_QUICK_BACKEND="software")
    from PySide6.QtCore import (
        Property,
        QAbstractListModel,
        QByteArray,
        QUrl,
        Signal,
        Slot,
        qInstallMessageHandler,
    )
    from PySide6.QtGui import QGuiApplication
    from PySide6.QtQuick import QQuickItem, QQuickView
    from PySide6.QtTest import QTest

    app = QGuiApplication([])
    errors = []
    qInstallMessageHandler(
        lambda mode, context, message: (
            errors.append(message)
            if any(
                w in message
                for w in [
                    "ReferenceError",
                    "TypeError",
                    "Cannot assign",
                    "Unable to assign",
                    "is not a function",
                    "Binding loop",
                ]
            )
            else None
        )
    )
    source = args.source.resolve()
    roles = re.findall(
        r'h.insert\(ROLE_\w+, QByteArray::from\("(\w+)"\)\)',
        (source / "crates/app/src/bridge.rs").read_text(),
    )

    class Model(QAbstractListModel):
        cursorChanged = Signal()

        def __init__(self):
            super().__init__()
            self.at = 0
            self.reads = 0
            self.selected = -1

        def rowCount(self, parent=None):
            return 200 if parent is None or not parent.isValid() else 0

        def roleNames(self):
            return {256 + i: QByteArray(r.encode()) for i, r in enumerate(roles)}

        def data(self, index, role):
            if not index.isValid() or role < 256 or role - 256 >= len(roles):
                return None
            name = roles[role - 256]
            return {
                "name": f"file-{index.row():04}.txt",
                "kind": "file",
                "sizeText": "8 B",
                "modifiedText": "today",
                "isDir": False,
                "isHidden": False,
                "isSymlink": False,
                "linkOk": True,
                "fullPath": "/fixture/file.txt",
                "isSelected": index.row() == self.selected,
                "thumbUrl": "",
                "typeText": "Text",
                "clipMark": "",
                "iconKey": "text",
                "emblems": "",
                "tagKeys": "",
            }.get(name, "")

        def get_cursor(self):
            self.reads += 1
            return self.at

        cursor = Property(int, get_cursor, notify=cursorChanged)
        sort_field = Property(str, lambda self: "name", constant=True)
        sort_ascending = Property(bool, lambda self: True, constant=True)
        empty_reason = Property(str, lambda self: "", constant=True)
        filter = Property(str, lambda self: "", constant=True)
        columns_side = Property(str, lambda self: "", constant=True)

        @Slot(int, int)
        def want_rows(self, first, count):
            pass

        @Slot()
        def request_columns(self):
            pass

        @Slot(int, result=str)
        def entry_name(self, row):
            return f"file-{row:04}.txt"

        @Slot(int, result=str)
        def entry_icon_key(self, row):
            return "text"

        @Slot(int, result=str)
        def entry_emblems(self, row):
            return ""

        def move(self, at):
            self.at = at
            self.cursorChanged.emit()

    with tempfile.TemporaryDirectory(prefix="gust-view-navigation-") as temp:
        root = Path(temp)
        module = root / "com/gust/app"
        module.mkdir(parents=True)
        (module / "qmldir").write_text(
            "module com.gust.app\nsingleton Tokens 1.0 Tokens.qml\n"
        )
        tokens = (
            (source / "ui/Tokens.qml").read_text().replace("import com.gust.app", "")
        )
        start = tokens.index("    property ThemeTokens tokens: ThemeTokens {")
        end = tokens.index("\n    }", start) + len("\n    }")
        tokens = (
            tokens[:start]
            + """    property QtObject tokens: QtObject {
        property int revision: 0
        property int icons_rev: 0
        property string name: "fixture"
        property bool is_light: false
        function color(k) { return k.indexOf("background") >= 0 ? "#202020" : "#80bfff" }
        function metric(k) { return ({font_base:12,spacing_scale:1,selected_background_alpha:0.08,normal_border_alpha:0.4,selection_fill_alpha:0.35,border_width:1})[k] || 0 }
        function icon_look(k) { return "page\\tcode\\t\\t" }
    }"""
            + tokens[end:]
        )
        (module / "Tokens.qml").write_text(tokens)
        results = []
        for kind, object_name in [
            ("RowsView", "rowsList"),
            ("IconGridView", "iconGrid"),
            ("ColumnsView", "columnsList"),
        ]:
            model = Model()
            view = QQuickView()
            view.engine().addImportPath(str(root))
            view.rootContext().setContextProperty("recordingModel", model)
            qml = root / (kind + ".qml")
            qml.write_text(
                "import QtQuick\nimport "
                + json.dumps((source / "ui").as_uri())
                + " as Gust\nGust."
                + kind
                + " { width:1200; height:650; model:recordingModel }\n"
            )
            view.setSource(QUrl.fromLocalFile(str(qml)))
            assert view.status() == QQuickView.Ready, view.errors()
            view.show()
            QTest.qWait(200)
            actual = view.rootObject().findChild(QQuickItem, object_name)
            assert actual is not None
            counts = []
            for at in [1, 2, 3, 4, 5, 4, 3]:
                previous = actual.property("currentItem")
                old_paint = (
                    previous.childItems()[0] if kind == "IconGridView" else previous
                )
                model.reads = 0
                model.move(at)
                QTest.qWait(20)
                counts.append(model.reads)
                assert actual.property("currentIndex") == at
                current = actual.property("currentItem")
                assert current is not None
                paint = current.childItems()[0] if kind == "IconGridView" else current
                assert paint.property("color").alphaF() > 0, kind
                assert old_paint.property("color").alphaF() == 0, kind
            results.append({"view": kind, "cursor_reads_per_step": counts})
            if not args.observe:
                assert max(counts) <= 3, (kind, counts)
            # Offscreen jumps, reuse and returning keep Qt's current item in sync.
            for at in [199, 0, 101, 3]:
                model.move(at)
                QTest.qWait(60)
                assert actual.property("currentIndex") == at
                current = actual.property("currentItem")
                assert current is not None
                paint = current.childItems()[0] if kind == "IconGridView" else current
                assert paint.property("color").alphaF() > 0, kind
            view.hide()
            view.setSource(QUrl())
            view.deleteLater()
            app.processEvents()
        assert not errors, errors
        print(
            json.dumps(
                {
                    "views": results,
                    "qml_errors": errors,
                    "scope": "Actual production QML views; model/theme services are fixtures.",
                },
                indent=2,
            )
        )


if __name__ == "__main__":
    main()
