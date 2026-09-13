import QtQuick
import com.gust.app

// Compact hint strip. Shows the keys for the ACTIVE preset only — Vim hints
// must never be presented as mandatory to a Standard-preset user.
Rectangle {
    id: root
    property string preset: "standard"
    property string status: ""
    /// Live selection summary. Non-empty means a selection exists.
    property string selection: ""
    /// A message the user must actually read: a refusal, a failure, a reason
    /// something did not happen. Outranks the selection count, which is only
    /// ambient state, and is cleared by the next action.
    property string alert: ""

    readonly property string displayText:
        root.alert !== "" ? root.alert
        : (root.selection !== "" ? root.selection : root.status)

    implicitHeight: Tokens.controlHeight
    color: Tokens.backgroundDark

    Rectangle {
        anchors.top: parent.top
        width: parent.width
        height: Tokens.borderWidth
        color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
    }

    Row {
        anchors.left: parent.left
        anchors.leftMargin: Tokens.rowPaddingX
        anchors.verticalCenter: parent.verticalCenter
        spacing: Tokens.spaceXl

        Repeater {
            model: root.preset === "vim"
                ? [
                    { k: "j k", d: "move" },
                    { k: "l", d: "open" },
                    { k: "h", d: "up" },
                    { k: "Space", d: "quick look" },
                    { k: "/", d: "filter" },
                    { k: ":", d: "command" },
                    { k: "?", d: "keys" }
                  ]
                : [
                    { k: "↑ ↓", d: "move" },
                    { k: "Enter", d: "open" },
                    { k: "Backspace", d: "up" },
                    { k: "Space", d: "quick look" },
                    { k: "/", d: "filter" },
                    { k: ":", d: "command" },
                    { k: "F1", d: "keys" }
                  ]

            delegate: Row {
                spacing: Tokens.spaceSm
                Text {
                    text: modelData.k
                    color: Tokens.accent
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontCaption
                }
                Text {
                    text: modelData.d
                    color: Tokens.foregroundDim
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontCaption
                }
            }
        }
    }

    Text {
        anchors.right: parent.right
        anchors.rightMargin: Tokens.rowPaddingX
        anchors.verticalCenter: parent.verticalCenter
        text: root.displayText
        // An alert is the one thing here the user must actually read.
        color: root.alert !== "" ? Tokens.danger : Tokens.foregroundDim
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontCaption
        objectName: "statusText"
    }

    Accessible.role: Accessible.StaticText
    Accessible.name: "Keyboard hints and status: " + root.displayText
}
