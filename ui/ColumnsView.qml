import QtQuick
import com.gust.app

// Columns — parents stay visible, with a preview pane at the right.
// Selecting a file PREVIEWS it; it never launches automatically.
FocusScope {
    id: root
    required property var model
    signal activated(int row)
    signal selectionTouched()
    // Declared so every view exposes the same drag contract, even where the
    // gesture is not wired yet: a handler assigned to a missing signal stops
    // the whole app from starting.
    signal dragStarted(int row)
    signal dropMovedOver(int row)
    signal dragMovedTo(real x, real y)
    function rowAt(x, y) {
        const p = mapToItem(list.contentItem, x, y)
        return list.indexAt(p.x, p.y)
    }
    signal dropReleased(int row)
    property bool dragActive: false
    property int dropRow: -1
    signal contextRequested(int row, real x, real y)
    /// Middle-click on a row: open it in a new tab (Main decides).
    signal middleClicked(int row)

    // Rows on screen, so details are read for what is visible (RowsView).
    function reportRows() {
        if (!root.model || list.count === 0) return
        // Arithmetic, not indexAt(): at the very bottom indexAt() can miss
        // and answer -1, which read as "row 0" and fetched nothing.
        const first = Math.max(0, Math.floor((list.contentY - list.originY) / Tokens.rowHeight))
        root.model.want_rows(first, Math.ceil(list.height / Tokens.rowHeight) + 1)
    }
    Timer { id: colsReport; interval: 40; onTriggered: root.reportRows() }

    // Parent and preview columns, filled on a worker (see request_columns).
    readonly property var side: {
        const s = root.model ? root.model.columns_side : ""
        if (!s) return null
        try { return JSON.parse(s) } catch (e) { return null }
    }
    // Ask again when the cursor, the folder or visibility changes; a short
    // delay coalesces a held-down arrow key into one request.
    Timer {
        id: sideTimer
        interval: 60
        onTriggered: if (root.visible && root.model) root.model.request_columns()
    }
    Connections {
        target: root.model
        function onCursorChanged() { sideTimer.restart() }
        function onLocationChanged() { sideTimer.restart() }
        function onCountChanged() { sideTimer.restart() }
    }
    onVisibleChanged: if (visible) sideTimer.restart()
    onModelChanged: sideTimer.restart()

    function cursorAnchor() {
        const it = list.itemAtIndex(root.model ? root.model.cursor : -1)
        if (it) return it.mapToItem(null, Tokens.rowPaddingX * 3, it.height)
        return list.mapToItem(null, Tokens.rowPaddingX, 0)
    }

    Rectangle {
        anchors.fill: parent
        color: Tokens.background

        MouseArea {
            anchors.fill: parent
            acceptedButtons: Qt.RightButton
            onClicked: (mouse) => {
                const g = mapToItem(null, mouse.x, mouse.y)
                root.contextRequested(-1, g.x, g.y)
            }
        }

        Row {
            anchors.fill: parent

            // Parent column: where we came from, kept in view.
            Rectangle {
                id: parentCol
                width: Math.round(parent.width * 0.26)
                height: parent.height
                color: Tokens.backgroundDark

                // The folder above, with this one marked. Click a sibling to
                // go there — the column is navigation, not a label.
                ListView {
                    id: parentList
                    objectName: "columnsParent"
                    anchors.fill: parent
                    clip: true
                    boundsBehavior: Flickable.StopAtBounds
                    model: root.side && root.side.parent ? root.side.parent.names : []
                    header: Text {
                        width: parentList.width
                        height: Tokens.controlHeight
                        leftPadding: Tokens.rowPaddingX
                        verticalAlignment: Text.AlignVCenter
                        text: root.side && root.side.parent
                              ? (root.side.parent.error !== "" ? root.side.parent.error
                                 : root.side.parent.path)
                              : ""
                        color: Tokens.foregroundDim
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontCaption
                        elide: Text.ElideLeft
                    }
                    delegate: Rectangle {
                        required property var modelData
                        width: parentList.width
                        height: Tokens.rowHeight
                        color: modelData.cur
                               ? Tokens.withAlpha(Tokens.selectedBackground, Math.max(Tokens.selectedAlpha, 0.14))
                               : pMouse.containsMouse ? Tokens.withAlpha(Tokens.foreground, 0.06) : "transparent"
                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            leftPadding: Tokens.rowPaddingX
                            width: parent.width - Tokens.rowPaddingX
                            text: modelData.n + (modelData.d ? "  ›" : "")
                            color: modelData.cur ? Tokens.foregroundBright
                                   : modelData.d ? Tokens.foreground : Tokens.foregroundDim
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                            elide: Text.ElideMiddle
                        }
                        MouseArea {
                            id: pMouse
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: modelData.d ? Qt.PointingHandCursor : Qt.ArrowCursor
                            onClicked: {
                                if (!modelData.d || modelData.cur) return
                                root.model.navigate(root.side.parent.path + "/" + modelData.n)
                            }
                        }
                    }
                    footer: Text {
                        visible: root.side && root.side.parent && root.side.parent.more > 0
                        width: parentList.width
                        leftPadding: Tokens.rowPaddingX
                        text: root.side && root.side.parent ? qsTr("and %1 more").arg(root.side.parent.more) : ""
                        color: Tokens.foregroundDim
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontCaption
                    }
                }

                Rectangle {
                    anchors.right: parent.right
                    width: Tokens.borderWidth
                    height: parent.height
                    color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                }
            }

            // Current column.
            Rectangle {
                id: currentCol
                width: Math.round(parent.width * 0.37)
                height: parent.height
                color: Tokens.background

                ListView {
                    id: list
                    objectName: "columnsList"
                    anchors.fill: parent
                    model: root.model
                    clip: true
                    focus: true
                    currentIndex: root.model ? root.model.cursor : 0
                    highlightMoveDuration: 0
                    boundsBehavior: Flickable.StopAtBounds
                    onContentYChanged: colsReport.restart()
                    onHeightChanged: colsReport.restart()
                    onCountChanged: colsReport.restart()
                    onCurrentIndexChanged: colsReport.restart()

                    delegate: Rectangle {
                        width: list.width
                        height: Tokens.rowHeight
                        color: index === root.model.cursor
                               ? Tokens.withAlpha(Tokens.selectedBackground, Math.max(Tokens.selectedAlpha, 0.14))
                               : "transparent"

                        Row {
                            anchors.fill: parent
                            anchors.leftMargin: Tokens.rowPaddingX
                            anchors.rightMargin: Tokens.spaceLg
                            spacing: Tokens.spaceLg

                            Text {
                                anchors.verticalCenter: parent.verticalCenter
                                text: name
                                // Hidden files dimmed, as in Rows and Grid.
                                color: isHidden ? Tokens.foregroundDim
                                                : (isDir ? Tokens.foregroundBright : Tokens.foreground)
                                font.family: Tokens.fontFamily
                                font.pixelSize: Tokens.fontBody
                                elide: Text.ElideMiddle
                                width: parent.width - chevron.width - Tokens.spaceLg
                            }
                            Text {
                                id: chevron
                                anchors.verticalCenter: parent.verticalCenter
                                text: isDir ? "›" : ""
                                color: Tokens.foregroundDim
                                font.family: Tokens.fontFamily
                                font.pixelSize: Tokens.fontBody
                            }
                        }

                        MouseArea {
                            anchors.fill: parent
                            acceptedButtons: Qt.LeftButton | Qt.RightButton | Qt.MiddleButton
                            preventStealing: true
                            property point pressPos: Qt.point(0, 0)
                            property bool armed: false
                            onPressed: (mouse) => {
                                pressPos = Qt.point(mouse.x, mouse.y)
                                armed = (mouse.button === Qt.LeftButton)
                            }
                            onReleased: {
                                if (root.dragActive)
                                    root.dropReleased(root.dropRow)
                                armed = false
                                root.dragActive = false
                                root.dropRow = -1
                            }
                            onPositionChanged: (mouse) => {
                                if (!armed)
                                    return
                                if (!root.dragActive) {
                                    const dx = mouse.x - pressPos.x
                                    const dy = mouse.y - pressPos.y
                                    if (dx * dx + dy * dy < 64)
                                        return
                                    root.dragStarted(index)
                                }
                                const gp = mapToItem(list.contentItem, mouse.x, mouse.y)
                                root.dropRow = list.indexAt(gp.x, gp.y)
                                root.dropMovedOver(root.dropRow)
                                const vp = mapToItem(root, mouse.x, mouse.y)
                                root.dragMovedTo(vp.x, vp.y)
                            }
                            onClicked: (mouse) => {
                                if (mouse.button === Qt.MiddleButton) { root.middleClicked(index); return }
                                if (mouse.button === Qt.RightButton) {
                                    if (!root.model.sel_contains(index))
                                        root.model.sel_only(index)
                                    root.model.focus_row(index)
                                    const g = mapToItem(null, mouse.x, mouse.y)
                                    root.contextRequested(index, g.x, g.y)
                                    return
                                }
                                root.model.focus_row(index)
                            }
                            onDoubleClicked: root.activated(index)
                        }

                        Accessible.role: Accessible.ListItem
                        Accessible.name: name + (isDir ? ", folder" : ", file")
                    }
                }

                Rectangle {
                    anchors.right: parent.right
                    width: Tokens.borderWidth
                    height: parent.height
                    color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                }
            }

            // Preview column: a folder's first entries, or a file's kind,
            // size and (for text) its first lines — from a bounded read on
            // a worker. Nothing is decoded here and nothing is run.
            Rectangle {
                width: parent.width - parentCol.width - currentCol.width
                height: parent.height
                color: Tokens.backgroundDark
                readonly property var peek: root.side ? root.side.peek : null

                Column {
                    id: prevHead
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.top: parent.top
                    anchors.margins: Tokens.spaceXl
                    spacing: Tokens.spaceSm
                    Text {
                        width: parent.width
                        text: parent.parent.peek ? parent.parent.peek.name : ""
                        color: Tokens.foregroundBright
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSubtitle
                        wrapMode: Text.WrapAnywhere
                    }
                    Text {
                        width: parent.width
                        readonly property var p: parent.parent.peek
                        text: !p ? qsTr("Nothing selected")
                              : p.kind === "dir"
                                ? (p.error !== "" ? p.error
                                   : (p.names.length + p.more) === 0 ? qsTr("Empty folder")
                                   : qsTr("Folder · %1 items").arg(p.names.length + p.more))
                              : [p.kindWords, p.sizeText, p.mode].filter(x => x).join("  ·  ")
                        color: Tokens.foregroundDim
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSmall
                        wrapMode: Text.Wrap
                    }
                    Rectangle {
                        width: parent.width
                        height: Tokens.borderWidth
                        color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                    }
                }

                // Folder: its entries (click one to go straight there).
                ListView {
                    id: peekList
                    objectName: "columnsPeek"
                    visible: parent.peek && parent.peek.kind === "dir"
                    anchors.top: prevHead.bottom
                    anchors.topMargin: Tokens.spaceSm
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.bottom: parent.bottom
                    clip: true
                    boundsBehavior: Flickable.StopAtBounds
                    model: visible ? parent.peek.names : []
                    delegate: Rectangle {
                        required property var modelData
                        width: peekList.width
                        height: Tokens.rowHeight
                        color: kMouse.containsMouse ? Tokens.withAlpha(Tokens.foreground, 0.06) : "transparent"
                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            leftPadding: Tokens.rowPaddingX
                            width: parent.width - Tokens.rowPaddingX
                            text: modelData.n + (modelData.d ? "  ›" : "")
                            color: modelData.d ? Tokens.foreground : Tokens.foregroundDim
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                            elide: Text.ElideMiddle
                        }
                        MouseArea {
                            id: kMouse
                            anchors.fill: parent
                            hoverEnabled: true
                            onClicked: {
                                const here = root.model.entry_path(root.model.cursor)
                                if (modelData.d) root.model.navigate(here + "/" + modelData.n)
                                else root.model.navigate(here)
                            }
                        }
                    }
                }

                // File: first lines of text, or why there is no preview.
                Flickable {
                    visible: parent.peek && parent.peek.kind === "file"
                    anchors.top: prevHead.bottom
                    anchors.topMargin: Tokens.spaceSm
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.bottom: parent.bottom
                    anchors.leftMargin: Tokens.spaceXl
                    anchors.rightMargin: Tokens.spaceXl
                    clip: true
                    contentHeight: peekText.implicitHeight
                    boundsBehavior: Flickable.StopAtBounds
                    Text {
                        id: peekText
                        width: parent.width
                        readonly property var p: parent.parent.peek
                        text: !p || p.kind !== "file" ? ""
                              : p.lines && p.lines.length > 0 ? p.lines.join("\n")
                              : p.noPreview
                        color: p && p.lines && p.lines.length > 0 ? Tokens.foreground : Tokens.foregroundDim
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontCaption
                        textFormat: Text.PlainText
                        wrapMode: p && p.lines && p.lines.length > 0 ? Text.NoWrap : Text.Wrap
                    }
                }
            }
        }
    }

    Accessible.role: Accessible.List
    Accessible.name: "File list, columns view with preview"
}
