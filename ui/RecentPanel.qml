import QtQuick
import com.gust.app

// Recent files: the desktop's own list (recently-used.xbel), which every GTK
// app and the file dialogs write. Open a file again, or show it in its folder.
Item {
    id: root
    anchors.fill: parent
    visible: open
    z: 60

    property bool open: false
    property var items: []
    property string error: ""
    property int current: 0

    signal openRequested(string path)
    signal revealRequested(string path)
    signal closed()

    function show(json) {
        try { root.items = JSON.parse(json) } catch (e) { root.items = [] }
        root.error = ""
        root.current = 0
        root.open = true
        Qt.callLater(() => keys.forceActiveFocus())
    }
    function hide() { root.open = false; root.closed() }

    Rectangle {
        anchors.fill: parent
        color: Tokens.withAlpha(Tokens.background, 0.82)
        MouseArea { anchors.fill: parent; onClicked: root.hide() }
    }

    Item {
        id: keys
        focus: root.open
        Keys.onPressed: (event) => {
            if (event.key === Qt.Key_Escape) { root.hide(); event.accepted = true }
            else if (event.key === Qt.Key_Down) { root.current = Math.min(root.items.length - 1, root.current + 1); event.accepted = true }
            else if (event.key === Qt.Key_Up) { root.current = Math.max(0, root.current - 1); event.accepted = true }
            else if ((event.key === Qt.Key_Return || event.key === Qt.Key_Enter) && root.items.length > 0) {
                root.openRequested(root.items[root.current].path); event.accepted = true
            }
        }
    }

    Rectangle {
        anchors.centerIn: parent
        width: Math.min(parent.width - Tokens.spaceXl * 4, Math.round(Tokens.rowHeight * 30))
        height: Math.min(parent.height - Tokens.spaceXl * 4, Math.round(Tokens.rowHeight * 22))
        color: Tokens.backgroundDark
        border.width: Math.max(1, Tokens.borderWidth)
        border.color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
        MouseArea { anchors.fill: parent }

        Column {
            id: head
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.margins: Tokens.spaceXl
            spacing: Tokens.spaceSm
            Text {
                text: qsTr("Recent files")
                color: Tokens.foregroundBright
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSubtitle
            }
            Text {
                text: root.items.length === 0 ? qsTr("No recent files yet")
                      : root.items.length === 1 ? qsTr("1 item") : qsTr("%1 items").arg(root.items.length)
                color: Tokens.foregroundDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
            }
            Text {
                visible: root.error !== ""
                width: parent.width
                text: root.error
                color: Tokens.danger
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
                wrapMode: Text.Wrap
            }
        }

        ListView {
            id: list
            WheelScroll { view: list }
            objectName: "recentList"
            anchors.top: head.bottom
            anchors.topMargin: Tokens.spaceLg
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: foot.top
            anchors.margins: Tokens.spaceSm
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            model: root.items
            currentIndex: root.current
            delegate: Rectangle {
                id: row
                required property var modelData
                required property int index
                width: list.width
                height: Tokens.rowHeight * 1.7
                color: index === root.current
                       ? Tokens.withAlpha(Tokens.selectedBackground, Math.max(Tokens.selectedAlpha, 0.14))
                       : rowMouse.containsMouse ? Tokens.withAlpha(Tokens.foreground, 0.05) : "transparent"
                MouseArea {
                    id: rowMouse
                    anchors.fill: parent
                    hoverEnabled: true
                    onClicked: root.current = row.index
                }
                Column {
                    anchors.left: parent.left
                    anchors.leftMargin: Tokens.rowPaddingX
                    anchors.right: buttons.left
                    anchors.rightMargin: Tokens.spaceLg
                    anchors.verticalCenter: parent.verticalCenter
                    Text {
                        width: parent.width
                        text: row.modelData.name
                        color: row.modelData.exists ? Tokens.foreground : Tokens.foregroundDim
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                        elide: Text.ElideMiddle
                    }
                    Text {
                        width: parent.width
                        text: (row.modelData.exists ? "" : qsTr("gone · ")) + qsTr("%1 · %2").arg(row.modelData.folder).arg(row.modelData.when)
                        color: Tokens.foregroundDim
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontCaption
                        elide: Text.ElideLeft
                    }
                }
                Row {
                    id: buttons
                    anchors.right: parent.right
                    anchors.rightMargin: Tokens.spaceLg
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: Tokens.spaceSm
                    Repeater {
                        model: row.modelData.exists ? [["open", qsTr("Open")], ["show", qsTr("Show in folder")]] : []
                        delegate: Rectangle {
                            required property var modelData
                            width: bl.implicitWidth + Tokens.spaceLg * 2
                            height: Tokens.controlHeight - Tokens.spaceSm
                            color: bm.containsMouse ? Tokens.withAlpha(Tokens.accent, 0.25) : "transparent"
                            border.color: modelData[0] === "open" ? Tokens.accent : Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                            border.width: Tokens.borderWidth
                            Text {
                                id: bl
                                anchors.centerIn: parent
                                text: modelData[1]
                                color: Tokens.foreground
                                font.family: Tokens.fontFamily
                                font.pixelSize: Tokens.fontSmall
                            }
                            MouseArea {
                                id: bm
                                anchors.fill: parent
                                hoverEnabled: true
                                cursorShape: Qt.PointingHandCursor
                                onClicked: {
                                    root.current = row.index
                                    if (modelData[0] === "open") root.openRequested(row.modelData.path)
                                    else root.revealRequested(row.modelData.path)
                                }
                            }
                            Accessible.role: Accessible.Button
                            Accessible.name: modelData[1] + " " + row.modelData.name
                        }
                    }
                }
            }
        }

        Text {
            id: foot
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            anchors.margins: Tokens.spaceXl
            text: qsTr("Enter opens · Escape closes")
            color: Tokens.foregroundDim
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
        }
    }

    Accessible.role: Accessible.Dialog
    Accessible.name: qsTr("Recent files")
}
