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
    /// A modal dialog is up: its keys, not the list's, are what work now
    /// (a stale "Enter open" under a Run prompt would mislead).
    property bool dialog: false
    readonly property var dialogKeys: [
        { k: "Tab", d: "switch button" },
        { k: "Enter", d: "press it" },
        { k: "Esc", d: "cancel" }
    ]
    /// The key labels shown now, for tests.
    readonly property string shownKeys: {
        const m = root.dialog ? root.dialogKeys : hintRep.model
        return m.map(x => x.k + " " + x.d).join(" · ")
    }

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

    /// Hints drawn but not whole (qa): must always be 0.
    function cutHints() {
        let n = 0
        for (let i = 0; i < hintRep.count; i++) {
            const it = hintRep.itemAt(i)
            if (it && it.opacity > 0 && it.x + it.width > hintRow.width) n++
        }
        return n
    }

    Row {
        id: hintRow
        anchors.left: parent.left
        anchors.leftMargin: Tokens.rowPaddingX
        anchors.verticalCenter: parent.verticalCenter
        spacing: Tokens.spaceXl
        // In a narrow (tiled) window the hints and the status used to
        // overlap into one unreadable line. The status is about what just
        // happened, so it wins: hints that would collide are clipped away.
        width: Math.max(0, statusText.x - Tokens.spaceXl - x)
        clip: true

        Repeater {
            id: hintRep
            model: root.dialog ? root.dialogKeys : root.preset === "vim"
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
                // A hint that does not fit whole is hidden whole, never cut
                // through its letters. Opacity, not visible: it must keep
                // its place so the Row's layout does not depend on itself.
                opacity: x + width <= hintRow.width ? 1 : 0
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
        id: statusText
        anchors.right: parent.right
        anchors.rightMargin: Tokens.rowPaddingX
        anchors.verticalCenter: parent.verticalCenter
        // Never wider than two thirds of the strip; a long path elides.
        width: Math.min(implicitWidth, Math.round(root.width * 0.66))
        elide: Text.ElideMiddle
        horizontalAlignment: Text.AlignRight
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
