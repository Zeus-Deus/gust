import QtQuick
import com.gust.app

// Shown once after a crash: what was interrupted, where, how far it got.
// The durable journal knows this; without a surface the user would find a
// half-copied folder weeks later and not know why. Partial staging files
// are already swept, so the only question left is what did arrive — hence
// "Open destination". Dismiss records that the user has seen it.
Rectangle {
    id: root

    // [{ id, text, dest }] from DirModel.interrupted_jobs().
    property var items: []
    readonly property bool shown: items.length > 0

    signal openDestination(string path)
    signal dismiss()

    height: shown ? col.implicitHeight + Tokens.spaceLg * 2 : 0
    visible: height > 0
    clip: true
    color: Tokens.menuBackground
    border.color: Tokens.warn
    border.width: Tokens.borderWidth

    Column {
        id: col
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        anchors.leftMargin: Tokens.rowPaddingX
        anchors.rightMargin: Tokens.rowPaddingX
        spacing: Tokens.spaceSm

        Text {
            text: root.items.length === 1
                  ? qsTr("Gust closed unexpectedly while it was working")
                  : qsTr("Gust closed unexpectedly while %1 operations were running").arg(root.items.length)
            color: Tokens.warn
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontBody
            font.bold: true
        }

        Repeater {
            model: root.items
            delegate: Item {
                required property var modelData
                width: col.width
                height: Tokens.controlHeight

                Text {
                    anchors.left: parent.left
                    anchors.right: openBtn.left
                    anchors.rightMargin: Tokens.spaceLg
                    anchors.verticalCenter: parent.verticalCenter
                    text: modelData.text
                    elide: Text.ElideMiddle
                    color: Tokens.foreground
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontSmall
                }

                Rectangle {
                    id: openBtn
                    visible: modelData.dest !== ""
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    width: openLabel.implicitWidth + Tokens.spaceLg * 2
                    height: Tokens.controlHeight - Tokens.spaceSm
                    color: openArea.containsMouse ? Tokens.selectedBackground : "transparent"
                    border.color: Tokens.border
                    border.width: Tokens.borderWidth
                    Text {
                        id: openLabel
                        anchors.centerIn: parent
                        text: qsTr("Open destination")
                        color: Tokens.foreground
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSmall
                    }
                    MouseArea {
                        id: openArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: root.openDestination(modelData.dest)
                    }
                    Accessible.role: Accessible.Button
                    Accessible.name: qsTr("Open destination")
                }
            }
        }

        Item {
            width: col.width
            height: Tokens.controlHeight
            Text {
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
                text: qsTr("Partial files were cleaned up. Check what arrived, then run it again.")
                color: Tokens.foregroundDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
            }
            Rectangle {
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                width: dismissLabel.implicitWidth + Tokens.spaceLg * 2
                height: Tokens.controlHeight - Tokens.spaceSm
                color: dismissArea.containsMouse ? Tokens.selectedBackground : "transparent"
                border.color: Tokens.border
                border.width: Tokens.borderWidth
                Text {
                    id: dismissLabel
                    anchors.centerIn: parent
                    text: qsTr("Dismiss")
                    color: Tokens.foreground
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontSmall
                }
                MouseArea {
                    id: dismissArea
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: root.dismiss()
                }
                Accessible.role: Accessible.Button
                Accessible.name: qsTr("Dismiss")
            }
        }
    }
}
