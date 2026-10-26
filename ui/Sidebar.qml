import QtQuick
import com.gust.app

// Places / Devices / Remote rail. Must work collapsed, with a visible
// restore control (never a hidden-only affordance).
Rectangle {
    id: root
    property var model
    signal placeChosen(string path)
    /// A drag is over a place: what releasing would do ("" = left).
    signal dragHover(string text)
    /// Something was dropped on a place, through `model`.
    signal dragLanded(var model, string err)

    color: Tokens.backgroundDark

    Rectangle {
        anchors.right: parent.right
        width: Tokens.borderWidth
        height: parent.height
        color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
    }

    /// Connected places, each {kind, name, path, lost}. Kept in sync by
    /// Main from the model's places_json.
    property var remotePlaces: []
    /// Open the Connect panel.
    signal connectChosen()
    /// A connected place clicked: browse it.
    signal remoteChosen(string path)
    /// Right-click on a connected place: Main offers Open / Disconnect.
    signal remoteMenu(string kind, string name, real x, real y)

    /// Pinned folders, shared with every GTK file dialog on the desktop.
    property var bookmarks: []
    /// Right-click on a bookmark: Main shows its menu (Remove).
    signal bookmarkMenu(string path, real x, real y)
    signal trashChosen()
    signal recentChosen()

    /// Removable devices the kernel reports. Refreshed by Main; never
    /// polled from here, because a rail that lies about what is attached is
    /// worse than one that says nothing.
    property var devices: []
    /// The user clicked a device row: mount it, or browse it if mounted.
    signal deviceChosen(string path, bool mounted)
    /// Right-click on a drive row: its menu (open, eject / lock).
    signal deviceMenu(string path, bool mounted, bool encrypted, bool locked, real x, real y)
    /// A phone row was clicked (DESIGN §6.13.1), or right-clicked for its menu.
    signal phoneChosen(string id, string stateKey)
    signal phoneMenu(string id, bool apps, bool open, real x, real y)

    /// Centre of each place row by label, in window coordinates (qa).
    function placePoints() {
        const out = {}
        for (let i = 0; i < placeRepeater.count; i++) {
            const it = placeRepeater.itemAt(i)
            if (!it) continue
            const p = it.mapToItem(null, it.width / 2, it.height / 2)
            out[places.get(i).label] = [Math.round(p.x), Math.round(p.y)]
        }
        for (let i = 0; i < bookmarkRepeater.count; i++) {
            const it = bookmarkRepeater.itemAt(i)
            if (!it) continue
            const p = it.mapToItem(null, it.width / 2, it.height / 2)
            out["bookmark:" + root.bookmarks[i].label] = [Math.round(p.x), Math.round(p.y)]
        }
        for (let i = 0; i < deviceRepeater.count; i++) {
            const it = deviceRepeater.itemAt(i)
            if (!it) continue
            const p = it.mapToItem(null, it.width / 2, it.height / 2)
            out["device:" + root.devices[i].name] = [Math.round(p.x), Math.round(p.y)]
        }
        for (let i = 0; i < remoteRepeater.count; i++) {
            const it = remoteRepeater.itemAt(i)
            if (!it) continue
            const p = it.mapToItem(null, it.width / 2, it.height / 2)
            out["remote:" + root.remotePlaces[i].name] = [Math.round(p.x), Math.round(p.y)]
        }
        const c = connectButton.mapToItem(null, connectButton.width / 2, connectButton.height / 2)
        out["connect"] = [Math.round(c.x), Math.round(c.y)]
        return out
    }

    ListModel {
        id: places
        // Defaults rooted in the person's configured HOME, not the listing.
    }

    Component.onCompleted: {
        const homeRoot = root.model ? root.model.home_path : ""
        if (homeRoot.length === 0) return
        const candidates = [
            { label: qsTr("Home"),        path: homeRoot,                     group: "Places" },
            { label: qsTr("Documents"),   path: homeRoot + "/Documents",      group: "Places" },
            { label: qsTr("Downloads"),   path: homeRoot + "/Downloads",      group: "Places" },
            { label: qsTr("Pictures"),    path: homeRoot + "/Pictures",       group: "Places" },
            { label: qsTr("Videos"),      path: homeRoot + "/Videos",         group: "Places" },
            { label: qsTr("Music"),       path: homeRoot + "/Music",          group: "Places" },
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
        WheelScroll { view: rail; enabled: rail.interactive }
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
            textFormat: Text.PlainText
            leftPadding: Tokens.rowPaddingX
            text: "Places"
            color: Tokens.foregroundDim
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
        }

        Repeater {
            id: placeRepeater
            model: places
            delegate: Rectangle {
                width: column.width
                height: Tokens.rowHeight
                color: hover.hovered ? Tokens.withAlpha(Tokens.foreground, 0.06) : "transparent"

                HoverHandler { id: hover }

                Text {
                    textFormat: Text.PlainText
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
                DragTarget {
                    receiver: root.model
                    anchors.fill: parent
                    path: model.path
                    onHover: (t) => root.dragHover(t)
                    onLanded: (m, e) => root.dragLanded(m, e)
                }

                Accessible.role: Accessible.Button
                Accessible.name: "Go to " + model.label
            }
        }

        // Recent files, from the desktop's own list.
        Rectangle {
            width: column.width
            height: Tokens.rowHeight
            color: recentHover.hovered ? Tokens.withAlpha(Tokens.foreground, 0.06) : "transparent"
            HoverHandler { id: recentHover }
            Text {
                textFormat: Text.PlainText
                anchors.verticalCenter: parent.verticalCenter
                leftPadding: Tokens.rowPaddingX
                text: qsTr("Recent")
                color: Tokens.foreground
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontBody
            }
            MouseArea {
                anchors.fill: parent
                cursorShape: Qt.PointingHandCursor
                onClicked: root.recentChosen()
            }
            Accessible.role: Accessible.Button
            Accessible.name: qsTr("Recent files")
        }

        // Trash: what was deleted, and a way back.
        Rectangle {
            width: column.width
            height: Tokens.rowHeight
            color: trashHover.hovered ? Tokens.withAlpha(Tokens.foreground, 0.06) : "transparent"
            HoverHandler { id: trashHover }
            Text {
                textFormat: Text.PlainText
                anchors.verticalCenter: parent.verticalCenter
                leftPadding: Tokens.rowPaddingX
                text: qsTr("Trash")
                color: Tokens.foreground
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontBody
            }
            MouseArea {
                anchors.fill: parent
                cursorShape: Qt.PointingHandCursor
                onClicked: root.trashChosen()
            }
            Accessible.role: Accessible.Button
            Accessible.name: qsTr("Open the Trash")
        }

        // Bookmarks — the same list Nautilus and the GTK Open/Save dialogs
        // use, so a folder pinned here is pinned everywhere.
        Item {
            width: 1
            height: Tokens.spaceXl
            visible: root.bookmarks.length > 0
        }
        Text {
            textFormat: Text.PlainText
            leftPadding: Tokens.rowPaddingX
            text: qsTr("Bookmarks")
            color: Tokens.foregroundDim
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
            visible: root.bookmarks.length > 0
        }
        Repeater {
            id: bookmarkRepeater
            model: root.bookmarks
            delegate: Rectangle {
                required property var modelData
                width: column.width
                height: Tokens.rowHeight
                color: bmMouse.containsMouse ? Tokens.withAlpha(Tokens.foreground, 0.06) : "transparent"
                Text {
                    textFormat: Text.PlainText
                    anchors.verticalCenter: parent.verticalCenter
                    leftPadding: Tokens.rowPaddingX
                    width: column.width - Tokens.rowPaddingX
                    text: modelData.label
                    // A pinned folder that is gone (unplugged drive, deleted)
                    // is dimmed rather than hidden: it may come back.
                    color: modelData.exists ? Tokens.foreground : Tokens.foregroundDim
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                    elide: Text.ElideRight
                }
                DragTarget {
                    receiver: root.model
                    anchors.fill: parent
                    z: 1
                    path: modelData.exists ? modelData.path : ""
                    onHover: (t) => root.dragHover(t)
                    onLanded: (m, e) => root.dragLanded(m, e)
                }
                MouseArea {
                    id: bmMouse
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    acceptedButtons: Qt.LeftButton | Qt.RightButton
                    onClicked: (mouse) => {
                        if (mouse.button === Qt.RightButton) {
                            const g = mapToItem(null, mouse.x, mouse.y)
                            root.bookmarkMenu(modelData.path, g.x, g.y)
                        } else {
                            root.placeChosen(modelData.path)
                        }
                    }
                }
                Accessible.role: Accessible.Button
                Accessible.name: qsTr("Go to bookmark ") + modelData.label
            }
        }

        // The Remote section is explanatory, not a control. In a window too
        // short to hold it, showing half a sentence is worse than showing
        // none: a clipped line reads as breakage. Hide the whole section
        // rather than letting it bleed past the bottom edge, and keep it
        // reachable by scrolling once there is room to scroll.
        // Devices — USB sticks, cards and discs. The list is a local read
        // of the kernel's own view, so it cannot hang; Gust never mounts
        // anything on its own (DESIGN §6.13).
        Item {
            width: 1
            height: Tokens.spaceXl
            visible: root.devices.length > 0
        }

        Text {
            textFormat: Text.PlainText
            id: devicesHeader
            leftPadding: Tokens.rowPaddingX
            text: qsTr("Devices")
            color: Tokens.foregroundDim
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
            visible: root.devices.length > 0
        }

        Repeater {
            id: deviceRepeater
            model: root.devices
            delegate: Rectangle {
                required property var modelData
                width: column.width
                height: Tokens.rowHeight
                color: deviceMouse.containsMouse
                       ? Tokens.withAlpha(Tokens.foreground, 0.06)
                       : "transparent"

                Column {
                    anchors.verticalCenter: parent.verticalCenter
                    anchors.left: parent.left
                    anchors.leftMargin: Tokens.rowPaddingX
                    anchors.right: parent.right
                    anchors.rightMargin: Tokens.rowPaddingX
                    spacing: 0

                    Text {
                        textFormat: Text.PlainText
                        width: parent.width
                        text: modelData.label
                        color: Tokens.foreground
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                        elide: Text.ElideRight
                    }
                    // State in WORDS as well as position: colour alone is
                    // not enough to say connected (plan §2 mockup F).
                    Text {
                        textFormat: Text.PlainText
                        width: parent.width
                        text: modelData.state
                        color: Tokens.foregroundDim
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontCaption
                        elide: Text.ElideRight
                    }
                }

                DragTarget {

                    receiver: root.model
                    anchors.fill: parent
                    z: 1
                    path: modelData.mounted && !modelData.empty ? modelData.path : ""
                    onHover: (t) => root.dragHover(t)
                    onLanded: (m, e) => root.dragLanded(m, e)
                }
                MouseArea {
                    id: deviceMouse
                    anchors.fill: parent
                    hoverEnabled: true
                    acceptedButtons: Qt.LeftButton | Qt.RightButton
                    // An empty drive has nothing to open, and clicking it
                    // must not look like a broken button.
                    onClicked: (mouse) => {
                        if (modelData.phone) {
                            if (mouse.button === Qt.RightButton) {
                                const g = mapToItem(null, mouse.x, mouse.y)
                                root.phoneMenu(modelData.name, !!modelData.apps, !!modelData.mounted, g.x, g.y)
                            } else {
                                root.phoneChosen(modelData.name, modelData.stateKey)
                            }
                            return
                        }
                        if (modelData.empty) return
                        if (mouse.button === Qt.RightButton) {
                            const g = mapToItem(null, mouse.x, mouse.y)
                            root.deviceMenu(modelData.path, !!modelData.mounted, !!modelData.encrypted,
                                            !!modelData.locked, g.x, g.y)
                            return
                        }
                        root.deviceChosen(modelData.path, modelData.mounted)
                    }
                }

                Accessible.role: Accessible.Button
                Accessible.name: modelData.label + ", " + modelData.state
            }
        }

        Item {
            width: 1
            height: Tokens.spaceXl
            visible: remoteBlock.visible
        }

        // "Remote", with the way in: Connect… opens one list of every
        // computer, Tailscale device and cloud remote Gust knows about.
        Item {
            width: column.width
            height: Tokens.rowHeight
            Text {
                textFormat: Text.PlainText
                id: remoteHeader
                anchors.verticalCenter: parent.verticalCenter
                leftPadding: Tokens.rowPaddingX
                text: qsTr("Remote")
                color: Tokens.foregroundDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontCaption
            }
            Rectangle {
                id: connectButton
                objectName: "sidebarConnect"
                anchors.right: parent.right
                anchors.rightMargin: Tokens.rowPaddingX
                anchors.verticalCenter: parent.verticalCenter
                width: connectLabel.implicitWidth + Tokens.spaceLg * 2
                height: Tokens.rowHeight - Tokens.spaceSm
                radius: Tokens.radius
                color: connectMouse.containsMouse ? Tokens.withAlpha(Tokens.accent, 0.22) : "transparent"
                border.width: Math.max(1, Tokens.borderWidth)
                border.color: Tokens.withAlpha(Tokens.accent, 0.7)
                Text {
                    textFormat: Text.PlainText
                    id: connectLabel
                    anchors.centerIn: parent
                    text: qsTr("Connect…")
                    color: Tokens.foreground
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontCaption
                }
                MouseArea {
                    id: connectMouse
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: root.connectChosen()
                }
                Accessible.role: Accessible.Button
                Accessible.name: qsTr("Connect to another computer or cloud storage")
            }
        }

        // Connected places: click to browse, drop files on them, right-click
        // to disconnect. Kept in sync by Main after every connect and
        // disconnect rather than guessed.
        Repeater {
            id: remoteRepeater
            model: root.remotePlaces
            delegate: Rectangle {
                required property var modelData
                width: column.width
                height: Tokens.rowHeight
                color: remoteMouse.containsMouse ? Tokens.withAlpha(Tokens.foreground, 0.06) : "transparent"
                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    leftPadding: Tokens.rowPaddingX
                    width: column.width - Tokens.rowPaddingX
                    // Colour alone never carries the state (plan §2 mockup F).
                    text: (modelData.label || modelData.name) + (modelData.lost ? qsTr("  ·  lost") : "")
                    color: modelData.lost ? Tokens.danger : Tokens.foreground
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                    elide: Text.ElideRight
                    textFormat: Text.PlainText
                }
                DragTarget {
                    receiver: root.model
                    anchors.fill: parent
                    z: 1
                    path: modelData.lost ? "" : modelData.path
                    onHover: (t) => root.dragHover(t)
                    onLanded: (m, e) => root.dragLanded(m, e)
                }
                MouseArea {
                    id: remoteMouse
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    acceptedButtons: Qt.LeftButton | Qt.RightButton
                    onClicked: (mouse) => {
                        if (mouse.button === Qt.RightButton) {
                            const g = mapToItem(null, mouse.x, mouse.y)
                            root.remoteMenu(modelData.kind, modelData.name, g.x, g.y)
                        } else {
                            root.remoteChosen(modelData.path)
                        }
                    }
                }
                Accessible.role: Accessible.Button
                Accessible.name: qsTr("Go to %1").arg(modelData.name)
            }
        }

        Text {
            textFormat: Text.PlainText
            id: remoteBlock
            leftPadding: Tokens.rowPaddingX
            rightPadding: Tokens.rowPaddingX
            width: column.width
            text: qsTr("Nothing connected")
            color: Tokens.foregroundDim
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
            wrapMode: Text.Wrap
            // Measured against the rail height directly, never against this
            // item's own y: y depends on earlier siblings' visibility, and
            // binding visible to it would be a loop (plan §8 warns about
            // exactly this class of QML layout bug).
            readonly property int placesHeight:
                Tokens.spaceXl + (places.count + 3) * Tokens.rowHeight
                + (root.bookmarks.length > 0 ? Tokens.spaceXl + (root.bookmarks.length + 1) * Tokens.rowHeight : 0)
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
