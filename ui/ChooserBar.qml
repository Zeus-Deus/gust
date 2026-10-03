import QtQuick
import com.gust.app

// Shown only when another app's Open or Save dialog started this window.
// One strip: what the app asked, a name field for Save, Cancel, and the
// primary action. Everything else in the window works as usual, so the
// user picks files the way they always do in Gust.
Rectangle {
    id: root

    property string mode: ""          // open | open-many | open-folder | save
    property string title: ""
    property alias name: nameField.text
    property string error: ""
    readonly property bool active: mode !== ""

    signal accepted()
    signal cancelled()

    function focusName() { nameField.forceActiveFocus(); nameField.selectAll() }

    height: active ? Tokens.controlHeight + Tokens.spaceLg * 2
                     + (error !== "" ? Tokens.fontCaption + Tokens.spaceXs : 0) : 0
    visible: active
    color: Tokens.menuBackground
    border.color: Tokens.accent
    border.width: Tokens.borderWidth

    readonly property string primaryLabel:
        mode === "save" ? qsTr("Save")
        : mode === "open-folder" ? qsTr("Choose this folder")
        : qsTr("Open")

    Row {
        id: row
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.margins: Tokens.spaceLg
        spacing: Tokens.spaceLg
        height: Tokens.controlHeight

        Text {
            id: label
            anchors.verticalCenter: parent.verticalCenter
            width: Math.min(implicitWidth, Math.round(row.width * 0.35))
            text: root.title !== "" ? root.title
                  : root.mode === "save" ? qsTr("Save as")
                  : root.mode === "open-folder" ? qsTr("Choose a folder")
                  : root.mode === "open-many" ? qsTr("Choose files")
                  : qsTr("Choose a file")
            color: Tokens.foregroundBright
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontBody
            elide: Text.ElideRight
        }

        Rectangle {
            visible: root.mode === "save"
            anchors.verticalCenter: parent.verticalCenter
            width: row.width - label.width - cancelBtn.width - okBtn.width - Tokens.spaceLg * 3
            height: Tokens.controlHeight - Tokens.spaceSm
            color: Tokens.background
            border.color: nameField.activeFocus ? Tokens.accent : Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
            border.width: Tokens.borderWidth
            TextInput {
                id: nameField
                objectName: "chooserName"
                anchors.fill: parent
                anchors.leftMargin: Tokens.spaceMd
                anchors.rightMargin: Tokens.spaceMd
                verticalAlignment: TextInput.AlignVCenter
                color: Tokens.foreground
                selectionColor: Tokens.accent
                selectedTextColor: Tokens.background
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontBody
                clip: true
                Keys.onReturnPressed: root.accepted()
                Keys.onEnterPressed: root.accepted()
                Keys.onEscapePressed: root.cancelled()
            }
        }
        Item {
            visible: root.mode !== "save"
            width: row.width - label.width - cancelBtn.width - okBtn.width - Tokens.spaceLg * 3
            height: 1
        }

        Rectangle {
            id: cancelBtn
            anchors.verticalCenter: parent.verticalCenter
            width: cancelLabel.implicitWidth + Tokens.spaceXl * 2
            height: Tokens.controlHeight - Tokens.spaceSm
            color: cancelArea.containsMouse ? Tokens.withAlpha(Tokens.foreground, 0.08) : "transparent"
            border.color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
            border.width: Tokens.borderWidth
            Text {
                id: cancelLabel
                anchors.centerIn: parent
                text: qsTr("Cancel")
                color: Tokens.foreground
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
            }
            MouseArea {
                id: cancelArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: root.cancelled()
            }
            Accessible.role: Accessible.Button
            Accessible.name: qsTr("Cancel")
        }

        Rectangle {
            id: okBtn
            objectName: "chooserAccept"
            anchors.verticalCenter: parent.verticalCenter
            width: okLabel.implicitWidth + Tokens.spaceXl * 2
            height: Tokens.controlHeight - Tokens.spaceSm
            color: okArea.containsMouse ? Tokens.withAlpha(Tokens.accent, 0.35) : Tokens.withAlpha(Tokens.accent, 0.22)
            border.color: Tokens.accent
            border.width: Tokens.borderWidth
            Text {
                id: okLabel
                anchors.centerIn: parent
                text: root.primaryLabel
                color: Tokens.foregroundBright
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
                font.bold: true
            }
            MouseArea {
                id: okArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: root.accepted()
            }
            Accessible.role: Accessible.Button
            Accessible.name: root.primaryLabel
        }
    }

    Text {
        anchors.left: parent.left
        anchors.leftMargin: Tokens.spaceLg
        anchors.top: row.bottom
        anchors.topMargin: Tokens.spaceXs
        visible: root.error !== ""
        text: root.error
        color: Tokens.danger
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontCaption
    }
}
