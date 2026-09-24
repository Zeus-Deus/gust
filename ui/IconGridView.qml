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
    signal dropReleased(int row)
    property bool dragActive: false
    property int dropRow: -1
    signal contextRequested(int row, real x, real y)

    function cursorAnchor() {
        const it = grid.itemAtIndex(root.model ? root.model.cursor : -1)
        if (it) return it.mapToItem(null, it.width / 2, it.height)
        return grid.mapToItem(null, 0, 0)
    }

    function zoomIn() { tileSize = Math.min(tileSize + 16, 240) }
    function zoomOut() { tileSize = Math.max(tileSize - 16, 48) }
    // Columns per row, so Up/Down move a full row rather than one tile.
    function columns() { return Math.max(1, Math.floor(grid.width / grid.cellWidth)) }

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

            // Fixed cell geometry for every tile. Deriving the label height
            // from the font rather than leaving it implicit keeps rows aligned
            // — a taller selected tile used to push its row out of line.
            readonly property real labelHeight: Math.round(Tokens.fontSmall * 2.6)
            cellWidth: root.tileSize + Tokens.spaceXl
            cellHeight: root.tileSize + labelHeight + Tokens.spaceLg

            delegate: Item {
                id: cell
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
                        color: Tokens.withAlpha(Tokens.foreground, 0.06)
                        border.width: Tokens.borderWidth
                        border.color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                        radius: Tokens.radius

                        // The glyph is what a tile shows until a thumbnail
                        // exists, and what it keeps forever for anything we
                        // will not preview. It is never replaced by a broken
                        // image icon.
                        Text {
                            anchors.centerIn: parent
                            visible: thumb.status !== Image.Ready
                            text: isDir ? "▣" : (isSymlink ? "↳" : "▢")
                            color: isDir ? Tokens.accent
                                         : (isSymlink ? (linkOk ? Tokens.warn : Tokens.danger)
                                                      : Tokens.foregroundDim)
                            font.family: Tokens.fontFamily
                            font.pixelSize: Math.round(root.tileSize * 0.32)
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
                        text: name
                        color: isHidden ? Tokens.foregroundDim
                                        : (isDir ? Tokens.foregroundBright : Tokens.foreground)
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSmall
                        elide: Text.ElideMiddle
                        maximumLineCount: 2
                        wrapMode: Text.Wrap
                    }

                    MouseArea {
                        anchors.fill: parent
                        acceptedButtons: Qt.LeftButton | Qt.RightButton
                        onClicked: (mouse) => {
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
                text: (root.model ? root.model.filter : "").length > 0 ? "No match for \"" + (root.model ? root.model.filter : "") + "\""
                                                   : "This folder is empty"
                color: Tokens.foregroundDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontBody
            }
        }

        // Sibling of the GridView so it does not scroll with the content.
        ScrollIndicator { target: grid }
    }

    Accessible.role: Accessible.List
    Accessible.name: "File list, grid view"
}
