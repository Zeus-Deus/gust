import QtQuick
import com.gust.app

// A toolbar button: one icon, a name for screen readers, and that same name
// (with its key) shown beside the pointer after a short rest on it, so an
// icon never has to be guessed at.
Rectangle {
    id: root
    /// A Glyph name.
    property string icon: ""
    /// What the button does, in words: its accessible name and its tip.
    property string label: ""
    /// The key that does the same, shown in the tip ("" for none).
    property string keys: ""
    /// Chosen state (the current view, an open search).
    property bool active: false
    property bool available: true
    signal clicked()

    width: Tokens.controlHeight
    height: Tokens.controlHeight
    radius: Tokens.radius
    color: root.active ? Tokens.withAlpha(Tokens.accent, 0.22)
         : hover.hovered && root.available ? Tokens.withAlpha(Tokens.foreground, 0.1)
         : "transparent"
    border.width: root.active ? Math.max(1, Tokens.borderWidth) : 0
    border.color: Tokens.accent

    HoverHandler {
        id: hover
        onHoveredChanged: {
            if (hovered && root.label !== "") tipDelay.restart()
            else { tipDelay.stop(); tip.visible = false }
        }
    }

    Glyph {
        anchors.centerIn: parent
        name: root.icon
        color: !root.available ? Tokens.withAlpha(Tokens.foreground, 0.3)
             : root.active ? Tokens.foregroundBright : Tokens.foreground
    }

    MouseArea {
        anchors.fill: parent
        cursorShape: root.available ? Qt.PointingHandCursor : Qt.ArrowCursor
        onPressed: { tipDelay.stop(); tip.visible = false }
        onClicked: if (root.available) root.clicked()
    }

    Timer {
        id: tipDelay
        interval: 600
        onTriggered: tip.place()
    }

    // Drawn in the window, not in the toolbar: the file area is painted
    // after the toolbar and would cover a tip that hangs below it.
    Rectangle {
        id: tip
        visible: false
        parent: root.Window.contentItem
        z: 200
        width: tipText.implicitWidth + Tokens.spaceLg * 2
        height: tipText.implicitHeight + Tokens.spaceSm * 2
        color: Tokens.menuBackground
        border.width: Math.max(1, Tokens.borderWidth)
        border.color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
        radius: Tokens.radius

        // Under the button, pulled in from the window's edge.
        function place() {
            if (!parent) return
            const p = root.mapToItem(parent, root.width / 2, root.height)
            x = Math.max(Tokens.spaceSm,
                         Math.min(Math.round(p.x - width / 2), parent.width - width - Tokens.spaceSm))
            y = Math.round(p.y + Tokens.spaceSm)
            visible = true
        }

        Text {
            id: tipText
            textFormat: Text.PlainText
            anchors.centerIn: parent
            text: root.keys !== "" ? root.label + "  " + root.keys : root.label
            color: Tokens.foreground
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontSmall
        }
    }

    Accessible.role: Accessible.Button
    Accessible.name: root.label
    Accessible.description: root.keys
    Accessible.onPressAction: if (root.available) root.clicked()
}
