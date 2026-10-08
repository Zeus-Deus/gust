import QtQuick
import com.gust.app

// The key map (? or F1). Built from the same table the key handler reads
// (gust-core keymap.rs), so what it lists is exactly what the keys do.
// Grouped, in columns that fit the window; Escape or any click closes it.
Item {
    id: root
    anchors.fill: parent
    visible: open
    z: 60

    property bool open: false
    property string preset: "standard"
    property var rows: []
    signal closed()

    // Groups placed into three columns, each group going to the shortest
    // column so far: no tall column beside a mostly empty one.
    readonly property var columns: {
        // Help is this panel itself; the subtitle says how it opens.
        const order = ["Go", "Files", "Select", "View", "Find"]
        const cols = [[], [], []], h = [0, 0, 0]
        for (const g of order) {
            const items = root.rows.filter(r => r.group === g)
            if (items.length === 0) continue
            let k = 0
            for (let i = 1; i < 3; i++) if (h[i] < h[k]) k = i
            cols[k].push({ name: g, items: items })
            h[k] += items.length + 2
        }
        // Each column's key width fits its own longest key, so nothing wraps
        // and a short key ("/") is not stranded far from what it does.
        return cols.map(c => {
            let longest = 1
            for (const g of c) for (const it of g.items) longest = Math.max(longest, it.keys.length)
            return { groups: c, keyChars: longest }
        })
    }

    function show(json, presetName) {
        try { root.rows = JSON.parse(json) } catch (e) { root.rows = [] }
        root.preset = presetName
        root.open = true
        Qt.callLater(() => keys.forceActiveFocus())
    }
    function hide() { root.open = false; root.closed() }

    Rectangle {
        anchors.fill: parent
        color: Tokens.withAlpha(Tokens.backgroundDark, 0.96)
        MouseArea { anchors.fill: parent; onClicked: root.hide() }
    }

    Item {
        id: keys
        focus: root.open
        Keys.onPressed: (event) => { root.hide(); event.accepted = true }
    }

    Rectangle {
        id: panel
        objectName: "keyMapPanel"
        anchors.centerIn: parent
        // Sized to its content, capped by the window: never a small island.
        width: Math.min(parent.width - Tokens.spaceXl * 4, flow.implicitWidth + (Tokens.spaceXxl + Tokens.spaceSm) * 2)
        height: Math.min(parent.height - Tokens.spaceXl * 4, content.implicitHeight + (Tokens.spaceXxl + Tokens.spaceSm) * 2)
        color: Tokens.background
        border.width: Math.max(2, Tokens.borderWidth * 2)
        border.color: Tokens.accent
        clip: true
        MouseArea { anchors.fill: parent; onClicked: root.hide() }

        Column {
            id: content
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.margins: Tokens.spaceXxl + Tokens.spaceSm
            spacing: Tokens.spaceXl

            Row {
                spacing: Tokens.spaceLg
                Text {
                    textFormat: Text.PlainText
                    text: qsTr("Keys")
                    color: Tokens.foregroundBright
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontTitle
                    font.bold: true
                }
                Text {
                    textFormat: Text.PlainText
                    anchors.baseline: parent.children[0].baseline
                    text: qsTr("preset: ") + root.preset + qsTr("  ·  ? or F1 opens this  ·  Settings changes the preset")
                    color: Tokens.foregroundDim
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontSmall
                }
            }

            Row {
                id: flow
                spacing: Tokens.spaceXxl * 3
                // Keys are short; a long one ("PgUp PgDn Home End") wraps
                // rather than pushing every description far from its key.
                readonly property real charWidth: keyMetrics.averageCharacterWidth
                readonly property real doesWidth: Math.round(Tokens.fontSmall * 15)
                FontMetrics {
                    id: keyMetrics
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontSmall
                    font.bold: true
                }

                Repeater {
                    model: root.columns
                    delegate: Column {
                        id: column
                        required property var modelData
                        readonly property real keyWidth: Math.ceil(flow.charWidth * (modelData.keyChars + 1))
                        spacing: Tokens.spaceXl
                        Repeater {
                            model: column.modelData.groups
                            delegate: Column {
                                required property var modelData
                                spacing: Tokens.spaceXs
                                Text {
                                    textFormat: Text.PlainText
                                    text: modelData.name
                                    font.capitalization: Font.AllUppercase
                                    color: Tokens.accent
                                    font.family: Tokens.fontFamily
                                    font.pixelSize: Tokens.fontCaption
                                    font.bold: true
                                    font.letterSpacing: 1
                                    bottomPadding: Tokens.spaceXs
                                }
                                Repeater {
                                    model: modelData.items
                                    delegate: Row {
                                        required property var modelData
                                        spacing: Tokens.spaceLg
                                        // The key is what the eye looks for:
                                        // brightest and bold. What it does is
                                        // secondary text.
                                        Text {
                                            textFormat: Text.PlainText
                                            width: column.keyWidth
                                            text: modelData.keys
                                            color: Tokens.foregroundBright
                                            font.family: Tokens.fontFamily
                                            font.pixelSize: Tokens.fontSmall
                                            font.bold: true
                                            wrapMode: Text.WordWrap
                                        }
                                        Text {
                                            textFormat: Text.PlainText
                                            width: flow.doesWidth
                                            text: modelData.does
                                            color: Tokens.foreground
                                            font.family: Tokens.fontFamily
                                            font.pixelSize: Tokens.fontSmall
                                            wrapMode: Text.WordWrap
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
