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
    signal dropReleased(int row)
    property bool dragActive: false
    property int dropRow: -1
    // Pixels the pointer must travel before a click becomes a drag.
    readonly property int dragThreshold: 8
    signal contextRequested(int row, real x, real y)

    function ensureCursorVisible() { list.positionViewAtIndex(model.cursor, ListView.Contain) }

    Rectangle {
        anchors.fill: parent
        color: Tokens.background

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
                        width: parent.width - sizeCol.width - modCol.width
                        height: parent.height
                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            text: "Name" + ((root.model ? root.model.sort_field : "") === "name"
                                            ? ((root.model ? root.model.sort_ascending : true) ? " ↓" : " ↑") : "")
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
                        id: sizeCol
                        width: Math.round(parent.width * 0.16)
                        height: parent.height
                        Text {
                            anchors.right: parent.right
                            anchors.rightMargin: Tokens.spaceLg
                            anchors.verticalCenter: parent.verticalCenter
                            text: "Size" + ((root.model ? root.model.sort_field : "") === "size"
                                            ? ((root.model ? root.model.sort_ascending : true) ? " ↓" : " ↑") : "")
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
                        width: Math.round(parent.width * 0.22)
                        height: parent.height
                        Text {
                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            text: "Modified" + ((root.model ? root.model.sort_field : "") === "modified"
                                            ? ((root.model ? root.model.sort_ascending : true) ? " ↓" : " ↑") : "")
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
                model: root.model
                clip: true
                focus: true
                currentIndex: (root.model ? root.model.cursor : 0)
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
                objectName: "rowsList"

                delegate: Rectangle {
                    id: rowItem
                    width: list.width
                    height: Tokens.rowHeight
                    // Three distinct states: selected (band), cursor (bar),
                    // hover. Selection and cursor are different things and
                    // must be separately visible.
                    color: isSelected
                           ? Tokens.withAlpha(Tokens.accent, 0.22)
                           : (index === (root.model ? root.model.cursor : 0)
                              ? Tokens.withAlpha(Tokens.selectedBackground, Math.max(Tokens.selectedAlpha, 0.14))
                              : (hover.hovered ? Tokens.withAlpha(Tokens.foreground, 0.05) : "transparent"))

                    // Focus ring: visible keyboard focus, not colour alone.
                    Rectangle {
                        visible: index === (root.model ? root.model.cursor : 0)
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
                            width: parent.width - sizeText.width - modText.width
                            height: parent.height
                            Row {
                                anchors.verticalCenter: parent.verticalCenter
                                spacing: Tokens.spaceLg
                                Text {
                                    // Type glyph, with a distinct symlink badge.
                                    text: isDir ? "▸" : (isSymlink ? "↳" : "·")
                                    color: isDir ? Tokens.accent
                                                 : (isSymlink ? (linkOk ? Tokens.warn : Tokens.danger)
                                                              : Tokens.foregroundDim)
                                    font.family: Tokens.fontFamily
                                    font.pixelSize: Tokens.fontBody
                                    width: Tokens.fontBody
                                }
                                Text {
                                    text: name
                                    color: isHidden ? Tokens.foregroundDim
                                                    : (isDir ? Tokens.foregroundBright : Tokens.foreground)
                                    font.family: Tokens.fontFamily
                                    font.pixelSize: Tokens.fontBody
                                    elide: Text.ElideMiddle
                                    width: Math.min(implicitWidth, rowItem.width * 0.55)
                                }
                                Text {
                                    visible: isSymlink && !linkOk
                                    text: "(broken link)"
                                    color: Tokens.danger
                                    font.family: Tokens.fontFamily
                                    font.pixelSize: Tokens.fontCaption
                                }
                            }
                        }
                        Text {
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
                        visible: root.dropRow === index && root.dragActive
                        z: 5
                    }

                    MouseArea {
                        id: rowMouse
                        anchors.fill: parent
                        acceptedButtons: Qt.LeftButton | Qt.RightButton
                        // Without this, onPositionChanged does not fire while
                        // a button is held, so a drag is never detected.
                        preventStealing: true
                        // A drag starts only after the pointer has actually
                        // moved: a click must never become a file operation.
                        property point pressPos: Qt.point(0, 0)
                        property bool armed: false

                        onPressed: (mouse) => {
                            pressPos = Qt.point(mouse.x, mouse.y)
                            armed = (mouse.button === Qt.LeftButton)
                        }
                        onReleased: {
                            if (root.dragActive)
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
                            // Which row is under the pointer now?
                            const gp = mapToItem(list.contentItem, mouse.x, mouse.y)
                            root.dropRow = list.indexAt(gp.x, gp.y)
                            root.dropMovedOver(root.dropRow)
                        }
                        onClicked: (mouse) => {
                            if (mouse.button === Qt.RightButton) {
                                // Right-clicking outside the selection acts on
                                // the clicked row instead of a stale set.
                                if (!root.model.sel_contains(index))
                                    root.model.sel_only(index)
                                root.model.focus_row(index)
                                root.contextRequested(index, mouse.x, mouse.y)
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
                    Accessible.selected: isSelected || index === (root.model ? root.model.cursor : 0)
                }

                // Empty state — never a blank void.
                Text {
                    anchors.centerIn: parent
                    visible: list.count === 0
                    text: (root.model ? root.model.filter : "").length > 0 ? "No match for \"" + (root.model ? root.model.filter : "") + "\""
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
    }

    Accessible.role: Accessible.List
    Accessible.name: "File list, rows view"
}
