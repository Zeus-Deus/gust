import QtQuick
import com.gust.app

// Settings — five choices, each one row, each a set of clickable options.
// Everything shown here is honoured: nothing is listed that Gust ignores.
// Saved at once to ~/.config/gust/settings.toml (see gust-core settings.rs).
Item {
    id: root
    anchors.fill: parent
    visible: open
    z: 60

    property bool open: false
    // Parsed DirModel.settings_json().
    property var s: ({})
    // Row the keyboard is on.
    property int current: 0

    signal changed(string key, string value)
    signal defaultRequested(bool on)
    signal keysRequested(bool on)
    signal dialogsRequested(bool on)
    signal closed()

    readonly property var rows: [
        { key: "key_preset", title: qsTr("Keys"),
          options: [["standard", qsTr("Standard")], ["vim", qsTr("Vim")],
                    ["mac", qsTr("Mac")], ["windows", qsTr("Windows")]],
          note: qsTr("Press ? for every key in the preset you pick") },
        { key: "density", title: qsTr("Density"),
          options: [["compact", qsTr("Compact")], ["normal", qsTr("Normal")], ["roomy", qsTr("Roomy")]],
          note: qsTr("Row height and spacing") },
        { key: "show_hidden", title: qsTr("Hidden files"),
          options: [["false", qsTr("Hide")], ["true", qsTr("Show")]],
          note: qsTr("When a folder opens. Ctrl+H toggles per tab") },
        { key: "sidebar", title: qsTr("Sidebar"),
          options: [["true", qsTr("Show")], ["false", qsTr("Hide")]],
          note: "" },
        { key: "restore_tabs", title: qsTr("Tabs at start"),
          options: [["false", qsTr("Fresh")], ["true", qsTr("Reopen last")]],
          note: qsTr("When Gust starts without a folder") },
        { key: "renderer", title: qsTr("Drawing"),
          options: [["auto", qsTr("Automatic")], ["software", qsTr("Software")], ["gpu", qsTr("GPU")]],
          note: qsTr("Next start. Now: ") + (root.s.renderer_now === "software" ? qsTr("software") : qsTr("GPU")) },
        { key: "default", title: qsTr("Default app"),
          options: [["true", qsTr("Use Gust")], ["false", qsTr("Previous app")]],
          note: qsTr("Opening folders and \"Show in folder\" from other apps") },
        { key: "file_dialogs", title: qsTr("Open/Save"),
          options: [["true", qsTr("Gust")], ["false", qsTr("Previous")]],
          note: qsTr("Other apps' file dialogs") },
        { key: "omarchy_keys", title: qsTr("Super+Shift+F"),
          options: [["true", qsTr("Gust")], ["false", qsTr("Omarchy default")]],
          note: root.s.is_omarchy ? qsTr("Edits ~/.config/hypr/bindings.lua, reversibly")
                                  : qsTr("Needs Omarchy's ~/.config/hypr/bindings.lua") }
    ]

    function show(json) {
        try { root.s = JSON.parse(json) } catch (e) { root.s = {} }
        root.current = 0
        root.open = true
        keys.forceActiveFocus()
        // A click that opened the panel is still being delivered; take the
        // keyboard again once it has finished, or Escape goes to the list.
        Qt.callLater(() => keys.forceActiveFocus())
    }
    function hide() { root.open = false; root.closed() }
    function valueOf(key) {
        if (key === "default") return root.s.is_default ? "true" : "false"
        if (key === "omarchy_keys") return root.s.omarchy_keys ? "true" : "false"
        if (key === "file_dialogs") return root.s.file_dialogs ? "true" : "false"
        const v = root.s[key]
        return v === undefined ? "" : String(v)
    }
    function choose(rowIndex, value) {
        const r = root.rows[rowIndex]
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
                text: qsTr("Settings")
                color: Tokens.foregroundBright
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSubtitle
            }
            Item { width: 1; height: Tokens.spaceSm }

            Repeater {
                model: root.rows
                delegate: Rectangle {
                    id: rowItem
                    required property var modelData
                    required property int index
                    width: col.width
                    height: Tokens.controlHeight + (modelData.note !== "" ? Tokens.fontCaption + Tokens.spaceXs : 0)
                    color: root.current === index ? Tokens.withAlpha(Tokens.foreground, 0.05) : "transparent"

                    Text {
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
                        x: Tokens.spaceSm
                        y: Tokens.controlHeight - Tokens.spaceXs
                        visible: modelData.note !== ""
                        text: modelData.note
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
            Text {
                width: col.width
                horizontalAlignment: Text.AlignRight
                text: qsTr("↑↓ choose · ←→ change · Escape to close")
                color: Tokens.foregroundDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontCaption
            }
        }
    }

    Accessible.role: Accessible.Dialog
    Accessible.name: qsTr("Settings")
}
