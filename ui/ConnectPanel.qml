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

    /// kind is "tailnet", "ssh" or "cloud".
    signal connectRequested(string kind, string name, string target, string user)
    signal openRequested(string path)
    signal disconnectRequested(string kind, string name)
    signal actionRequested(string action)
    signal closed()

    function show() {
        root.current = 0
        root.open = true
        Qt.callLater(() => keys.forceActiveFocus())
    }
    function hide() { root.open = false; root.closed() }
    /// Take the keyboard back (after a prompt on top of the panel closed).
    function refocus() { if (root.open) keys.forceActiveFocus() }

    /// The rows on screen: section headings, places, notes and actions.
    readonly property var rows: {
        const p = root.places || {}
        const out = []
        const note = (t) => out.push({ type: "note", text: t })
        if (p.sshfs === false)
            note(qsTr("Computers need sshfs to connect — install it to enable them"))

        out.push({ type: "head", text: qsTr("Tailscale devices") })
        const tn = p.tailnet || []
        if (p.tailnetError) note(p.tailnetError)
        else if (tn.length === 0) note(qsTr("No other devices on your tailnet"))
        for (const d of tn)
            out.push({ type: "place", kind: "tailnet", name: d.name, detail: d.detail,
                       extra: d.os + (d.online ? "" : qsTr(" · offline")), online: d.online,
                       connected: d.connected, lost: d.lost, path: d.path,
                       user: d.user || p.localUser || "" })

        out.push({ type: "head", text: qsTr("Computers (SSH config)") })
        const ss = p.ssh || []
        if (ss.length === 0) note(qsTr("No computers in ~/.ssh/config"))
        for (const h of ss)
            out.push({ type: "place", kind: "ssh", name: h.name, detail: h.detail, extra: "",
                       online: true, connected: h.connected, lost: h.lost, path: h.path, user: "" })

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
    function step(d) {
        let i = root.current
        for (let n = 0; n < rows.length; n++) {
            i = Math.max(0, Math.min(rows.length - 1, i + d))
            if (actionable(i)) { root.current = i; list.positionViewAtIndex(i, ListView.Contain); return }
        }
    }
    onRowsChanged: if (!actionable(root.current)) Qt.callLater(() => root.step(1))

    function activate(i) {
        const r = rows[i]
        if (!r) return
        if (r.type === "action") { root.actionRequested(r.action); return }
        if (r.type !== "place") return
        if (r.connected && !r.lost) root.openRequested(r.path)
        else root.connectRequested(r.kind, r.name, r.detail, r.user)
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
            if (event.key === Qt.Key_Escape) { root.hide(); event.accepted = true }
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
                text: qsTr("Connect")
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
                    onClicked: root.current = row.index
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
                        text: (row.isAction ? row.modelData.text : row.modelData.name) || ""
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
                              : [row.modelData.detail, row.modelData.extra,
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
                            onClicked: { root.current = row.index; root.disconnectRequested(row.modelData.kind, row.modelData.name) }
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
                        border.color: Tokens.accent
                        border.width: Tokens.borderWidth
                        Text {
                            textFormat: Text.PlainText
                            id: mainLabel
                            anchors.centerIn: parent
                            text: row.isAction ? qsTr("Set up")
                                  : row.modelData.connected && !row.modelData.lost ? qsTr("Open") : qsTr("Connect")
                            color: Tokens.foreground
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                        }
                        MouseArea {
                            id: mainMouse
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: { root.current = row.index; root.activate(row.index) }
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
            text: qsTr("Enter connects or opens · Escape closes")
            color: Tokens.foregroundDim
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
        }
    }

    Accessible.role: Accessible.Dialog
    Accessible.name: qsTr("Connect")
}
