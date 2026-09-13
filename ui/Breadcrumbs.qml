import QtQuick
import com.gust.app

// Clickable breadcrumbs. Keeps context while navigating.
Item {
    id: root
    required property var model
    signal pathChosen(string path)

    // BOTH dimensions must be declared. An Item defaults to zero width, and a
    // zero-width item inside a Row simply does not appear — the crumbs existed
    // but were invisible until implicitWidth was bound to the content.
    implicitHeight: Tokens.controlHeight
    implicitWidth: crumbRow.implicitWidth

    Row {
        id: crumbRow
        anchors.left: parent.left
        anchors.verticalCenter: parent.verticalCenter
        spacing: 0

        Repeater {
            id: rep
            model: root.model ? root.model.breadcrumbs() : []

            delegate: Row {
                spacing: 0
                Text {
                    id: sep
                    visible: index > 0
                    text: "/"
                    color: Tokens.foregroundDim
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                    anchors.verticalCenter: parent.verticalCenter
                }
                Rectangle {
                    width: crumbText.implicitWidth + Tokens.spaceLg
                    height: Tokens.controlHeight - Tokens.spaceSm
                    color: hover.hovered ? Tokens.withAlpha(Tokens.foreground, 0.08) : "transparent"
                    radius: Tokens.radius

                    HoverHandler { id: hover }

                    Text {
                        id: crumbText
                        anchors.centerIn: parent
                        // Last crumb is the current folder: brighter, bolder.
                        text: {
                            const p = modelData
                            if (p === "/") return "/"
                            const parts = p.split("/")
                            return parts[parts.length - 1]
                        }
                        color: index === rep.count - 1 ? Tokens.foregroundBright : Tokens.foreground
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                        font.bold: index === rep.count - 1
                    }

                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: root.pathChosen(modelData)
                    }

                    Accessible.role: Accessible.Button
                    Accessible.name: "Go to " + modelData
                }
            }
        }
    }

    // Rebuild when the location changes.
    Connections {
        target: root.model
        function onLocationChanged() { rep.model = root.model.breadcrumbs() }
    }
}
