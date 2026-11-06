import QtQuick
import com.gust.app

// Settings — five choices, each one row, each a set of clickable options.
// Everything shown here is honoured: nothing is listed that Gust ignores.
// A choice shows as soon as it is accepted; the preferences worker saves it
// to ~/.config/gust/settings.toml, and the status line says "Saved" only
// once that save is acknowledged (DESIGN §2.1).
Item {
    id: root
    anchors.fill: parent
    visible: open
    z: 60

    property bool open: false
    // Parsed DirModel.settings_json(), bound by the window (never assigned
    // here, so every published snapshot arrives).
    property var s: ({})
    // "loading", "saving", "saved", "unsaved" or "problem", and the reason.
    property string status: "loading"
    property string error: ""
    // An integration row whose new state the worker is still re-reading.
    property string busyKey: ""
    // Row the keyboard is on.
    property int current: 0

    readonly property bool ready: root.status !== "loading"
    readonly property bool failed: root.status === "unsaved" || root.status === "problem"
    readonly property string statusText: root.status === "unsaved" ? qsTr("Not saved: ") + root.error
                                         : root.status === "problem" ? qsTr("Problem: ") + root.error
                                         : root.status === "saving" ? qsTr("Saving…")
                                         : root.status === "saved" ? qsTr("Saved")
                                         : qsTr("Loading settings…")

    signal changed(string key, string value)
    signal retryRequested()
    signal defaultRequested(bool on)
    signal keysRequested(bool on)
    signal dialogsRequested(bool on)
    signal closed()

    readonly property var rows: [
        { key: "key_preset", title: qsTr("Keys"),
          options: [["standard", qsTr("Standard")], ["vim", qsTr("Vim")],
                    ["mac", qsTr("Mac")], ["windows", qsTr("Windows")]],
          note: qsTr("? shows every key") },
        { key: "density", title: qsTr("Density"),
          options: [["compact", qsTr("Compact")], ["normal", qsTr("Normal")], ["roomy", qsTr("Roomy")]],
          note: qsTr("Row height and spacing") },
        { key: "show_hidden", title: qsTr("Hidden files"),
          options: [["false", qsTr("Hide")], ["true", qsTr("Show")]],
          note: qsTr("Ctrl+H per tab") },
        { key: "sidebar", title: qsTr("Sidebar"),
          options: [["true", qsTr("Show")], ["false", qsTr("Hide")]],
          note: "" },
        { key: "restore_tabs", title: qsTr("Tabs at start"),
          options: [["false", qsTr("Fresh")], ["true", qsTr("Reopen last")]],
          note: "" },
        { key: "renderer", title: qsTr("Drawing"),
          options: [["auto", qsTr("Automatic")], ["software", qsTr("Software")], ["gpu", qsTr("GPU")]],
          note: "", live: "renderer" },
        { key: "default", title: qsTr("Default app"),
          options: [["true", qsTr("Use Gust")], ["false", qsTr("Previous app")]],
          note: qsTr("For folders opened by other apps") },
        { key: "file_dialogs", title: qsTr("Open/Save"),
          options: [["true", qsTr("Gust")], ["false", qsTr("Previous")]],
          note: qsTr("Other apps' file dialogs") },
        { key: "omarchy_keys", title: qsTr("Super+Shift+F"),
          options: [["true", qsTr("Gust")], ["false", qsTr("Omarchy default")]],
          note: "", live: "omarchy" }
    ]

    function show() {
        root.current = 0
        root.open = true
        keys.forceActiveFocus()
        // A click that opened the panel is still being delivered; take the
        // keyboard again once it has finished, or Escape goes to the list.
        Qt.callLater(() => keys.forceActiveFocus())
    }
    function hide() { root.open = false; root.closed() }
    /// Take the keyboard back after something shown over the panel (a
    /// confirmation) goes, on the row the user was on.
    function refocus() { if (root.open) keys.forceActiveFocus() }
    function valueOf(key) {
        if (key === "default") return root.s.is_default ? "true" : "false"
        if (key === "omarchy_keys") return root.s.omarchy_keys ? "true" : "false"
        if (key === "file_dialogs") return root.s.file_dialogs ? "true" : "false"
        const v = root.s[key]
        return v === undefined ? "" : String(v)
    }
    function choose(rowIndex, value) {
        const r = root.rows[rowIndex]
        // Nothing is accepted before the first load, and an integration
        // change runs once: no second request while its answer is pending.
        const integration = r.key === "default" || r.key === "file_dialogs" || r.key === "omarchy_keys"
        if (!root.ready || (integration && root.busyKey !== "")) return
        if (r.key === "default") root.defaultRequested(value === "true")
        else if (r.key === "omarchy_keys") root.keysRequested(value === "true")
        else if (r.key === "file_dialogs") root.dialogsRequested(value === "true")
        else root.changed(r.key, value)
    }
    function step(dir) {
        const r = root.rows[root.current]
        const cur = valueOf(r.key)
        let i = r.options.findIndex(o => o[0] === cur)
        i = Math.max(0, Math.min(r.options.length - 1, i + dir))
        if (r.options[i][0] !== cur) choose(root.current, r.options[i][0])
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
            else if (event.key === Qt.Key_Down || event.key === Qt.Key_J) { root.current = Math.min(root.rows.length - 1, root.current + 1); event.accepted = true }
            else if (event.key === Qt.Key_Up || event.key === Qt.Key_K) { root.current = Math.max(0, root.current - 1); event.accepted = true }
            else if (event.key === Qt.Key_Right || event.key === Qt.Key_L) { root.step(1); event.accepted = true }
            else if (event.key === Qt.Key_Left || event.key === Qt.Key_H) { root.step(-1); event.accepted = true }
            else if (event.key === Qt.Key_R && root.failed) { root.retryRequested(); event.accepted = true }
        }
    }

    Rectangle {
        id: panel
        anchors.centerIn: parent
        width: Math.min(parent.width - Tokens.spaceXl * 4, Math.round(Tokens.rowHeight * 26))
        height: col.implicitHeight + Tokens.spaceXl * 2
        color: Tokens.backgroundDark
        border.width: Math.max(1, Tokens.borderWidth)
        border.color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
        MouseArea { anchors.fill: parent }   // clicks inside do not close

        Column {
            id: col
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.margins: Tokens.spaceXl
            spacing: Tokens.spaceSm

            Text {
                textFormat: Text.PlainText
                text: qsTr("Settings")
                color: Tokens.foregroundBright
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSubtitle
            }
            Item { width: 1; height: Tokens.spaceSm }

            Repeater {
                model: root.open ? root.rows : []
                delegate: Rectangle {
                    id: rowItem
                    required property var modelData
                    required property int index
                    width: col.width
                    // A row marked `live` takes its note from the snapshot.
                    readonly property string note: root.busyKey === modelData.key ? qsTr("Checking…")
                        : modelData.live === "renderer"
                          ? qsTr("Next start. Now: ") + (root.s.renderer_now === "software" ? qsTr("software") : qsTr("GPU"))
                        : modelData.live === "omarchy" ? (root.s.is_omarchy ? qsTr("Reversible") : qsTr("Needs Omarchy"))
                        : modelData.note
                    height: Tokens.controlHeight + (rowItem.note !== "" ? Tokens.fontCaption + Tokens.spaceXs : 0)
                    color: root.current === index
                           ? Tokens.withAlpha(Tokens.selectedBackground, Math.max(Tokens.selectedAlpha, 0.14))
                           : "transparent"
                    Rectangle {
                        visible: root.current === rowItem.index
                        width: Math.max(2, Tokens.borderWidth * 2)
                        height: parent.height
                        color: Tokens.accent
                    }

                    Text {
                        textFormat: Text.PlainText
                        id: title
                        x: Tokens.spaceSm
                        y: Math.round((Tokens.controlHeight - height) / 2)
                        width: Math.round(Tokens.rowHeight * 5)
                        text: modelData.title
                        color: Tokens.foreground
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                    }
                    Text {
                        textFormat: Text.PlainText
                        x: Tokens.spaceSm
                        y: Tokens.controlHeight - Tokens.spaceXs
                        objectName: "settingsNote-" + rowItem.modelData.key
                        visible: rowItem.note !== ""
                        text: rowItem.note
                        color: Tokens.foregroundDim
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontCaption
                    }
                    Row {
                        anchors.right: parent.right
                        anchors.rightMargin: Tokens.spaceSm
                        y: Math.round((Tokens.controlHeight - height) / 2)
                        spacing: Tokens.spaceXs
                        Repeater {
                            model: rowItem.modelData.options
                            delegate: Rectangle {
                                required property var modelData
                                readonly property bool chosen: root.valueOf(rowItem.modelData.key) === modelData[0]
                                width: optLabel.implicitWidth + Tokens.spaceLg * 2
                                height: Tokens.controlHeight - Tokens.spaceSm
                                // Same treatment as the palette and menus: the
                                // theme's selection colour at low alpha, so the
                                // label keeps full contrast in every theme.
                                color: chosen ? Tokens.withAlpha(Tokens.selectedBackground, Math.max(Tokens.selectedAlpha, 0.16))
                                       : optMouse.containsMouse ? Tokens.withAlpha(Tokens.foreground, 0.08) : "transparent"
                                border.color: chosen ? Tokens.accent : Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                                border.width: Tokens.borderWidth
                                Text {
                                    textFormat: Text.PlainText
                                    id: optLabel
                                    anchors.centerIn: parent
                                    text: modelData[1]
                                    color: chosen ? Tokens.foregroundBright : Tokens.foreground
                                    font.bold: chosen
                                    font.family: Tokens.fontFamily
                                    font.pixelSize: Tokens.fontSmall
                                }
                                MouseArea {
                                    id: optMouse
                                    anchors.fill: parent
                                    hoverEnabled: true
                                    cursorShape: Qt.PointingHandCursor
                                    onClicked: { root.current = rowItem.index; root.choose(rowItem.index, modelData[0]) }
                                }
                                Accessible.role: Accessible.RadioButton
                                Accessible.checked: chosen
                                Accessible.name: rowItem.modelData.title + ": " + modelData[1]
                            }
                        }
                    }
                }
            }

            Item { width: 1; height: Tokens.spaceSm }
            // Persistent: an error stays until a save succeeds.
            Text {
                objectName: "settingsStatus"
                textFormat: Text.PlainText
                width: col.width
                wrapMode: Text.Wrap
                horizontalAlignment: Text.AlignRight
                text: root.statusText
                color: root.failed ? Tokens.danger : Tokens.foregroundDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontCaption
                MouseArea {
                    anchors.fill: parent
                    enabled: root.failed
                    cursorShape: root.failed ? Qt.PointingHandCursor : Qt.ArrowCursor
                    onClicked: root.retryRequested()
                }
                Accessible.role: root.failed ? Accessible.Button : Accessible.StaticText
                Accessible.name: root.failed ? root.statusText + qsTr(". Retry") : root.statusText
            }
            Text {
                textFormat: Text.PlainText
                width: col.width
                horizontalAlignment: Text.AlignRight
                text: root.failed ? qsTr("↑↓ choose · ←→ change · R retry · Esc close")
                                  : qsTr("↑↓ choose · ←→ change · Esc close")
                color: Tokens.foregroundDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontCaption
            }
        }
    }

    Accessible.role: Accessible.Dialog
    Accessible.name: qsTr("Settings")
}
