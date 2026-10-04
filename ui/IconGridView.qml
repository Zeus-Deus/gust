import QtQuick
import com.gust.app

// Grid — for images and media you recognise visually. Adjustable tile size,
// two-dimensional selection movement.
FocusScope {
    id: root
    required property var model
    property int tileSize: Math.round(96 * Tokens.spacingScale)
    signal activated(int row)
    signal selectionTouched()
    // Declared so every view exposes the same drag contract, even where the
    // gesture is not wired yet: a handler assigned to a missing signal stops
    // the whole app from starting.
    signal dragStarted(int row)
    signal dropMovedOver(int row)
    signal dragMovedTo(real x, real y)
    function rowAt(x, y) {
        const p = mapToItem(grid.contentItem, x, y)
        return grid.indexAt(p.x, p.y)
    }
    signal dropReleased(int row)
    property bool dragActive: false
    property int dropRow: -1
    /// A drag made with the right button ends in a Copy/Move/Link menu.
    property bool dragRight: false
    /// The click that ends a right-button drag must not also open the menu.
    property bool suppressClick: false
    signal dropMenuRequested(int row, real x, real y)
    readonly property int dragThreshold: 8
    signal contextRequested(int row, real x, real y)
    /// Middle-click on a row: open it in a new tab (Main decides).
    signal middleClicked(int row)

    function cursorAnchor() {
        const it = grid.itemAtIndex(root.model ? root.model.cursor : -1)
        if (it) return it.mapToItem(null, it.width / 2, it.height)
        return grid.mapToItem(null, 0, 0)
    }

    /// How far the list is scrolled, in pixels (qa).
    function scrollTop() { return Math.round(grid.contentY - grid.originY) }
    /// What each of the first rows shows for an icon (qa).
    function iconInfo() {
        const out = []
        if (!root.model) return out
        for (let i = 0; i < Math.min(grid.count, 40); i++) {
            const it = grid.itemAtIndex(i)
            const k = root.model.entry_icon_key(i)
            out.push({ name: root.model.entry_name(i), key: k,
                       emblems: root.model.entry_emblems(i),
                       look: Tokens.iconLook(k),
                       shown: it ? it.iconShown : null })
        }
        return out
    }

    function zoomIn() { tileSize = Math.min(tileSize + 16, 240) }
    function zoomOut() { tileSize = Math.max(tileSize - 16, 48) }
    function zoomReset() { tileSize = Math.round(96 * Tokens.spacingScale) }
    // Columns per row, so Up/Down move a full row rather than one tile.
    function columns() { return Math.max(1, Math.floor(grid.width / grid.cellWidth)) }

    // Rows on screen, so details are read for what is visible (RowsView).
    function reportRows() {
        if (!root.model || grid.count === 0) return
        const first = Math.max(0, Math.floor((grid.contentY - grid.originY) / grid.cellHeight) * root.columns())
        const perScreen = root.columns() * (Math.ceil(grid.height / grid.cellHeight) + 1)
        root.model.want_rows(first, perScreen)
    }
    Timer { id: gridReport; interval: 40; onTriggered: root.reportRows() }

    Rectangle {
        anchors.fill: parent
        color: Tokens.background

        MouseArea {
            anchors.fill: parent
            acceptedButtons: Qt.RightButton
            onClicked: (mouse) => {
                const g = mapToItem(null, mouse.x, mouse.y)
                root.contextRequested(-1, g.x, g.y)
            }
        }

        GridView {
            id: grid
            objectName: "iconGrid"
            anchors.fill: parent
            anchors.margins: Tokens.spaceLg
            model: root.model
            clip: true
            focus: true
            currentIndex: (root.model ? root.model.cursor : 0)
            highlightMoveDuration: 0
            // Recycle delegates so a huge folder cannot create one
            // QQuickItem per entry (see RowsView).
            reuseItems: true
            cacheBuffer: Math.round((root.tileSize + Tokens.spaceXxl) * 4)
            boundsBehavior: Flickable.StopAtBounds
            keyNavigationEnabled: false
            WheelScroll { view: grid }
            onContentYChanged: gridReport.restart()
            onHeightChanged: gridReport.restart()
            onCountChanged: gridReport.restart()
            onCurrentIndexChanged: gridReport.restart()

            // Fixed cell geometry for every tile. Deriving the label height
            // from the font rather than leaving it implicit keeps rows aligned
            // — a taller selected tile used to push its row out of line.
            readonly property real labelHeight: Math.round(Tokens.fontSmall * 2.6)
            cellWidth: root.tileSize + Tokens.spaceXl
            cellHeight: root.tileSize + labelHeight + Tokens.spaceLg

            delegate: Item {
                id: cell
                property bool iconShown: tileIcon.visible || thumb.status === Image.Ready
                width: grid.cellWidth
                height: grid.cellHeight

                Rectangle {
                    anchors.fill: parent
                    anchors.margins: Tokens.spaceXs
                    color: isSelected
                           ? Tokens.withAlpha(Tokens.accent, 0.22)
                           : (index === (root.model ? root.model.cursor : 0)
                              ? Tokens.withAlpha(Tokens.selectedBackground, Math.max(Tokens.selectedAlpha, 0.14))
                              : (hover.hovered ? Tokens.withAlpha(Tokens.foreground, 0.05) : "transparent"))
                    border.width: (isSelected || index === (root.model ? root.model.cursor : 0))
                                  ? Math.max(1, Tokens.borderWidth) : 0
                    border.color: Tokens.accent
                    radius: Tokens.radius

                    HoverHandler { id: hover }

                    // Placeholder tile: real thumbnails arrive through the
                    // sandboxed thumbnailer, never decoded in this process.
                    Rectangle {
                        id: preview
                        width: root.tileSize - Tokens.spaceLg
                        height: root.tileSize - Tokens.spaceLg
                        anchors.horizontalCenter: parent.horizontalCenter
                        anchors.top: parent.top
                        anchors.topMargin: Tokens.spaceSm
                        // A thumbnail sits in a frame; a drawn icon is bare.
                        readonly property bool bare: thumb.status !== Image.Ready
                        color: bare ? "transparent" : Tokens.withAlpha(Tokens.foreground, 0.06)
                        border.width: bare ? 0 : Tokens.borderWidth
                        border.color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                        radius: Tokens.radius

                        // The drawn icon (DESIGN §6.21) is what a tile shows
                        // until a thumbnail exists, and what it keeps forever
                        // for anything we will not preview.
                        FileIcon {
                            id: tileIcon
                            anchors.centerIn: parent
                            size: Math.round(parent.width * 0.62)
                            visible: thumb.status !== Image.Ready
                            look: Tokens.iconLook(iconKey)
                            opacity: clipMark === "cut" || isHidden ? 0.55 : 1
                        }

                        // The thumbnail itself. `thumbUrl` is empty until a
                        // SANDBOXED helper has produced and validated a PNG;
                        // this process never decodes the original file.
                        //
                        // asynchronous: true keeps even the cached PNG's
                        // decode off the UI thread, so scrolling a folder of
                        // large images cannot stutter the window.
                        Image {
                            id: thumb
                            anchors.fill: parent
                            anchors.margins: Tokens.borderWidth
                            source: thumbUrl
                            visible: status === Image.Ready
                            asynchronous: true
                            cache: true
                            fillMode: Image.PreserveAspectFit
                            // Bound the decode: a helper is capped at 256px,
                            // but sourceSize makes that explicit and stops a
                            // surprising file from allocating a huge texture.
                            sourceSize.width: 256
                            sourceSize.height: 256
                        }

                        // Read-only, no access, mount point, link: square
                        // badges on the icon's bottom-right corner.
                        Row {
                            anchors.bottom: tileIcon.visible ? tileIcon.bottom : parent.bottom
                            anchors.bottomMargin: -Tokens.spaceXs
                            anchors.right: tileIcon.visible ? tileIcon.right : parent.right
                            anchors.rightMargin: -Tokens.spaceSm
                            spacing: Tokens.spaceXs
                            visible: emblems !== ""
                            Repeater {
                                model: emblems === "" ? [] : emblems.split(",")
                                delegate: Emblem {
                                    required property string modelData
                                    kind: modelData
                                    size: Math.round(root.tileSize * 0.24)
                                }
                            }
                        }
                    }

                    Text {
                        anchors.top: preview.bottom
                        anchors.topMargin: Tokens.spaceSm
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.leftMargin: Tokens.spaceXs
                        anchors.rightMargin: Tokens.spaceXs
                        height: grid.labelHeight
                        horizontalAlignment: Text.AlignHCenter
                        verticalAlignment: Text.AlignTop
                        // Break long names after _ . - rather than mid-word.
                        text: Tokens.wrapName(name)
                        color: isHidden ? Tokens.foregroundDim
                                        : (isDir ? Tokens.foregroundBright : Tokens.foreground)
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSmall
                        elide: Text.ElideMiddle
                        maximumLineCount: 2
                        wrapMode: Text.Wrap
                    }

                    // Copied / cut, on the tile's corner (Flea #169).
                    Text {
                        visible: clipMark !== ""
                        anchors.top: parent.top
                        anchors.right: parent.right
                        anchors.topMargin: Tokens.spaceXs
                        anchors.rightMargin: Tokens.spaceSm
                        text: clipMark === "cut" ? qsTr("cut") : qsTr("copied")
                        color: clipMark === "cut" ? Tokens.warn : Tokens.accent
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontCaption
                    }

                    MouseArea {
                        anchors.fill: parent
                        acceptedButtons: Qt.LeftButton | Qt.RightButton | Qt.MiddleButton
                        // Same drag gesture as Rows: nothing happens until the
                        // pointer has really moved, so a click stays a click.
                        preventStealing: true
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
                            } else if (root.dragActive)
                                root.dropReleased(root.dropRow)
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
                            const gp = mapToItem(grid.contentItem, mouse.x, mouse.y)
                            root.dropRow = grid.indexAt(gp.x, gp.y)
                            root.dropMovedOver(root.dropRow)
                            const vp = mapToItem(root, mouse.x, mouse.y)
                            root.dragMovedTo(vp.x, vp.y)
                        }
                        onClicked: (mouse) => {
                            if (root.suppressClick) { root.suppressClick = false; return }
                            if (mouse.button === Qt.MiddleButton) { root.middleClicked(index); return }
                            if (mouse.button === Qt.RightButton) {
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
                }

                Accessible.role: Accessible.ListItem
                Accessible.name: name + (isDir ? ", folder" : ", file")
                Accessible.selected: isSelected || index === (root.model ? root.model.cursor : 0)
            }

            Text {
                anchors.centerIn: parent
                visible: grid.count === 0
                text: (root.model && root.model.empty_reason !== "") ? root.model.empty_reason
                                  : (root.model ? root.model.filter : "").length > 0 ? "No match for \"" + (root.model ? root.model.filter : "") + "\""
                                                   : "This folder is empty"
                color: Tokens.foregroundDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontBody
            }
        }

        // Sibling of the GridView so it does not scroll with the content.
        ScrollIndicator { target: grid }

        RubberBand {
            anchors.fill: grid
            view: grid
            model: root.model
            // A click or drag on empty space hands the keyboard back to the
            // window (arrow keys were dead after one before).
            onBandChanged: root.selectionTouched()
            cellWidth: grid.cellWidth
            cellHeight: grid.cellHeight
            columns: root.columns()
        }
    }

    Accessible.role: Accessible.List
    Accessible.name: "File list, grid view"
}
