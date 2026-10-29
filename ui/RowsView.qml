import QtQuick
import com.gust.app

// Rows — the everyday default. Names, dates, dense directories.
FocusScope {
    id: root
    required property var model
    signal activated(int row)
    signal selectionTouched()
    // Drag and drop. The view owns the gesture; Main.qml owns the policy.
    signal dragStarted(int row)
    signal dropMovedOver(int row)
    /// Pointer position during a drag, in this view's coordinates: lets the
    /// pane hand the drag to other apps once it leaves the window.
    signal dragMovedTo(real x, real y)
    /// Row under a point in this view's coordinates, or -1.
    function rowAt(x, y) {
        const p = mapToItem(list.contentItem, x, y)
        return list.indexAt(p.x, p.y)
    }
    signal dropReleased(int row)
    property bool dragActive: false
    /// Keyboard modifiers held during the drag: Ctrl copies, Shift moves.
    property int dragMods: 0
    property int dropRow: -1
    /// A drag from another window or application is over this view; the
    /// ring then shows on `dropRow` without an in-view drag (Pane sets it).
    property bool platformDrop: false
    /// A drag made with the right button ends in a Copy/Move/Link menu.
    property bool dragRight: false
    /// The click that ends a right-button drag must not also open the menu.
    property bool suppressClick: false
    signal dropMenuRequested(int row, real x, real y)
    // Pixels the pointer must travel before a click becomes a drag.
    readonly property int dragThreshold: 8
    signal contextRequested(int row, real x, real y)
    /// Middle-click on a row: open it in a new tab (Main decides).
    signal middleClicked(int row)

    readonly property int iconSize: Math.round(Tokens.rowHeight * 0.7)
    readonly property int emblemSize: Math.round(Tokens.rowHeight * 0.6)


    /// How far the list is scrolled, in pixels (qa).
    function scrollTop() { return Math.round(list.contentY - list.originY) }
    /// What each of the first rows shows for an icon (qa).
    function iconInfo() {
        const out = []
        if (!root.model) return out
        for (let i = 0; i < Math.min(list.count, 40); i++) {
            const it = list.itemAtIndex(i)
            const k = root.model.entry_icon_key(i)
            out.push({ name: root.model.entry_name(i), key: k,
                       emblems: root.model.entry_emblems(i),
                       look: Tokens.iconLook(k),
                       shown: it ? it.iconShown : null })
        }
        return out
    }

    function ensureCursorVisible() { list.positionViewAtIndex(model.cursor, ListView.Contain) }

    // Tell the model which rows are on screen, so details (size, date) are
    // read for what the user can see rather than for the whole folder.
    // Debounced: a fling reports once it slows, not on every frame.
    function reportRows() {
        if (!root.visible || !root.model || list.count === 0) return
        // Arithmetic, not indexAt(): at the very bottom indexAt() can miss
        // and answer -1, which read as "row 0" and fetched nothing.
        const first = Math.max(0, Math.floor((list.contentY - list.originY) / Tokens.rowHeight))
        root.model.want_rows(first, Math.ceil(list.height / Tokens.rowHeight) + 1)
    }
    Timer { id: rowsReport; interval: 40; onTriggered: root.reportRows() }
    onVisibleChanged: if (visible) rowsReport.restart()

    /// Where a keyboard-opened menu should appear: under the cursor row,
    /// in window coordinates.
    /// Centre of the Name column header in window coordinates (qa clicks it).
    function nameHeaderPoint() {
        return header.mapToItem(null, Tokens.rowPaddingX * 3, header.height / 2)
    }
    /// The four header labels as drawn, for the qa probe (sort arrows).
    function headerTexts() {
        return [nameHeaderText.text, kindHeaderText.text, sizeHeaderText.text, modHeaderText.text]
    }

    /// Centre of each visible row by name, in window coordinates (qa).
    function rowPoints() {
        const out = {}
        if (!root.model) return out
        for (let i = 0; i < Math.min(list.count, 80); i++) {
            const it = list.itemAtIndex(i)
            if (!it) continue
            const p = it.mapToItem(null, Tokens.rowPaddingX * 4, it.height / 2)
            if (p.y > 0) out[root.model.entry_name(i)] = [Math.round(p.x), Math.round(p.y)]
        }
        return out
    }

    function cursorAnchor() {
        const it = list.itemAtIndex(root.model ? root.model.cursor : -1)
        if (it) return it.mapToItem(null, Tokens.rowPaddingX * 3, it.height)
        return list.mapToItem(null, Tokens.rowPaddingX, 0)
    }

    Rectangle {
        anchors.fill: parent
        color: Tokens.background

        // Right-click on empty space: the folder's own menu. Declared first
        // so the rows, which sit above it, keep their clicks.
        MouseArea {
            anchors.fill: parent
            acceptedButtons: Qt.RightButton
            onClicked: (mouse) => {
                const g = mapToItem(null, mouse.x, mouse.y)
                root.contextRequested(-1, g.x, g.y)
            }
        }

        Column {
            anchors.fill: parent

            // Column headers double as sort controls.
            Rectangle {
                id: header
                width: parent.width
                height: Tokens.controlHeight
                color: Tokens.backgroundDark

                Rectangle {
                    anchors.bottom: parent.bottom
                    width: parent.width
                    height: Tokens.borderWidth
                    color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                }

                Row {
                    anchors.fill: parent
                    anchors.leftMargin: Tokens.rowPaddingX
                    anchors.rightMargin: Tokens.rowPaddingX

                    Item {
                        width: parent.width - kindCol.width - sizeCol.width - modCol.width
                        height: parent.height
                        Text {
                            textFormat: Text.PlainText
                            anchors.verticalCenter: parent.verticalCenter
                            id: nameHeaderText
                            text: "Name" + ((root.model ? root.model.sort_field : "") === "name"
                                            ? ((root.model ? root.model.sort_ascending : true) ? " ▲" : " ▼") : "")
                            color: (root.model ? root.model.sort_field : "") === "name" ? Tokens.foregroundBright : Tokens.foregroundDim
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                        }
                        MouseArea {
                            anchors.fill: parent
                            onClicked: root.model.set_sort("name")
                            cursorShape: Qt.PointingHandCursor
                        }
                        Accessible.role: Accessible.Button
                        Accessible.name: "Sort by name"
                    }
                    Item {
                        id: kindCol
                        width: Math.round(list.width * 0.10)
                        height: parent.height
                        Text {
                            textFormat: Text.PlainText
                            anchors.left: parent.left
                            anchors.verticalCenter: parent.verticalCenter
                            id: kindHeaderText
                            text: "Kind" + ((root.model ? root.model.sort_field : "") === "kind"
                                            ? ((root.model ? root.model.sort_ascending : true) ? " ▲" : " ▼") : "")
                            color: (root.model ? root.model.sort_field : "") === "kind" ? Tokens.foregroundBright : Tokens.foregroundDim
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                        }
                        MouseArea {
                            anchors.fill: parent
                            onClicked: root.model.set_sort("kind")
                            cursorShape: Qt.PointingHandCursor
                        }
                        Accessible.role: Accessible.Button
                        Accessible.name: "Sort by kind"
                    }
                    Item {
                        id: sizeCol
                        width: Math.round(list.width * 0.16)
                        height: parent.height
                        Text {
                            textFormat: Text.PlainText
                            anchors.right: parent.right
                            anchors.rightMargin: Tokens.spaceLg
                            anchors.verticalCenter: parent.verticalCenter
                            id: sizeHeaderText
                            text: "Size" + ((root.model ? root.model.sort_field : "") === "size"
                                            ? ((root.model ? root.model.sort_ascending : true) ? " ▲" : " ▼") : "")
                            color: (root.model ? root.model.sort_field : "") === "size" ? Tokens.foregroundBright : Tokens.foregroundDim
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                        }
                        MouseArea {
                            anchors.fill: parent
                            onClicked: root.model.set_sort("size")
                            cursorShape: Qt.PointingHandCursor
                        }
                        Accessible.role: Accessible.Button
                        Accessible.name: "Sort by size"
                    }
                    Item {
                        id: modCol
                        width: Math.round(list.width * 0.22)
                        height: parent.height
                        Text {
                            textFormat: Text.PlainText
                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            id: modHeaderText
                            text: "Modified" + ((root.model ? root.model.sort_field : "") === "modified"
                                            ? ((root.model ? root.model.sort_ascending : true) ? " ▲" : " ▼") : "")
                            color: (root.model ? root.model.sort_field : "") === "modified" ? Tokens.foregroundBright : Tokens.foregroundDim
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                        }
                        MouseArea {
                            anchors.fill: parent
                            onClicked: root.model.set_sort("modified")
                            cursorShape: Qt.PointingHandCursor
                        }
                        Accessible.role: Accessible.Button
                        Accessible.name: "Sort by modified date"
                    }
                }
            }

            ListView {
                id: list
                width: parent.width
                height: parent.height - header.height
                model: root.visible ? root.model : null
                clip: true
                focus: true
                currentIndex: model ? root.model.cursor : -1
                // Keep the keyboard cursor visible without animating the view
                // under the user (reduced-motion friendly).
                highlightMoveDuration: 0
                // Recycle delegates: without this a six-figure directory
                // creates a QQuickItem per row and the GUI thread never
                // catches up with its own event queue.
                reuseItems: true
                // Only realise a screenful plus a little, never the whole
                // model, whatever its size.
                cacheBuffer: Math.round(Tokens.rowHeight * 12)
                boundsBehavior: Flickable.StopAtBounds
                // Keys belong to the window's handler; a view that took
                // focus must not quietly consume them (DESIGN §6.22).
                keyNavigationEnabled: false
                objectName: "rowsList"
                WheelScroll { view: list }
                onContentYChanged: rowsReport.restart()
                onHeightChanged: rowsReport.restart()
                onCountChanged: rowsReport.restart()
                onCurrentIndexChanged: rowsReport.restart()

                delegate: Rectangle {
                    id: rowItem
                    property bool iconShown: rowIcon.visible
                    width: list.width
                    height: Tokens.rowHeight
                    // Three distinct states: selected (band), cursor (bar),
                    // hover. Selection and cursor are different things and
                    // must be separately visible.
                    color: isSelected
                           ? Tokens.withAlpha(Tokens.accent, 0.22)
                           : (rowItem.ListView.isCurrentItem
                              ? Tokens.withAlpha(Tokens.selectedBackground, Math.max(Tokens.selectedAlpha, 0.14))
                              : (hover.hovered ? Tokens.withAlpha(Tokens.foreground, 0.05) : "transparent"))

                    // Focus ring: visible keyboard focus, not colour alone.
                    Rectangle {
                        visible: rowItem.ListView.isCurrentItem
                        anchors.left: parent.left
                        width: Math.max(2, Tokens.borderWidth * 2)
                        height: parent.height
                        color: Tokens.accent
                    }

                    HoverHandler { id: hover }

                    Row {
                        anchors.fill: parent
                        anchors.leftMargin: Tokens.rowPaddingX
                        anchors.rightMargin: Tokens.rowPaddingX
                        spacing: 0

                        Item {
                            width: parent.width - kindText.width - sizeText.width - modText.width
                            height: parent.height
                            Row {
                                anchors.verticalCenter: parent.verticalCenter
                                spacing: Tokens.spaceLg
                                // The drawn file icon (DESIGN §6.21).
                                FileIcon {
                                    id: rowIcon
                                    size: root.iconSize
                                    anchors.verticalCenter: parent.verticalCenter
                                    look: Tokens.iconLook(iconKey)
                                    opacity: clipMark === "cut" || isHidden ? 0.55 : 1
                                }
                                Text {
                                    textFormat: Text.PlainText
                                    text: name
                                    // A cut row is dimmed: it is about to leave.
                                    color: clipMark === "cut" || isHidden ? Tokens.foregroundDim
                                                    : (isDir ? Tokens.foregroundBright : Tokens.foreground)
                                    font.family: Tokens.fontFamily
                                    font.pixelSize: Tokens.fontBody
                                    elide: Text.ElideMiddle
                                    width: Math.min(implicitWidth, rowItem.width * 0.55)
                                }
                                // Colour tags (DESIGN §6.25).
                                TagDots {
                                    anchors.verticalCenter: parent.verticalCenter
                                    keys: tagKeys
                                }
                                // Read-only, no access, mount point, link.
                                Row {
                                    anchors.verticalCenter: parent.verticalCenter
                                    spacing: Tokens.spaceXs
                                    visible: emblems !== ""
                                    Repeater {
                                        model: emblems === "" ? [] : emblems.split(",")
                                        delegate: Emblem {
                                            required property string modelData
                                            kind: modelData
                                            size: root.emblemSize
                                            framed: false
                                        }
                                    }
                                }
                                // Copied / cut, after the name (Flea #169).
                                Text {
                                    textFormat: Text.PlainText
                                    visible: clipMark !== ""
                                    anchors.verticalCenter: parent.verticalCenter
                                    text: clipMark === "cut" ? qsTr("cut") : qsTr("copied")
                                    color: clipMark === "cut" ? Tokens.warn : Tokens.accent
                                    font.family: Tokens.fontFamily
                                    font.pixelSize: Tokens.fontCaption
                                }
                                Text {
                                    textFormat: Text.PlainText
                                    visible: isSymlink && !linkOk
                                    text: "(broken link)"
                                    color: Tokens.danger
                                    font.family: Tokens.fontFamily
                                    font.pixelSize: Tokens.fontCaption
                                }
                            }
                        }
                        Text {
                            textFormat: Text.PlainText
                            id: kindText
                            width: Math.round(list.width * 0.10)
                            anchors.verticalCenter: parent.verticalCenter
                            text: typeText
                            color: Tokens.foregroundDim
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                            elide: Text.ElideRight
                        }
                        Text {
                            textFormat: Text.PlainText
                            id: sizeText
                            width: Math.round(list.width * 0.16)
                            horizontalAlignment: Text.AlignRight
                            rightPadding: Tokens.spaceLg
                            anchors.verticalCenter: parent.verticalCenter
                            text: sizeText_
                            property string sizeText_: model.sizeText
                            color: Tokens.foregroundDim
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                        }
                        Text {
                            textFormat: Text.PlainText
                            id: modText
                            width: Math.round(list.width * 0.22)
                            horizontalAlignment: Text.AlignRight
                            anchors.verticalCenter: parent.verticalCenter
                            text: modifiedText
                            color: Tokens.foregroundDim
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                        }
                    }

                    // A drop target: any row can receive, but only a folder
                    // keeps the items — a drop onto a file goes to its
                    // containing folder (DESIGN.md §6.1), which the Rust
                    // planner decides, not this delegate.
                    Rectangle {
                        id: dropRing
                        anchors.fill: parent
                        color: "transparent"
                        border.width: visible ? Math.max(2, Tokens.borderWidth * 2) : 0
                        border.color: Tokens.accent
                        visible: root.dropRow === index && (root.dragActive || root.platformDrop)
                        z: 5
                    }

                    MouseArea {
                        id: rowMouse
                        anchors.fill: parent
                        acceptedButtons: Qt.LeftButton | Qt.RightButton | Qt.MiddleButton
                        // Without this, onPositionChanged does not fire while
                        // a button is held, so a drag is never detected.
                        preventStealing: true
                        // A drag starts only after the pointer has actually
                        // moved: a click must never become a file operation.
                        property point pressPos: Qt.point(0, 0)
                        property bool armed: false

                        onPressed: (mouse) => {
                            pressPos = Qt.point(mouse.x, mouse.y)
                            armed = (mouse.button === Qt.LeftButton || mouse.button === Qt.RightButton)
                            root.dragRight = (mouse.button === Qt.RightButton)
                            root.suppressClick = false
                        }
                        onReleased: (mouse) => {
                            if (root.dragActive && root.dragRight) {
                                const g = mapToItem(null, mouse.x, mouse.y)
                                root.suppressClick = true
                                root.dropMenuRequested(root.dropRow, g.x, g.y)
                            } else if (root.dragActive) {
                                root.dragMods = mouse.modifiers
                                root.dropReleased(root.dropRow)
                            }
                            armed = false
                            root.dragActive = false
                            root.dropRow = -1
                        }
                        onPositionChanged: (mouse) => {
                            if (!armed)
                                return
                            if (!root.dragActive) {
                                const dx = mouse.x - pressPos.x
                                const dy = mouse.y - pressPos.y
                                if (dx * dx + dy * dy < root.dragThreshold * root.dragThreshold)
                                    return
                                root.dragStarted(index)
                            }
                            // Which row is under the pointer now?
                            const gp = mapToItem(list.contentItem, mouse.x, mouse.y)
                            root.dropRow = list.indexAt(gp.x, gp.y)
                            root.dragMods = mouse.modifiers
                            root.dropMovedOver(root.dropRow)
                            const vp = mapToItem(root, mouse.x, mouse.y)
                            root.dragMovedTo(vp.x, vp.y)
                        }
                        onClicked: (mouse) => {
                            if (root.suppressClick) { root.suppressClick = false; return }
                            if (mouse.button === Qt.MiddleButton) { root.middleClicked(index); return }
                            if (mouse.button === Qt.RightButton) {
                                // Right-clicking outside the selection acts on
                                // the clicked row instead of a stale set.
                                if (!root.model.sel_contains(index))
                                    root.model.sel_only(index)
                                root.model.focus_row(index)
                                const g = mapToItem(null, mouse.x, mouse.y)
                                root.contextRequested(index, g.x, g.y)
                                return
                            }
                            if (mouse.modifiers & Qt.ControlModifier)
                                root.model.sel_toggle(index)
                            else if (mouse.modifiers & Qt.ShiftModifier)
                                root.model.sel_range_to(index)
                            else {
                                root.model.sel_clear()
                                root.model.focus_row(index)
                            }
                            root.selectionTouched()
                        }
                        onDoubleClicked: root.activated(index)
                    }

                    Accessible.role: Accessible.ListItem
                    Accessible.name: name + (isDir ? ", folder" : ", file")
                    Accessible.selectable: true
                    Accessible.selected: isSelected || rowItem.ListView.isCurrentItem
                }

                // Empty state — never a blank void.
                Text {
                    textFormat: Text.PlainText
                    anchors.centerIn: parent
                    visible: list.count === 0
                    text: (root.model && root.model.empty_reason !== "") ? root.model.empty_reason
                                  : (root.model ? root.model.filter : "").length > 0 ? "No match for \"" + (root.model ? root.model.filter : "") + "\""
                                                       : "This folder is empty"
                    color: Tokens.foregroundDim
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                }
            }
        }

        // Sibling of the ListView, not a child: a child would scroll with the
        // content and be clipped.
        ScrollIndicator { target: list }

        // Drag on empty space draws a selection band (Nautilus, Dolphin).
        RubberBand {
            id: rowsBand
            x: list.x; y: header.height
            width: list.width; height: list.height
            view: list
            model: root.model
            cellWidth: list.width
            cellHeight: Tokens.rowHeight
            columns: 1
            onBandChanged: root.selectionTouched()
        }
    }

    Accessible.role: Accessible.List
    Accessible.name: "File list, rows view"
}
