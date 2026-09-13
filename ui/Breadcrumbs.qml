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
            // `revision` is read so this re-evaluates on every navigation
            // AND whenever the pane hands over a different model.
            model: (root.revision, root.model) ? root.model.breadcrumbs() : []

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

    // Rebuild when the location changes. Assigning rep.model here would
    // DESTROY the binding below and freeze the crumbs on whichever model was
    // current at the time — switching panes swaps the model without a
    // locationChanged, and the crumbs kept the other pane's path.
    property int revision: 0
    Connections {
        target: root.model
        function onLocationChanged() { root.revision++ }
    }
    onModelChanged: root.revision++
}
