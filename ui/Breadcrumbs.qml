import QtQuick
import com.gust.app

// Clickable breadcrumbs. Keeps context while navigating.
Item {
    id: root
    required property var model
    signal pathChosen(string path)
    signal dragHover(string text)
    signal dragLanded(var model, string err)

    /// Centre of each crumb by its path, in window coordinates (qa).
    function crumbPoints() {
        const out = {}
        for (let i = 0; i < crumbRow.children.length; i++) {
            const d = crumbRow.children[i]
            if (!d.crumbPath) continue
            const p = d.mapToItem(null, d.width - d.crumbWidth / 2, d.height / 2)
            out[d.crumbPath] = [Math.round(p.x), Math.round(p.y)]
        }
        return out
    }

    // BOTH dimensions must be declared. An Item defaults to zero width, and a
    // zero-width item inside a Row simply does not appear — the crumbs existed
    // but were invisible until implicitWidth was bound to the content.
    implicitHeight: Tokens.controlHeight
    implicitWidth: crumbRow.implicitWidth

    /// What the path bar shows, as text, for tests: visible separators and
    /// crumb labels in order, space-separated.
    function shownText() {
        const out = []
        for (let i = 0; i < crumbRow.children.length; i++) {
            const d = crumbRow.children[i]
            if (!d.crumbLabel) continue
            if (d.sepShown) out.push("/")
            out.push(d.crumbLabel)
        }
        return out.join(" ")
    }

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
                readonly property bool sepShown: sep.visible
                readonly property string crumbLabel: crumbText.text
                readonly property string crumbPath: modelData
                readonly property real crumbWidth: crumbBox.width
                Text {
                    textFormat: Text.PlainText
                    id: sep
                    // The root crumb is itself "/", so the crumb after it
                    // needs no separator ("/ tmp", not "/ / tmp").
                    visible: index > 1
                    text: "/"
                    color: Tokens.foregroundDim
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                    anchors.verticalCenter: parent.verticalCenter
                }
                Rectangle {
                    id: crumbBox
                    width: crumbText.implicitWidth + Tokens.spaceLg
                    height: Tokens.controlHeight - Tokens.spaceSm
                    color: hover.hovered ? Tokens.withAlpha(Tokens.foreground, 0.08) : "transparent"
                    radius: Tokens.radius

                    HoverHandler { id: hover }

                    Text {
                        textFormat: Text.PlainText
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

                    // Drop onto a crumb: into that parent folder.
                    DragTarget {
                        anchors.fill: parent
                        path: modelData
                        onHover: (t) => root.dragHover(t)
                        onLanded: (m, e) => root.dragLanded(m, e)
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
