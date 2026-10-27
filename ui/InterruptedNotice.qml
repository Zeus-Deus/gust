import QtQuick
import com.gust.app

// Shown once after a crash: what was interrupted, where, how far it got.
// The durable journal knows this; without a surface the user would find a
// half-copied folder weeks later and not know why. It does NOT know whether
// leftovers were cleaned up: a remote place that could not be reached keeps
// its temporary files on purpose, and an item may have been completed just
// before the crash. So the notice says what may be there and sends the user
// to look ("Open destination") before running anything again; nothing is
// retried for them. Dismiss records that the user has seen it.
Rectangle {
    id: root

    // [{ id, text, dest }] from DirModel.interrupted_jobs().
    property var items: []
    /// What to do next. No claim about cleanup: the journal records what
    /// was attempted, not what is left at the destination.
    readonly property string guidance: qsTr("Some items may already be complete and temporary files may remain. Check the destination before running it again.")
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
            textFormat: Text.PlainText
            text: root.items.length === 1
                  ? qsTr("Gust closed during an operation")
                  : qsTr("Gust closed during %1 operations").arg(root.items.length)
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
                    textFormat: Text.PlainText
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
                    // The primary action: checking what arrived is the one
                    // thing to do next, so it is the one that stands out.
                    color: Tokens.withAlpha(Tokens.accent, openArea.containsMouse ? 0.30 : 0.18)
                    border.color: Tokens.accent
                    border.width: Math.max(1, Tokens.borderWidth)
                    Text {
                        textFormat: Text.PlainText
                        id: openLabel
                        anchors.centerIn: parent
                        text: qsTr("Open destination")
                        color: Tokens.foregroundBright
                        font.bold: true
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
            height: Math.max(Tokens.controlHeight, guidanceText.implicitHeight)
            Text {
                id: guidanceText
                objectName: "interruptedGuidance"
                textFormat: Text.PlainText
                anchors.left: parent.left
                anchors.right: dismissButton.left
                anchors.rightMargin: Tokens.spaceLg
                anchors.verticalCenter: parent.verticalCenter
                wrapMode: Text.Wrap
                text: root.guidance
                // The instruction is what the user acts on: primary text,
                // not secondary grey (4.5:1 on every theme, not 3:1).
                color: Tokens.foreground
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
            }
            Rectangle {
                id: dismissButton
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                width: dismissLabel.implicitWidth + Tokens.spaceLg * 2
                height: Tokens.controlHeight - Tokens.spaceSm
                // Secondary: no outline until hovered.
                color: dismissArea.containsMouse ? Tokens.selectedBackground : "transparent"
                border.color: dismissArea.containsMouse ? Tokens.border : "transparent"
                border.width: Tokens.borderWidth
                Text {
                    textFormat: Text.PlainText
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
