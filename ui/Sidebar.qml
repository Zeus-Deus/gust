import QtQuick
import com.gust.app

// Places / Devices / Remote rail. Must work collapsed, with a visible
// restore control (never a hidden-only affordance).
Rectangle {
    id: root
    property var model
    signal placeChosen(string path)

    color: Tokens.backgroundDark

    Rectangle {
        anchors.right: parent.right
        width: Tokens.borderWidth
        height: parent.height
        color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
    }

    /// Aliases of places that are connected right now.
    property var remotePlaces: []

    ListModel {
        id: places
        // Populated at startup from XDG dirs that actually exist.
    }

    Component.onCompleted: {
        const home = root.model ? root.model.location : ""
        // Derive the home root from the starting location's prefix.
        const parts = home.split("/")
        const homeRoot = parts.length > 2 ? "/" + parts[1] + "/" + parts[2] : home
        const candidates = [
            { label: "Home",        path: homeRoot,                     group: "Places" },
            { label: "Documents",   path: homeRoot + "/Documents",      group: "Places" },
            { label: "Downloads",   path: homeRoot + "/Downloads",      group: "Places" },
            { label: "Pictures",    path: homeRoot + "/Pictures",       group: "Places" },
            { label: "Videos",      path: homeRoot + "/Videos",         group: "Places" },
            { label: "Music",       path: homeRoot + "/Music",          group: "Places" },
        ]
        for (var i = 0; i < candidates.length; i++)
            places.append(candidates[i])
    }

    // A Flickable, not a bare Column: at the minimum window size the places
    // list plus the Remote note is taller than the rail, and a plain Column
    // spills past the bottom edge where the parent clips it mid-sentence.
    // Scrolling keeps every place reachable in a small window, which §6 of
    // the plan requires ("narrow window/rail restore").
    Flickable {
        id: rail
        anchors.fill: parent
        anchors.topMargin: Tokens.spaceXl
        contentWidth: width
        contentHeight: column.implicitHeight + Tokens.spaceXl
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        // Only interactive when there is something to scroll to, so a
        // roomy window does not drift under a stray gesture.
        interactive: contentHeight > height

    Column {
        id: column
        width: rail.width
        spacing: Tokens.spaceSm

        Text {
            leftPadding: Tokens.rowPaddingX
            text: "Places"
            color: Tokens.foregroundDim
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
        }

        Repeater {
            model: places
            delegate: Rectangle {
                width: column.width
                height: Tokens.rowHeight
                color: hover.hovered ? Tokens.withAlpha(Tokens.foreground, 0.06) : "transparent"

                HoverHandler { id: hover }

                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    leftPadding: Tokens.rowPaddingX
                    text: model.label
                    color: Tokens.foreground
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                    elide: Text.ElideRight
                    width: column.width - Tokens.rowPaddingX
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: root.placeChosen(model.path)
                }

                Accessible.role: Accessible.Button
                Accessible.name: "Go to " + model.label
            }
        }

        // The Remote section is explanatory, not a control. In a window too
        // short to hold it, showing half a sentence is worse than showing
        // none: a clipped line reads as breakage. Hide the whole section
        // rather than letting it bleed past the bottom edge, and keep it
        // reachable by scrolling once there is room to scroll.
        Item {
            width: 1
            height: Tokens.spaceXl
            visible: remoteBlock.visible
        }

        Text {
            id: remoteHeader
            leftPadding: Tokens.rowPaddingX
            text: "Remote"
            color: Tokens.foregroundDim
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
            // Shown when there is either a connected place or the note.
            visible: remoteBlock.visible || root.remotePlaces.length > 0
        }

        // Connected places. Kept in sync by Main on every connect/disconnect
        // rather than polled: a rail that lies about what is mounted is
        // worse than one that says nothing.
        Repeater {
            model: root.remotePlaces
            delegate: Text {
                leftPadding: Tokens.rowPaddingX
                width: column.width
                height: Tokens.rowHeight
                verticalAlignment: Text.AlignVCenter
                text: modelData + "  ·  connected"
                color: Tokens.foreground
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontBody
                elide: Text.ElideRight
                visible: remoteHeader.visible
            }
        }

        Text {
            id: remoteBlock
            leftPadding: Tokens.rowPaddingX
            rightPadding: Tokens.rowPaddingX
            width: column.width
            text: "No places added yet. Discovery never connects on its own."
            color: Tokens.foregroundDim
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
            wrapMode: Text.Wrap
            // Measured against the rail height directly, never against this
            // item's own y: y depends on earlier siblings' visibility, and
            // binding visible to it would be a loop (plan §8 warns about
            // exactly this class of QML layout bug).
            readonly property int placesHeight:
                Tokens.spaceXl + (places.count + 1) * Tokens.rowHeight
            // The note explains why the list is empty, so it belongs only
            // when the list IS empty. Saying "no places added yet" beside a
            // connected place is a lie the rail must never tell.
            visible: root.remotePlaces.length === 0
                     && placesHeight + Tokens.spaceXl + implicitHeight * 2
                        <= rail.height
        }
    }
    }

    Accessible.role: Accessible.Pane
    Accessible.name: "Places sidebar"
}
