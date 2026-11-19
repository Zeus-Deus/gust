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
    /// Once per menu, after it is over: the action picked, after chosen()
    /// has run it, or "" when the menu was dismissed. closed() comes BEFORE
    /// chosen() (so an action may reopen the menu as a submenu); cleanup that
    /// must follow the action, such as releasing a right-drag, belongs here.
    signal finished(string action)
    signal highlightMoved()
    onCurrentChanged: {
        if (!visible) return
        reveal(current)
        highlightMoved()
    }

    /// Scroll so entry `i` is inside the frame (keyboard travel in a menu
    /// taller than the window).
    function reveal(i) {
        if (i < 0 || i >= items.length || !menu.scrolls) return
        let top = padY
        for (let k = 0; k < i; k++) top += items[k].separator ? sepH : rowH
        const bottom = top + rowH
        if (top - padY < scroller.contentY) scroller.contentY = top - padY
        else if (bottom + padY > scroller.contentY + scroller.height)
            scroller.contentY = bottom + padY - scroller.height
    }
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
    /// Where the last menu was opened (window coordinates), so a follow-on
    /// menu (Tag ▸ colours) opens in the same place.
    property point lastAt: Qt.point(0, 0)

    function popup(px, py, list) {
        lastAt = Qt.point(px, py)
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
        menu.preferredWidth = Math.ceil(Tokens.rowPaddingX * 2 + wl + (wh > 0 ? Tokens.spaceXxl * 2 + wh : 0))
        const m = Tokens.spaceSm
        // Never taller than the window: a long menu in a short (tiled) or
        // zoomed window used to run off the bottom, taking its last
        // entries (Move to Trash) out of reach. It scrolls instead.
        menu.fullHeight = Math.ceil(h)
        menu.height = Math.min(menu.fullHeight, Math.max(rowH + padY * 2, root.height - m * 2))
        scroller.contentY = 0
        menu.y = py + menu.height + m <= root.height
                 ? py
                 : Math.max(m, py - menu.height)
        current = -1
        pointerSeen = false
        visible = true
        keys.forceActiveFocus()
    }

    function horizontalEdges() { return [menu.x, menu.x + menu.width] }

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

    property bool picking: false
    /// True while a picked entry is being carried out: from before the menu
    /// closes until its action and finished() have run. The window keeps rows
    /// still through it, so the action resolves the rows the menu was opened
    /// on, not a re-sort or re-filter that was waiting for the menu to go.
    property bool dispatching: false
    function close() {
        if (!visible) return
        visible = false
        items = []
        closed()
        if (!picking) finished("")
    }

    function pick(i) {
        if (!isPickable(i)) return
        const a = items[i].action
        dispatching = true
        picking = true
        close()
        picking = false
        chosen(a)
        finished(a)
        dispatching = false
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
        property real preferredWidth: 0
        width: Math.min(preferredWidth, Math.max(0, root.width - 2 * Tokens.spaceSm))
        x: Math.max(Tokens.spaceSm, Math.min(root.lastAt.x, root.width - width - Tokens.spaceSm))
        color: Tokens.menuBackground
        border.width: Math.max(1, Tokens.borderWidth)
        border.color: Tokens.border
        radius: Tokens.radius
        /// The height every entry needs; `height` is capped to the window.
        property real fullHeight: 0
        readonly property bool scrolls: fullHeight > height + 1

        // Clicks inside the frame but between rows must not fall through
        // to the dismiss area.
        MouseArea {
            anchors.fill: parent
            acceptedButtons: Qt.AllButtons
            // At a scroll limit the Flickable passes on further wheel ticks.
            onWheel: (wheel) => { wheel.accepted = true }
        }

        Flickable {
            id: scroller
            objectName: "contextMenuScroller"
            anchors.fill: parent
            anchors.topMargin: Math.max(1, Tokens.borderWidth)
            anchors.bottomMargin: Math.max(1, Tokens.borderWidth)
            contentWidth: width
            contentHeight: menu.fullHeight - Math.max(1, Tokens.borderWidth) * 2
            interactive: menu.scrolls
            boundsBehavior: Flickable.StopAtBounds
            clip: menu.scrolls

        Column {
            y: root.padY - Math.max(1, Tokens.borderWidth)
            width: parent.width

            Repeater {
                id: menuRepeater
                model: root.items
                delegate: Item {
                    id: entry
                    required property var modelData
                    required property int index
                    width: menu.width
                    height: modelData.separator ? root.sepH : root.rowH
                    readonly property real textBudget: Math.max(0, width - 2 * Tokens.rowPaddingX
                                                                - (shortcut.text ? Tokens.spaceSm : 0))

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
                        id: entryLabel
                        objectName: "menuLabel"
                        textFormat: Text.PlainText
                        visible: !modelData.separator
                        anchors.verticalCenter: parent.verticalCenter
                        x: Tokens.rowPaddingX
                        width: entry.textBudget - shortcut.width
                        elide: Text.ElideRight
                        text: modelData.label || ""
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                        color: modelData.enabled === false ? Tokens.foregroundDim
                             : modelData.danger ? Tokens.danger
                             : index === root.current ? Tokens.foregroundBright
                             : Tokens.foreground
                    }
                    Text {
                        id: shortcut
                        objectName: "menuShortcut"
                        textFormat: Text.PlainText
                        visible: !modelData.separator && (modelData.hint || "") !== ""
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.right: parent.right
                        anchors.rightMargin: Tokens.rowPaddingX
                        // Share a crowded row. A short action or shortcut leaves
                        // its unused space to the other column.
                        width: Math.min(implicitWidth, Math.max(entry.textBudget / 2,
                                                               entry.textBudget - entryLabel.implicitWidth))
                        horizontalAlignment: Text.AlignRight
                        elide: Text.ElideRight
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

        // More above or below: a thin bar at the edge that has it.
        Rectangle {
            visible: menu.scrolls && !scroller.atYBeginning
            anchors.top: parent.top
            x: Tokens.spaceSm
            width: parent.width - Tokens.spaceSm * 2
            height: Math.max(2, Tokens.borderWidth * 2)
            color: Tokens.accent
        }
        Rectangle {
            visible: menu.scrolls && !scroller.atYEnd
            anchors.bottom: parent.bottom
            x: Tokens.spaceSm
            width: parent.width - Tokens.spaceSm * 2
            height: Math.max(2, Tokens.borderWidth * 2)
            color: Tokens.accent
        }
    }
}
