import QtQuick
import com.gust.app

// Right-click menu.
//
// One flat list with no submenus: everything here is also in the palette,
// so the menu only has to put the common things one click away. Every entry
// shows its key so the menu also teaches the faster path.
//
// Opened by a right-click (on a row, or on empty space), by the Menu key, or
// by Shift+F10. Up/Down/Enter/Escape work while it is open, and a click
// anywhere else closes it without acting.
Item {
    id: root
    anchors.fill: parent
    visible: false
    z: 95

    /// [{label, hint, action, danger, enabled}] or {separator: true}
    property var items: []
    property int current: -1
    // Where the pointer was when the menu appeared; hover counts only
    // once it has moved away from there.
    property bool pointerSeen: false
    property point pointerOrigin: Qt.point(0, 0)
    readonly property bool open: visible

    signal chosen(string action)
    signal highlightMoved()
    onCurrentChanged: if (visible) highlightMoved()
    signal closed()

    readonly property real rowH: Tokens.rowHeight
    readonly property real sepH: Tokens.spaceSm * 2 + 1
    readonly property real padY: Tokens.spaceSm

    TextMetrics { id: labelMetrics; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontBody }
    TextMetrics { id: hintMetrics; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontSmall }

    function isPickable(i) {
        return i >= 0 && i < items.length && !items[i].separator && items[i].enabled !== false
    }
    function step(from, dir) {
        for (let n = 0, i = from; n < items.length; n++) {
            i = (i + dir + items.length) % items.length
            if (isPickable(i)) return i
        }
        return -1
    }

    /// Show `list` with its top-left corner at (px, py) in window
    /// coordinates, flipped or nudged so it always fits on screen.
    function popup(px, py, list) {
        items = list
        let wl = 0, wh = 0, h = padY * 2
        for (let i = 0; i < list.length; i++) {
            if (list[i].separator) { h += sepH; continue }
            h += rowH
            labelMetrics.text = list[i].label
            wl = Math.max(wl, labelMetrics.advanceWidth)
            hintMetrics.text = list[i].hint || ""
            wh = Math.max(wh, hintMetrics.advanceWidth)
        }
        menu.width = Math.ceil(Tokens.rowPaddingX * 2 + wl + (wh > 0 ? Tokens.spaceXxl * 2 + wh : 0))
        menu.height = Math.ceil(h)
        const m = Tokens.spaceSm
        menu.x = Math.max(m, Math.min(px, root.width - menu.width - m))
        menu.y = py + menu.height + m <= root.height
                 ? py
                 : Math.max(m, py - menu.height)
        current = -1
        pointerSeen = false
        visible = true
        keys.forceActiveFocus()
    }

    /// Centre of each entry by label, in window coordinates (qa).
    function itemPoints() {
        const out = {}
        for (let i = 0; i < menuRepeater.count; i++) {
            const it = menuRepeater.itemAt(i)
            if (!it || items[i].separator) continue
            const p = it.mapToItem(null, it.width / 2, it.height / 2)
            out[items[i].label] = [Math.round(p.x), Math.round(p.y)]
        }
        return out
    }

    function close() {
        if (!visible) return
        visible = false
        items = []
        closed()
    }

    function pick(i) {
        if (!isPickable(i)) return
        const a = items[i].action
        close()
        chosen(a)
    }

    // Anything outside the menu dismisses it and does nothing else: a click
    // meant to close a menu must never also select or open a file.
    MouseArea {
        anchors.fill: parent
        acceptedButtons: Qt.AllButtons
        onPressed: root.close()
        onWheel: root.close()
    }

    Item {
        id: keys
        focus: true
        Keys.onPressed: (event) => {
            if (event.key === Qt.Key_Down) { root.current = root.step(root.current, 1) }
            else if (event.key === Qt.Key_Up) {
                root.current = root.step(root.current < 0 ? 0 : root.current, -1)
            }
            else if (event.key === Qt.Key_Home) { root.current = root.step(-1, 1) }
            else if (event.key === Qt.Key_End) { root.current = root.step(0, -1) }
            else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter
                     || event.key === Qt.Key_Space) { root.pick(root.current) }
            else if (event.key === Qt.Key_Escape || event.key === Qt.Key_Menu
                     || (event.key === Qt.Key_F10 && (event.modifiers & Qt.ShiftModifier))) {
                root.close()
            }
            event.accepted = true
        }
    }

    Rectangle {
        id: menu
        objectName: "contextMenu"
        color: Tokens.menuBackground
        border.width: Math.max(1, Tokens.borderWidth)
        border.color: Tokens.border
        radius: Tokens.radius

        // Clicks inside the frame but between rows must not fall through
        // to the dismiss area.
        MouseArea { anchors.fill: parent; acceptedButtons: Qt.AllButtons }

        Column {
            y: root.padY
            width: parent.width

            Repeater {
                id: menuRepeater
                model: root.items
                delegate: Item {
                    required property var modelData
                    required property int index
                    width: menu.width
                    height: modelData.separator ? root.sepH : root.rowH

                    Rectangle {
                        visible: modelData.separator === true
                        anchors.verticalCenter: parent.verticalCenter
                        x: Tokens.spaceSm
                        width: parent.width - Tokens.spaceSm * 2
                        height: 1
                        color: Tokens.withAlpha(Tokens.border, 0.6)
                    }

                    Rectangle {
                        visible: !modelData.separator
                        anchors.fill: parent
                        anchors.leftMargin: Math.max(1, Tokens.borderWidth)
                        anchors.rightMargin: Math.max(1, Tokens.borderWidth)
                        color: index === root.current && root.isPickable(index)
                               ? Tokens.withAlpha(Tokens.selectedBackground, Math.max(Tokens.selectedAlpha, 0.16))
                               : "transparent"
                        // The current entry also gets the list's accent bar:
                        // a pale fill alone vanished on light themes.
                        Rectangle {
                            visible: index === root.current && root.isPickable(index)
                            width: Math.max(2, Tokens.borderWidth * 2)
                            height: parent.height
                            color: Tokens.accent
                        }
                    }

                    Text {
                        visible: !modelData.separator
                        anchors.verticalCenter: parent.verticalCenter
                        x: Tokens.rowPaddingX
                        text: modelData.label || ""
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                        color: modelData.enabled === false ? Tokens.foregroundDim
                             : modelData.danger ? Tokens.danger
                             : index === root.current ? Tokens.foregroundBright
                             : Tokens.foreground
                    }
                    Text {
                        visible: !modelData.separator && (modelData.hint || "") !== ""
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.right: parent.right
                        anchors.rightMargin: Tokens.rowPaddingX
                        text: modelData.hint || ""
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSmall
                        color: Tokens.foregroundDim
                    }

                    MouseArea {
                        visible: !modelData.separator
                        anchors.fill: parent
                        hoverEnabled: true
                        acceptedButtons: Qt.LeftButton | Qt.RightButton
                        // Movement, not mere presence: a menu opened by the
                        // keyboard can appear under a resting pointer, and
                        // that pointer must not pick the entry Enter runs.
                        onPositionChanged: (mouse) => {
                            const g = mapToItem(root, mouse.x, mouse.y)
                            if (!root.pointerSeen) {
                                root.pointerSeen = true
                                root.pointerOrigin = g
                                return
                            }
                            if (Math.abs(g.x - root.pointerOrigin.x) + Math.abs(g.y - root.pointerOrigin.y) < 3)
                                return
                            root.current = root.isPickable(index) ? index : -1
                        }
                        onExited: if (root.current === index) root.current = -1
                        onClicked: root.pick(index)
                    }

                    Accessible.role: modelData.separator ? Accessible.Separator : Accessible.MenuItem
                    Accessible.name: modelData.label || ""
                }
            }
        }
    }
}
