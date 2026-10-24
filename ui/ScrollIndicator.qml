import QtQuick
import com.gust.app

// Minimal scroll indicator in plain QtQuick.
//
// Deliberately NOT QtQuick.Controls.ScrollBar: Controls brings a style engine
// whose own colours would compete with Gust's theme tokens, and it is one more
// QML module on the eager startup path. Everything here is token-driven.
//
// Positioning is done with explicit geometry bindings rather than anchors:
// the indicator is a sibling of the view it tracks, and anchoring to a
// non-parent/non-sibling silently fails at runtime.
Item {
    id: root
    required property Flickable target
    property real thickness: Math.max(3, Math.round(4 * Tokens.spacingScale))

    width: thickness
    height: target ? target.height : 0
    x: target ? target.x + target.width - width : 0
    y: target ? target.y : 0

    // Only meaningful when there is something to scroll.
    visible: target && target.contentHeight > target.height

    Rectangle {
        id: track
        anchors.fill: parent
        color: Tokens.withAlpha(Tokens.foreground, 0.05)
    }

    Rectangle {
        id: handle
        width: parent.width
        color: hover.hovered || dragging
               ? Tokens.withAlpha(Tokens.accent, 0.8)
               : Tokens.withAlpha(Tokens.foreground, 0.28)

        property bool dragging: drag.drag.active

        height: {
            if (!root.target || root.target.contentHeight <= 0) return 0
            const ratio = root.target.height / root.target.contentHeight
            return Math.max(Tokens.rowHeight, root.height * Math.min(1, ratio))
        }
        y: {
            if (!root.target || root.target.contentHeight <= root.target.height) return 0
            const maxContentY = root.target.contentHeight - root.target.height
            const progress = Math.max(0, Math.min(1, root.target.contentY / maxContentY))
            return progress * (root.height - height)
        }

        HoverHandler { id: hover }

        MouseArea {
            id: drag
            anchors.fill: parent
            drag.target: handle
            drag.axis: Drag.YAxis
            drag.minimumY: 0
            drag.maximumY: Math.max(0, root.height - handle.height)
            onPositionChanged: {
                if (!drag.drag.active || !root.target) return
                const span = root.height - handle.height
                if (span <= 0) return
                const progress = handle.y / span
                root.target.contentY = progress * (root.target.contentHeight - root.target.height)
            }
        }
    }

    Accessible.role: Accessible.ScrollBar
    Accessible.name: "Scroll position"
}
