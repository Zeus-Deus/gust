import QtQuick
import com.gust.app

// Columns — parents stay visible, with a preview pane at the right.
// Selecting a file PREVIEWS it; it never launches automatically.
FocusScope {
    id: root
    required property var model
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

    Rectangle {
        anchors.fill: parent
        color: Tokens.background

        Row {
            anchors.fill: parent

            // Parent column: where we came from, kept in view.
            Rectangle {
                id: parentCol
                width: Math.round(parent.width * 0.26)
                height: parent.height
                color: Tokens.backgroundDark

                Column {
                    anchors.fill: parent
                    Text {
                        width: parent.width
                        height: Tokens.controlHeight
                        leftPadding: Tokens.rowPaddingX
                        verticalAlignment: Text.AlignVCenter
                        text: "Parent"
                        color: Tokens.foregroundDim
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontCaption
                    }
                    Text {
                        width: parent.width
                        leftPadding: Tokens.rowPaddingX
                        rightPadding: Tokens.rowPaddingX
                        text: root.model.location_display
                        color: Tokens.foreground
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSmall
                        wrapMode: Text.WrapAnywhere
                    }
                }

                Rectangle {
                    anchors.right: parent.right
                    width: Tokens.borderWidth
                    height: parent.height
                    color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                }
            }

            // Current column.
            Rectangle {
                id: currentCol
                width: Math.round(parent.width * 0.37)
                height: parent.height
                color: Tokens.background

                ListView {
                    id: list
                    objectName: "columnsList"
                    anchors.fill: parent
                    model: root.model
                    clip: true
                    focus: true
                    currentIndex: root.model.cursor
                    highlightMoveDuration: 0
                    boundsBehavior: Flickable.StopAtBounds

                    delegate: Rectangle {
                        width: list.width
                        height: Tokens.rowHeight
                        color: index === root.model.cursor
                               ? Tokens.withAlpha(Tokens.selectedBackground, Math.max(Tokens.selectedAlpha, 0.14))
                               : "transparent"

                        Row {
                            anchors.fill: parent
                            anchors.leftMargin: Tokens.rowPaddingX
                            anchors.rightMargin: Tokens.spaceLg
                            spacing: Tokens.spaceLg

                            Text {
                                anchors.verticalCenter: parent.verticalCenter
                                text: name
                                color: isDir ? Tokens.foregroundBright : Tokens.foreground
                                font.family: Tokens.fontFamily
                                font.pixelSize: Tokens.fontBody
                                elide: Text.ElideMiddle
                                width: parent.width - chevron.width - Tokens.spaceLg
                            }
                            Text {
                                id: chevron
                                anchors.verticalCenter: parent.verticalCenter
                                text: isDir ? "›" : ""
                                color: Tokens.foregroundDim
                                font.family: Tokens.fontFamily
                                font.pixelSize: Tokens.fontBody
                            }
                        }

                        MouseArea {
                            anchors.fill: parent
                            acceptedButtons: Qt.LeftButton | Qt.RightButton
                            onClicked: (mouse) => {
                                root.model.focus_row(index)
                                if (mouse.button === Qt.RightButton)
                                    root.contextRequested(index, mouse.x, mouse.y)
                            }
                            onDoubleClicked: root.activated(index)
                        }

                        Accessible.role: Accessible.ListItem
                        Accessible.name: name + (isDir ? ", folder" : ", file")
                    }
                }

                Rectangle {
                    anchors.right: parent.right
                    width: Tokens.borderWidth
                    height: parent.height
                    color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                }
            }

            // Preview pane. Metadata only — no decoding in this process.
            Rectangle {
                width: parent.width - parentCol.width - currentCol.width
                height: parent.height
                color: Tokens.backgroundDark

                Column {
                    anchors.fill: parent
                    anchors.margins: Tokens.spaceXl
                    spacing: Tokens.spaceLg

                    Text {
                        width: parent.width
                        text: root.model.count > 0 ? root.model.entry_name(root.model.cursor) : ""
                        color: Tokens.foregroundBright
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSubtitle
                        wrapMode: Text.WrapAnywhere
                    }

                    Rectangle {
                        width: parent.width
                        height: Tokens.borderWidth
                        color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                    }

                    Text {
                        width: parent.width
                        text: root.model.count > 0
                              ? (root.model.is_dir(root.model.cursor) ? "Folder" : "File")
                              : "Nothing selected"
                        color: Tokens.foregroundDim
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSmall
                    }

                    Text {
                        width: parent.width
                        text: root.model.count > 0 ? root.model.entry_path(root.model.cursor) : ""
                        color: Tokens.foregroundDim
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontCaption
                        wrapMode: Text.WrapAnywhere
                    }

                    Text {
                        width: parent.width
                        text: "Preview follows selection. Files never open by themselves."
                        color: Tokens.foregroundDim
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontCaption
                        wrapMode: Text.Wrap
                    }
                }
            }
        }
    }

    Accessible.role: Accessible.List
    Accessible.name: "File list, columns view with preview"
}
