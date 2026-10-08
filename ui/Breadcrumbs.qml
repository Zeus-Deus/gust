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
                // Each crumb is "path\tlabel" (a mounted place is named by
                // what it is, not by its mount path).
                readonly property string crumbPath: modelData.split("\t")[0]
                readonly property string crumbName: modelData.split("\t").slice(1).join("\t")
                readonly property real crumbWidth: crumbBox.width
                Text {
                    textFormat: Text.PlainText
                    id: sep
                    // The root crumb is itself "/", so the crumb after it
                    // needs no separator ("/ tmp", not "/ / tmp"). A place's
                    // name is not a separator: "nas / Shared".
                    visible: index > 1 || (index === 1 && root.fromPlace)
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
                        text: crumbName
                        color: index === rep.count - 1 ? Tokens.foregroundBright : Tokens.foreground
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                        font.bold: index === rep.count - 1
                    }

                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: root.pathChosen(crumbPath)
                    }

                    // Drop onto a crumb: into that parent folder.
                    DragTarget {
                        receiver: root.model
                        anchors.fill: parent
                        path: crumbPath
                        onHover: (t) => root.dragHover(t)
                        onLanded: (m, e) => root.dragLanded(m, e)
                    }

                    Accessible.role: Accessible.Button
                    Accessible.name: qsTr("Go to %1").arg(crumbName)
                }
            }
        }
    }

    // Rebuild when the location changes. Assigning rep.model here would
    // DESTROY the binding below and freeze the crumbs on whichever model was
    // current at the time — switching panes swaps the model without a
    // locationChanged, and the crumbs kept the other pane's path.
    property int revision: 0
    /// The first crumb is a place's name, not "/": the crumb after it needs
    /// a separator ("nas / Shared").
    readonly property bool fromPlace: {
        const m = rep.model
        return m && m.length > 0 && m[0].split("\t").slice(1).join("\t") !== "/"
    }
    Connections {
        target: root.model
        function onLocationChanged() { root.revision++ }
    }
    onModelChanged: root.revision++
}
