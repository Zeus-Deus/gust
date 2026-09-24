import QtQuick
import com.gust.app

// Destructive-action confirmation.
//
// Rules (DESIGN.md §8): Trash and permanent delete never share wording, and
// for an irreversible delete CANCEL is the focused default.
Rectangle {
    id: root
    property string title: ""
    property string body: ""
    property string confirmText: "Confirm"
    property bool dangerous: false
    signal accepted()
    signal rejected()
    /// Emitted whenever the dialog stops being visible. The window uses it to
    /// restore keyboard focus to the file list — otherwise focus stays on a
    /// hidden button and the app appears frozen to the keyboard.
    signal closed()

    anchors.fill: parent
    color: Tokens.withAlpha(Tokens.background, 0.72)
    visible: false
    z: 100

    function open(t, b, c, danger) {
        title = t; body = b; confirmText = c; dangerous = danger
        visible = true
        // Cancel takes focus for a dangerous action.
        if (danger) cancelButton.forceActiveFocus()
        else confirmButton.forceActiveFocus()
    }
    function close() {
        visible = false
        closed()
    }

    MouseArea {
        anchors.fill: parent
        onClicked: { root.rejected(); root.close() }
    }

    Rectangle {
        anchors.centerIn: parent
        width: Math.min(parent.width * 0.7, Math.round(420 * Tokens.spacingScale))
        height: content.implicitHeight + Tokens.spaceXxl * 2
        color: Tokens.menuBackground
        border.width: Math.max(1, Tokens.borderWidth)
        border.color: root.dangerous ? Tokens.danger : Tokens.border
        radius: Tokens.radius

        MouseArea { anchors.fill: parent } // swallow clicks

        Column {
            id: content
            anchors.centerIn: parent
            width: parent.width - Tokens.spaceXxl * 2
            spacing: Tokens.spaceXl

            Text {
                width: parent.width
                text: root.title
                color: root.dangerous ? Tokens.danger : Tokens.foregroundBright
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSubtitle
                font.bold: true
                wrapMode: Text.Wrap
                objectName: "confirmTitle"
            }

            Text {
                width: parent.width
                text: root.body
                color: Tokens.foreground
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontBody
                wrapMode: Text.Wrap
                objectName: "confirmBody"
            }

            Row {
                anchors.right: parent.right
                spacing: Tokens.spaceLg

                Rectangle {
                    id: cancelButton
                    width: cancelLabel.implicitWidth + Tokens.spaceXxl
                    height: Tokens.controlHeight
                    color: activeFocus ? Tokens.withAlpha(Tokens.foreground, 0.14) : "transparent"
                    border.width: Math.max(1, Tokens.borderWidth)
                    border.color: activeFocus ? Tokens.accent
                                              : Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                    radius: Tokens.radius
                    focus: root.dangerous
                    activeFocusOnTab: true
                    objectName: "cancelButton"

                    Text {
                        id: cancelLabel
                        anchors.centerIn: parent
                        text: "Cancel"
                        color: Tokens.foreground
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                    }
                    MouseArea {
                        anchors.fill: parent
                        onClicked: { root.rejected(); root.close() }
                    }
                    Keys.onReturnPressed: { root.rejected(); root.close() }
                    Keys.onSpacePressed: { root.rejected(); root.close() }
                    Accessible.role: Accessible.Button
                    Accessible.name: "Cancel"
                }

                Rectangle {
                    id: confirmButton
                    width: confirmLabel.implicitWidth + Tokens.spaceXxl
                    height: Tokens.controlHeight
                    color: activeFocus
                           ? Tokens.withAlpha(root.dangerous ? Tokens.danger : Tokens.accent, 0.2)
                           : "transparent"
                    border.width: Math.max(1, Tokens.borderWidth)
                    border.color: root.dangerous ? Tokens.danger : Tokens.accent
                    radius: Tokens.radius
                    activeFocusOnTab: true
                    objectName: "confirmButton"

                    Text {
                        id: confirmLabel
                        anchors.centerIn: parent
                        text: root.confirmText
                        color: root.dangerous ? Tokens.danger : Tokens.foregroundBright
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                    }
                    MouseArea {
                        anchors.fill: parent
                        onClicked: { root.accepted(); root.close() }
                    }
                    Keys.onReturnPressed: { root.accepted(); root.close() }
                    Keys.onSpacePressed: { root.accepted(); root.close() }
                    Accessible.role: Accessible.Button
                    Accessible.name: root.confirmText
                }
            }
        }
    }

    Keys.onEscapePressed: { root.rejected(); root.close() }

    Accessible.role: Accessible.Dialog
    Accessible.name: root.title
}
