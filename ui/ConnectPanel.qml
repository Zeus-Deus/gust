import QtQuick
import com.gust.app

// Connect to another computer or to cloud storage (DESIGN §6.4.1).
//
// One list of everything Gust knows how to reach: Tailscale devices, the
// computers in ~/.ssh/config, and rclone's cloud remotes, plus "add a
// shared folder" and "add cloud storage". Opening this panel connects to
// nothing; each row's Connect is the first moment a device is contacted.
Item {
    id: root
    anchors.fill: parent
    visible: open
    z: 60

    property bool open: false
    /// The parsed places_json from the model.
    property var places: ({})
    property int current: 0
    property string selectionKey: ""
    /// One member's stable route key identifies the selected computer even
    /// when discovery refreshes its objects or changes its display label.
    property string routeKey: ""
    readonly property var selectedComputer: {
        for (const c of (root.places.computers || []))
            if (c.routes.some(r => r.kind + ":" + r.name === root.routeKey)) return c
        return null
    }

    /// kind is "tailnet", "ssh" or "cloud".
    signal connectRequested(string kind, string name, string target, string user)
    signal openRequested(string path)
    signal disconnectRequested(string kind, string name)
    signal actionRequested(string action)
    signal closed()

    function show() {
        root.routeKey = ""
        root.selectionKey = ""
        root.current = 0
        root.open = true
        Qt.callLater(() => keys.forceActiveFocus())
    }
    function showRoutes(key) {
        root.routeKey = key
        root.selectionKey = ""
        root.current = 0
        root.open = true
        Qt.callLater(() => {
            const usable = root.rows.findIndex(r => r.type === "place" && !r.lost
                && (r.kind === "ssh" || r.available !== false) && r.online !== false)
            if (usable >= 0) root.select(usable)
            else root.step(1)
            keys.forceActiveFocus()
        })
    }
    function back() {
        if (root.routeKey !== "") {
            root.routeKey = ""
            root.selectionKey = ""
            root.current = 0
            Qt.callLater(() => root.step(1))
        } else root.hide()
    }
    function hide() { root.open = false; root.closed() }
    /// Take the keyboard back (after a prompt on top of the panel closed).
    function refocus() { if (root.open) keys.forceActiveFocus() }

    function independentAlternate(lost, candidate) {
        if (candidate.kind + ":" + candidate.name === lost.kind + ":" + lost.name
            || candidate.lost || candidate.available === false || candidate.online === false) return false
        if (lost.requires_tailnet && candidate.requires_tailnet) return false
        const endpoints = lost.endpoint_ids || []
        if ((candidate.endpoint_ids || []).some(x => endpoints.indexOf(x) >= 0)) return false
        return true
    }

    /// A recovery suggestion never starts a connection or changes a mount.
    /// Only the currently browsed place may interrupt with a new suggestion.
    function recoveryCandidate(alreadyLost, location) {
        for (const c of (root.places.computers || [])) {
            for (const r of c.routes) {
                const key = r.kind + ":" + r.name
                if (!r.connected || !r.lost || alreadyLost.indexOf(key) >= 0 || !r.path) continue
                if (location !== r.path && !location.startsWith(r.path + "/")) continue
                if (c.routes.some(x => root.independentAlternate(r, x))) return key
            }
        }
        return ""
    }

    /// The rows on screen: section headings, places, notes and actions.
    readonly property var rows: {
        const p = root.places || {}
        const out = []
        const note = (t) => out.push({ type: "note", text: t })
        if (p.sshfs === false)
            note(qsTr("Computers need sshfs to connect — install it to enable them"))

        if (root.selectedComputer) {
            out.push({ type: "action", action: "back", text: qsTr("Back to computers") })
            note(qsTr("Each connection keeps its own account and folder. Interrupted file operations are not retried."))
            if (p.tailnetError) note(p.tailnetError)
            for (const r of root.selectedComputer.routes) {
                const via = r.kind === "tailnet" ? qsTr("Tailscale") : qsTr("SSH config")
                out.push(Object.assign({}, r, { type: "place", label: r.name + " · " + via,
                    extra: r.available === false ? qsTr("unavailable")
                           : r.kind === "tailnet" && !r.online ? qsTr("offline") : "" }))
            }
            return out
        }
        out.push({ type: "head", text: qsTr("Computers") })
        if (p.tailnetError) note(p.tailnetError)
        const computers = p.computers || []
        if (computers.length === 0) note(qsTr("No computers found in Tailscale or SSH config"))
        for (const c of computers) {
            const routes = c.routes || []
            if (routes.length === 0) continue
            const active = routes.find(r => r.connected && !r.lost)
            const r = active || routes[0]
            const vias = []
            if (routes.some(x => x.kind === "tailnet")) vias.push(qsTr("Tailscale"))
            if (routes.some(x => x.kind === "ssh")) vias.push(qsTr("SSH config"))
            const description = vias.join(" · ")
            out.push(Object.assign({}, r, { type: "place", label: c.name, computer: c,
                detail: description, extra: active ? qsTr("via %1").arg(active.name) : "",
                lost: !active && routes.some(x => x.lost) }))
        }

        out.push({ type: "head", text: qsTr("Cloud storage (rclone)") })
        const cl = p.cloud || []
        if (p.cloudError) note(p.cloudError)
        else if (cl.length === 0) note(qsTr("No cloud storage set up yet"))
        for (const c of cl)
            out.push({ type: "place", kind: "cloud", name: c.name, detail: c.detail, extra: "",
                       online: true, connected: c.connected, lost: c.lost, path: c.path, user: "" })

        out.push({ type: "head", text: qsTr("Add") })
        out.push({ type: "action", action: "cloud-setup", text: qsTr("Add cloud storage (opens rclone config)") })
        out.push({ type: "action", action: "share-add", text: qsTr("Add a shared folder (Windows or NFS)") })
        return out
    }
    /// The height of the rows, summed by type, BEFORE any delegate exists
    /// (a note's text is assumed to fit on two lines at most).
    readonly property real rowsHeight: {
        let h = 0
        for (const r of root.rows)
            h += r.type === "head" ? Tokens.rowHeight * 1.4
               : r.type === "note" ? Math.max(Tokens.rowHeight, Tokens.fontSmall * 2.8 + Tokens.spaceSm)
               : r.type === "place" ? Tokens.rowHeight * 1.7 : Tokens.rowHeight * 1.3
        return h
    }
    /// Rows that Enter can act on.
    function actionable(i) { return i >= 0 && i < rows.length && (rows[i].type === "place" || rows[i].type === "action") }
    function select(i) {
        root.current = i
        const r = root.rows[i]
        root.selectionKey = !r ? "" : r.type === "action" ? "action:" + r.action : r.kind + ":" + r.name
    }
    function step(d) {
        let i = root.current
        for (let n = 0; n < rows.length; n++) {
            i = Math.max(0, Math.min(rows.length - 1, i + d))
            if (actionable(i)) { root.select(i); list.positionViewAtIndex(i, ListView.Contain); return }
        }
    }
    onRowsChanged: {
        if (root.selectionKey === "") {
            if (!actionable(root.current)) Qt.callLater(() => { if (root.selectionKey === "") root.step(1) })
        } else {
            root.current = root.rows.findIndex(r =>
                r.type === "action" ? "action:" + r.action === root.selectionKey
                : r.type === "place" && (r.kind + ":" + r.name === root.selectionKey
                    || (r.computer && r.computer.routes.some(x => x.kind + ":" + x.name === root.selectionKey))))
        }
    }

    function activate(i) {
        const r = rows[i]
        if (!r) return
        if (r.type === "action") {
            if (r.action === "back") root.back()
            else root.actionRequested(r.action)
            return
        }
        if (r.type !== "place") return
        if (r.connected && !r.lost) { root.openRequested(r.path); return }
        if (r.computer && r.computer.routes.length > 1) {
            root.showRoutes(r.kind + ":" + r.name)
            return
        }
        if (r.lost || (r.kind !== "ssh" && r.available === false)) return
        root.connectRequested(r.kind, r.name, r.detail, r.user || root.places.localUser || "")
    }

    /// Window points of each row's buttons, "kind:name" for the main one and
    /// "kind:name:off" for Disconnect (qa).
    function buttonPoints() {
        const out = {}
        for (let i = 0; i < list.count; i++) {
            const it = list.itemAtIndex(i)
            if (!it || !it.mainButton) continue
            const r = rows[i]
            const key = r.type === "action" ? "action:" + r.action : r.kind + ":" + r.name
            const b = it.mainButton
            if (b.visible) {
                const p = b.mapToItem(null, b.width / 2, b.height / 2)
                out[key] = [Math.round(p.x), Math.round(p.y)]
            }
            const o = it.offButton
            if (o && o.visible) {
                const q = o.mapToItem(null, o.width / 2, o.height / 2)
                out[key + ":off"] = [Math.round(q.x), Math.round(q.y)]
            }
        }
        return out
    }

    Rectangle {
        anchors.fill: parent
        color: Tokens.withAlpha(Tokens.background, 0.82)
        MouseArea { anchors.fill: parent; onClicked: root.hide() }
    }

    Item {
        id: keys
        focus: root.open
        Keys.onPressed: (event) => {
            if (event.key === Qt.Key_Escape || event.key === Qt.Key_Left) { root.back(); event.accepted = true }
            else if (event.key === Qt.Key_Right) {
                const r = root.rows[root.current]
                if (r && r.computer && r.computer.routes.length > 1) root.showRoutes(r.kind + ":" + r.name)
                event.accepted = true
            }
            else if (event.key === Qt.Key_Down) { root.step(1); event.accepted = true }
            else if (event.key === Qt.Key_Up) { root.step(-1); event.accepted = true }
            else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                root.activate(root.current); event.accepted = true
            }
        }
    }

    Rectangle {
        id: card
        objectName: "connectPanel"
        anchors.centerIn: parent
        width: Math.min(parent.width - Tokens.spaceXl * 4, Math.round(Tokens.rowHeight * 30))
        // Sized from what it lists: the title block, every row at its own
        // height, and the footer, so a short list is a short card (it was a
        // fixed 24 rows tall, mostly empty). A long list scrolls.
        height: Math.min(parent.height - Tokens.spaceXl * 4,
                         Math.round(head.implicitHeight + Tokens.spaceXl + Tokens.spaceLg
                                    + root.rowsHeight + Tokens.spaceSm * 2
                                    + foot.implicitHeight + Tokens.spaceXl * 2))
        color: Tokens.backgroundDark
        border.width: Math.max(1, Tokens.borderWidth)
        border.color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
        MouseArea { anchors.fill: parent }

        Column {
            id: head
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.margins: Tokens.spaceXl
            spacing: Tokens.spaceSm
            Text {
                textFormat: Text.PlainText
                text: root.selectedComputer ? qsTr("Connections to %1").arg(root.selectedComputer.name) : qsTr("Connect")
                color: Tokens.foregroundBright
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSubtitle
            }
            Text {
                textFormat: Text.PlainText
                width: parent.width
                text: qsTr("Browse another computer or your cloud storage like a folder. Nothing connects until you choose Connect.")
                color: Tokens.foregroundDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
                wrapMode: Text.Wrap
            }
        }

        ListView {
            id: list
            WheelScroll { view: list }
            objectName: "connectList"
            anchors.top: head.bottom
            anchors.topMargin: Tokens.spaceLg
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: foot.top
            anchors.margins: Tokens.spaceSm
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            model: root.rows
            currentIndex: root.current
            delegate: Rectangle {
                id: row
                required property var modelData
                required property int index
                readonly property bool isPlace: modelData.type === "place"
                readonly property bool isAction: modelData.type === "action"
                property alias mainButton: mainBtn
                property alias offButton: offBtn
                width: list.width
                height: modelData.type === "head" ? Tokens.rowHeight * 1.4
                      : modelData.type === "note" ? Math.max(Tokens.rowHeight, noteText.implicitHeight + Tokens.spaceSm)
                      : isPlace ? Tokens.rowHeight * 1.7 : Tokens.rowHeight * 1.3
                color: (isPlace || isAction) && index === root.current
                       ? Tokens.withAlpha(Tokens.selectedBackground, Math.max(Tokens.selectedAlpha, 0.14))
                       : (isPlace || isAction) && rowMouse.containsMouse ? Tokens.withAlpha(Tokens.foreground, 0.05)
                       : "transparent"
                MouseArea {
                    id: rowMouse
                    anchors.fill: parent
                    hoverEnabled: true
                    enabled: row.isPlace || row.isAction
                    onClicked: root.select(row.index)
                    onDoubleClicked: root.activate(row.index)
                }

                // Section heading.
                Text {
                    textFormat: Text.PlainText
                    visible: row.modelData.type === "head"
                    anchors.left: parent.left
                    anchors.leftMargin: Tokens.rowPaddingX
                    anchors.bottom: parent.bottom
                    anchors.bottomMargin: Tokens.spaceXs
                    text: row.modelData.text || ""
                    color: Tokens.foregroundDim
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontCaption
                    font.bold: true
                }
                // Why a section is empty, in words.
                Text {
                    id: noteText
                    visible: row.modelData.type === "note"
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.leftMargin: Tokens.rowPaddingX
                    anchors.verticalCenter: parent.verticalCenter
                    text: row.modelData.text || ""
                    color: Tokens.foregroundDim
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontSmall
                    wrapMode: Text.Wrap
                    textFormat: Text.PlainText
                }

                Column {
                    visible: row.isPlace || row.isAction
                    anchors.left: parent.left
                    anchors.leftMargin: Tokens.rowPaddingX
                    anchors.right: btns.left
                    anchors.rightMargin: Tokens.spaceLg
                    anchors.verticalCenter: parent.verticalCenter
                    Text {
                        width: parent.width
                        text: (row.isAction ? row.modelData.text : row.modelData.label || row.modelData.name) || ""
                        color: row.isPlace && !row.modelData.online ? Tokens.foregroundDim : Tokens.foreground
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                        elide: Text.ElideRight
                        textFormat: Text.PlainText
                    }
                    Text {
                        visible: row.isPlace
                        width: parent.width
                        // State in words, never colour alone.
                        text: !row.isPlace ? ""
                              : [row.modelData.computer ? row.modelData.detail
                                 : (row.modelData.user ? row.modelData.user + "@" : "") + row.modelData.detail
                                   + (row.modelData.port ? ":" + row.modelData.port : ""),
                                 row.modelData.computer ? "" : row.modelData.remote_path || qsTr("login folder"),
                                 row.modelData.extra,
                                 row.modelData.lost ? qsTr("connection lost")
                                 : row.modelData.connected ? qsTr("connected") : ""]
                                .filter(x => x).join("  ·  ")
                        color: row.isPlace && row.modelData.lost ? Tokens.danger
                               : row.isPlace && row.modelData.connected ? Tokens.accent : Tokens.foregroundDim
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontCaption
                        elide: Text.ElideMiddle
                        textFormat: Text.PlainText
                    }
                }

                Row {
                    id: btns
                    anchors.right: parent.right
                    anchors.rightMargin: Tokens.spaceLg
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: Tokens.spaceSm
                    Rectangle {
                        visible: row.isPlace && !!row.modelData.computer
                                 && row.modelData.computer.routes.length > 1
                                 && row.modelData.connected && !row.modelData.lost
                        width: routesLabel.implicitWidth + Tokens.spaceLg
                        height: Tokens.controlHeight - Tokens.spaceSm
                        color: "transparent"
                        border.color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                        border.width: Tokens.borderWidth
                        Text {
                            id: routesLabel
                            anchors.centerIn: parent
                            text: qsTr("Connections…")
                            textFormat: Text.PlainText
                            color: Tokens.foreground
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                        }
                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: root.showRoutes(row.modelData.kind + ":" + row.modelData.name)
                        }
                        Accessible.role: Accessible.Button
                        Accessible.name: qsTr("Connections to %1").arg(row.modelData.label || "")
                    }
                    Rectangle {
                        id: offBtn
                        objectName: "connectOff"
                        visible: row.isPlace && row.modelData.connected
                        width: offLabel.implicitWidth + Tokens.spaceLg * 2
                        height: Tokens.controlHeight - Tokens.spaceSm
                        color: offMouse.containsMouse ? Tokens.withAlpha(Tokens.danger, 0.2) : "transparent"
                        border.color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                        border.width: Tokens.borderWidth
                        Text {
                            textFormat: Text.PlainText
                            id: offLabel
                            anchors.centerIn: parent
                            text: qsTr("Disconnect")
                            color: Tokens.foreground
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                        }
                        MouseArea {
                            id: offMouse
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: { root.select(row.index); root.disconnectRequested(row.modelData.kind, row.modelData.name) }
                        }
                        Accessible.role: Accessible.Button
                        Accessible.name: qsTr("Disconnect %1").arg(row.modelData.name || "")
                    }
                    Rectangle {
                        id: mainBtn
                        objectName: "connectMain"
                        visible: row.isPlace || row.isAction
                        width: mainLabel.implicitWidth + Tokens.spaceLg * 2
                        height: Tokens.controlHeight - Tokens.spaceSm
                        color: mainMouse.containsMouse ? Tokens.withAlpha(Tokens.accent, 0.25) : "transparent"
                        opacity: row.isPlace && row.modelData.available === false
                                 && !(row.modelData.computer && row.modelData.computer.routes.length > 1) ? 0.5 : 1
                        border.color: Tokens.accent
                        border.width: Tokens.borderWidth
                        Text {
                            textFormat: Text.PlainText
                            id: mainLabel
                            anchors.centerIn: parent
                            text: row.isAction ? (row.modelData.action === "back" ? qsTr("Back") : qsTr("Set up"))
                                  : row.modelData.connected && !row.modelData.lost ? qsTr("Open")
                                  : row.modelData.computer && row.modelData.computer.routes.length > 1 ? qsTr("Connections…")
                                  : row.modelData.lost ? qsTr("Disconnect first")
                                  : row.modelData.kind !== "ssh" && row.modelData.available === false ? qsTr("Unavailable") : qsTr("Connect")
                            color: Tokens.foreground
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                        }
                        MouseArea {
                            id: mainMouse
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: { root.select(row.index); root.activate(row.index) }
                        }
                        Accessible.role: Accessible.Button
                        Accessible.name: mainLabel.text + " " + (row.modelData.name || row.modelData.text || "")
                    }
                }
            }
        }

        Text {
            textFormat: Text.PlainText
            id: foot
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            anchors.margins: Tokens.spaceXl
            text: root.routeKey !== "" ? qsTr("Enter chooses · Escape goes back") : qsTr("Enter connects or opens · Escape closes")
            color: Tokens.foregroundDim
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
        }
    }

    Accessible.role: Accessible.Dialog
    Accessible.name: qsTr("Connect")
}
