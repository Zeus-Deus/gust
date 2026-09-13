import QtQuick
import com.gust.app

// A single-line text prompt: rename, new folder, go-to-path.
//
// Rename selects the STEM, leaving the extension editable but not selected —
// the common case is changing the name, not the type.
FocusScope {
    id: root
    visible: opacity > 0
    opacity: 0
    z: 100

    property string title: ""
    property string hint: ""
    property string errorText: ""
    // The row this prompt refers to; -1 when it is not about a row.
    property int targetRow: -1

    signal accepted(string text)
    signal closed()

    function openWith(titleText, initial, hintText, row) {
        root.title = titleText
        root.hint = hintText === undefined ? "" : hintText
        root.errorText = ""
        root.targetRow = row === undefined ? -1 : row
        field.text = initial === undefined ? "" : initial
        root.opacity = 1
        field.forceActiveFocus()
        // Select the stem so typing replaces the name but keeps ".txt".
        var dot = field.text.lastIndexOf(".")
        if (dot > 0)
            field.select(0, dot)
        else
            field.selectAll()
    }

    function showError(message) {
        root.errorText = message
        field.forceActiveFocus()
        field.selectAll()
    }

    function close() {
        root.opacity = 0
        root.errorText = ""
        root.closed()
    }

    Behavior on opacity { NumberAnimation { duration: Tokens.animFast } }

    // Click-away scrim.
    MouseArea {
        anchors.fill: parent
        enabled: root.opacity > 0
        onClicked: root.close()
    }

    Rectangle {
        id: card
        anchors.centerIn: parent
        width: Math.min(parent.width - Tokens.spaceXl * 2, Math.round(420 * Tokens.spacingScale))
        height: column.implicitHeight + Tokens.spaceLg * 2
        color: Tokens.menuBackground
        border.color: root.errorText !== "" ? Tokens.danger : Tokens.border
        border.width: Tokens.borderWidth
        radius: Tokens.radius

        // Swallow clicks so the scrim does not close the prompt.
        MouseArea { anchors.fill: parent }

        Column {
            id: column
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            anchors.margins: Tokens.spaceLg
            spacing: Tokens.spaceMd

            Text {
                text: root.title
                color: Tokens.foreground
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSubtitle
                font.bold: true
            }

            Rectangle {
                width: parent.width
                height: Tokens.controlHeight
                color: Tokens.backgroundDark
                border.color: field.activeFocus ? Tokens.accent : Tokens.border
                border.width: Tokens.borderWidth
                radius: Tokens.radius

                TextInput {
                    id: field
                    anchors.fill: parent
                    anchors.leftMargin: Tokens.spaceMd
                    anchors.rightMargin: Tokens.spaceMd
                    verticalAlignment: TextInput.AlignVCenter
                    color: Tokens.foreground
                    selectionColor: Tokens.selectedBackground
                    selectedTextColor: Tokens.selectedText
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                    clip: true
                    // IME composition must win over file shortcuts.
                    Keys.onPressed: function (event) {
                        if (event.key === Qt.Key_Escape) {
                            root.close()
                            event.accepted = true
                        } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                            if (field.text.length > 0)
                                root.accepted(field.text)
                            event.accepted = true
                        }
                    }
                    onTextChanged: root.errorText = ""
                }
            }

            Text {
                width: parent.width
                text: root.errorText !== "" ? root.errorText : root.hint
                color: root.errorText !== "" ? Tokens.danger : Tokens.muted
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
                wrapMode: Text.Wrap
                visible: text !== ""
            }

            Row {
                spacing: Tokens.spaceMd
                anchors.right: parent.right

                Rectangle {
                    width: cancelLabel.implicitWidth + Tokens.spaceLg
                    height: Tokens.controlHeight
                    color: cancelArea.containsMouse ? Tokens.backgroundLighter : "transparent"
                    border.color: Tokens.border
                    border.width: Tokens.borderWidth
                    radius: Tokens.radius
                    Text {
                        id: cancelLabel
                        anchors.centerIn: parent
                        text: qsTr("Cancel")
                        color: Tokens.foreground
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                    }
                    MouseArea {
                        id: cancelArea
                        anchors.fill: parent
                        hoverEnabled: true
                        onClicked: root.close()
                    }
                }

                Rectangle {
                    width: okLabel.implicitWidth + Tokens.spaceLg
                    height: Tokens.controlHeight
                    color: okArea.containsMouse ? Tokens.withAlpha(Tokens.accent, 0.22)
                                                : "transparent"
                    border.color: Tokens.accent
                    border.width: Tokens.borderWidth
                    radius: Tokens.radius
                    Text {
                        id: okLabel
                        anchors.centerIn: parent
                        text: qsTr("OK")
                        color: Tokens.foregroundBright
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                        font.bold: true
                    }
                    MouseArea {
                        id: okArea
                        anchors.fill: parent
                        hoverEnabled: true
                        onClicked: if (field.text.length > 0) root.accepted(field.text)
                    }
                }
            }
        }
    }
}
