import QtQuick
import com.gust.app

// Rename many files at once by editing their names (DESIGN §6.16).
//
// Every selected name is one line. Edit any of them, or use Find/Replace
// across all of them; each line is checked as you type and a bad one is
// marked where it is. Nothing is renamed until Rename is pressed, and if
// any line has a problem nothing is renamed at all. Swaps just work, and one
// Ctrl+Z puts every name back.
Rectangle {
    id: root
    /// The model whose selection is being renamed, and the cursor row the
    /// selection was resolved against.
    property var model: null
    property int row: -1
    /// The names as they were, one per line, in screen order.
    property var originals: []
    /// The latest check: { ok, summary, changing, lines: [msg|null] }.
    property var check: ({ ok: false, summary: "", changing: 0, lines: [] })
    readonly property bool open: visible

    signal accepted(string names)
    signal closed()

    anchors.fill: parent
    color: Tokens.withAlpha(Tokens.background, 0.72)
    visible: false
    z: 100

    function openWith(m, r, text) {
        model = m
        row = r
        originals = text.split("\n")
        editor.text = text
        findInput.text = ""
        replaceInput.text = ""
        visible = true
        recheck()
        editor.forceActiveFocus()
        editor.cursorPosition = 0
    }
    function close() {
        visible = false
        model = null
        closed()
    }
    /// The first marked line and its problem, refreshed by recheck() (a
    /// binding over a function call would evaluate once).
    property int firstBadLine: -1
    property string firstBadText: ""

    function recheck() {
        if (!model) return
        try {
            check = JSON.parse(model.bulk_check(row, editor.text))
        } catch (e) {
            check = { ok: false, summary: qsTr("Those names could not be read"), changing: 0, lines: [] }
        }
        const ls = check.lines || []
        firstBadLine = -1
        firstBadText = ""
        for (let i = 0; i < ls.length; i++) {
            if (ls[i]) { firstBadLine = i; firstBadText = ls[i]; break }
        }
    }
    function confirm() {
        recheck()
        if (!check.ok) return
        accepted(editor.text)
    }
    /// Find/Replace: literal text, across every line, in one step (so one
    /// Ctrl+Z in the field undoes it).
    function replaceAll() {
        const f = findInput.text
        if (f === "") return
        editor.text = editor.text.split("\n").map(l => l.split(f).join(replaceInput.text)).join("\n")
        recheck()
    }
    /// For qa: how many lines are marked.
    function badLines() {
        return (check.lines || []).filter(l => !!l).length
    }

    Timer {
        id: recheckSoon
        interval: 90
        repeat: false
        onTriggered: root.recheck()
    }

    MouseArea {
        anchors.fill: parent
        onClicked: root.close()
    }

    FontMetrics {
        id: fm
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontBody
    }

    Rectangle {
        id: panel
        anchors.centerIn: parent
        width: Math.min(parent.width * 0.86, Math.round(720 * Tokens.spacingScale))
        // The list is clamped to what fits (below), so the panel is simply
        // as tall as its content: no size depends on itself.
        height: content.implicitHeight + Tokens.spaceXxl * 2
        color: Tokens.menuBackground
        border.width: Math.max(1, Tokens.borderWidth)
        border.color: Tokens.border
        radius: Tokens.radius

        MouseArea { anchors.fill: parent } // swallow clicks

        Column {
            id: content
            anchors.top: parent.top
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.margins: Tokens.spaceXxl
            spacing: Tokens.spaceXl

            Text {
                id: title
                textFormat: Text.PlainText
                width: parent.width
                text: qsTr("Rename %1 items").arg(root.originals.length)
                color: Tokens.foregroundBright
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSubtitle
                font.bold: true
            }

            // Find / Replace across every line.
            Row {
                id: findRow
                spacing: Tokens.spaceLg
                width: parent.width
                readonly property real labelsWidth: findLabelMetrics.advanceWidth + replaceLabelMetrics.advanceWidth
                TextMetrics { id: findLabelMetrics; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontSmall; text: qsTr("Find") }
                TextMetrics { id: replaceLabelMetrics; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontSmall; text: qsTr("Replace with") }
                height: Tokens.controlHeight
                Repeater {
                    model: [
                        { label: qsTr("Find"), which: 0 },
                        { label: qsTr("Replace with"), which: 1 }
                    ]
                    delegate: Row {
                        spacing: Tokens.spaceMd
                        Text {
                            textFormat: Text.PlainText
                            anchors.verticalCenter: parent.verticalCenter
                            text: modelData.label
                            color: Tokens.foregroundDim
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                        }
                        Rectangle {
                            // Two labels, two fields and the button share the
                            // row; the fields take what the rest leaves.
                            width: Math.max(fm.averageCharacterWidth * 6,
                                            Math.floor((findRow.width - applyButton.width
                                                        - findRow.labelsWidth
                                                        - findRow.spacing * 2
                                                        - Tokens.spaceMd * 2) / 2))
                            height: Tokens.controlHeight
                            radius: Tokens.radius
                            color: Tokens.backgroundDark
                            border.width: Math.max(1, Tokens.borderWidth)
                            border.color: (modelData.which === 0 ? findInput : replaceInput).activeFocus
                                          ? Tokens.accent : Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                            Component.onCompleted: {
                                const input = modelData.which === 0 ? findInput : replaceInput
                                input.parent = this
                            }
                        }
                    }
                }
                Rectangle {
                    id: applyButton
                    width: applyLabel.implicitWidth + Tokens.spaceXxl
                    height: Tokens.controlHeight
                    radius: Tokens.radius
                    color: applyHover.containsMouse ? Tokens.withAlpha(Tokens.accent, 0.2) : "transparent"
                    border.width: Math.max(1, Tokens.borderWidth)
                    border.color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                    opacity: findInput.text !== "" ? 1.0 : 0.4
                    Text {
                        id: applyLabel
                        textFormat: Text.PlainText
                        anchors.centerIn: parent
                        text: qsTr("Replace all")
                        color: Tokens.foreground
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                    }
                    MouseArea {
                        id: applyHover
                        anchors.fill: parent
                        hoverEnabled: true
                        onClicked: root.replaceAll()
                    }
                    Accessible.role: Accessible.Button
                    Accessible.name: qsTr("Replace all")
                }
            }

            // Was → will be, one line per file. The right side is editable.
            Rectangle {
                id: listBox
                width: parent.width
                // As tall as the names, but never taller than the window
                // leaves once the rest of the card is placed.
                height: Math.max(fm.lineSpacing * 2 + Tokens.spaceLg * 2,
                                 Math.min(fm.lineSpacing * Math.max(3, root.originals.length) + Tokens.spaceLg * 2,
                                          root.height * 0.86 - Tokens.spaceXxl * 2 - title.height
                                          - findRow.height - footer.height - buttons.height
                                          - content.spacing * 4))
                color: Tokens.backgroundDark
                radius: Tokens.radius
                border.width: Math.max(1, Tokens.borderWidth)
                border.color: editor.activeFocus ? Tokens.accent : Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                clip: true

                Flickable {
                    id: flick
                    anchors.fill: parent
                    anchors.margins: Tokens.spaceLg
                    contentWidth: width
                    contentHeight: Math.max(editor.contentHeight, fm.lineSpacing * root.originals.length)
                    boundsBehavior: Flickable.StopAtBounds
                    clip: true
                    function ensureVisible(r) {
                        if (contentY >= r.y) contentY = r.y
                        else if (contentY + height <= r.y + r.height) contentY = r.y + r.height - height
                    }

                    Column {
                        id: olds
                        width: Math.round(flick.width * 0.42)
                        Repeater {
                            model: root.originals
                            delegate: Row {
                                height: fm.lineSpacing
                                spacing: Tokens.spaceMd
                                readonly property string problem: (root.check.lines || [])[index] || ""
                                Text {
                                    textFormat: Text.PlainText
                                    width: fm.averageCharacterWidth * 2
                                    height: fm.lineSpacing
                                    verticalAlignment: Text.AlignVCenter
                                    text: parent.problem !== "" ? "\u2715" : ""
                                    color: Tokens.danger
                                    font.family: Tokens.fontFamily
                                    font.pixelSize: Tokens.fontBody
                                }
                                Text {
                                    textFormat: Text.PlainText
                                    width: olds.width - fm.averageCharacterWidth * 4
                                    height: fm.lineSpacing
                                    verticalAlignment: Text.AlignVCenter
                                    text: modelData
                                    elide: Text.ElideMiddle
                                    color: parent.problem !== "" ? Tokens.danger : Tokens.foregroundDim
                                    font.family: Tokens.fontFamily
                                    font.pixelSize: Tokens.fontBody
                                }
                            }
                        }
                    }
                    Column {
                        x: olds.width
                        Repeater {
                            model: root.originals.length
                            delegate: Text {
                                width: fm.averageCharacterWidth * 3
                                height: fm.lineSpacing
                                verticalAlignment: Text.AlignVCenter
                                textFormat: Text.PlainText
                                text: "\u2192"
                                color: Tokens.foregroundDim
                                font.family: Tokens.fontFamily
                                font.pixelSize: Tokens.fontBody
                            }
                        }
                    }
                    TextEdit {
                        id: editor
                        objectName: "renameEditor"
                        x: olds.width + fm.averageCharacterWidth * 3
                        width: flick.width - x
                        textFormat: TextEdit.PlainText
                        wrapMode: TextEdit.NoWrap
                        selectByMouse: true
                        persistentSelection: true
                        color: Tokens.foregroundBright
                        selectionColor: Tokens.withAlpha(Tokens.accent, 0.4)
                        selectedTextColor: Tokens.foregroundBright
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                        onTextChanged: recheckSoon.restart()
                        onCursorRectangleChanged: flick.ensureVisible(cursorRectangle)
                        Keys.onPressed: (e) => {
                            if ((e.key === Qt.Key_Return || e.key === Qt.Key_Enter)
                                    && (e.modifiers & Qt.ControlModifier)) {
                                root.confirm(); e.accepted = true; return
                            }
                            // A new line would pair names with the wrong
                            // files: Enter renames instead, like F2 does.
                            if (e.key === Qt.Key_Return || e.key === Qt.Key_Enter) {
                                root.confirm(); e.accepted = true; return
                            }
                            if (e.key === Qt.Key_Escape) { root.close(); e.accepted = true; return }
                            if (e.key === Qt.Key_Tab) { findInput.forceActiveFocus(); e.accepted = true; return }
                        }
                        Accessible.role: Accessible.EditableText
                        Accessible.name: qsTr("New names, one per line")
                    }
                }
            }

            Text {
                id: footer
                textFormat: Text.PlainText
                width: parent.width
                wrapMode: Text.Wrap
                text: root.firstBadLine >= 0
                      ? qsTr("Line %1: %2").arg(root.firstBadLine + 1).arg(root.firstBadText)
                        + "  \u00b7  " + root.check.summary
                      : root.check.summary
                color: root.check.ok ? Tokens.foregroundDim : Tokens.danger
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
            }

            Row {
                id: buttons
                anchors.right: parent.right
                height: Tokens.controlHeight
                spacing: Tokens.spaceLg
                Text {
                    textFormat: Text.PlainText
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("Enter rename · Esc cancel")
                    color: Tokens.foregroundDim
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontCaption
                }
                Repeater {
                    model: [
                        { id: "cancel", label: qsTr("Cancel") },
                        { id: "rename", label: qsTr("Rename") }
                    ]
                    delegate: Rectangle {
                        id: btn
                        objectName: "rename-" + modelData.id
                        readonly property bool live: modelData.id === "cancel" || root.check.ok
                        width: lbl.implicitWidth + Tokens.spaceXxl
                        height: Tokens.controlHeight
                        radius: Tokens.radius
                        opacity: live ? 1.0 : 0.4
                        color: hover.containsMouse && live ? Tokens.withAlpha(Tokens.accent, 0.2) : "transparent"
                        border.width: Math.max(1, Tokens.borderWidth)
                        border.color: modelData.id === "rename" ? Tokens.accent
                                      : Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                        Component.onCompleted: if (modelData.id === "rename") root.renameButton = btn
                                               else root.cancelButton = btn
                        Text {
                            id: lbl
                            textFormat: Text.PlainText
                            anchors.centerIn: parent
                            text: modelData.label
                            color: modelData.id === "rename" ? Tokens.foregroundBright : Tokens.foreground
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontBody
                        }
                        MouseArea {
                            id: hover
                            anchors.fill: parent
                            hoverEnabled: true
                            enabled: btn.live
                            onClicked: modelData.id === "rename" ? root.confirm() : root.close()
                        }
                        Accessible.role: Accessible.Button
                        Accessible.name: modelData.label
                    }
                }
            }
        }
    }

    // The two fields live inside the Find/Replace boxes (re-parented there).
    TextInput {
        id: findInput
        objectName: "renameFind"
        anchors.fill: parent
        anchors.leftMargin: Tokens.spaceMd
        anchors.rightMargin: Tokens.spaceMd
        verticalAlignment: TextInput.AlignVCenter
        clip: true
        color: Tokens.foregroundBright
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontBody
        selectByMouse: true
        Keys.onReturnPressed: root.replaceAll()
        Keys.onEnterPressed: root.replaceAll()
        Keys.onEscapePressed: root.close()
        Keys.onTabPressed: replaceInput.forceActiveFocus()
        Accessible.role: Accessible.EditableText
        Accessible.name: qsTr("Find")
    }
    TextInput {
        id: replaceInput
        objectName: "renameReplace"
        anchors.fill: parent
        anchors.leftMargin: Tokens.spaceMd
        anchors.rightMargin: Tokens.spaceMd
        verticalAlignment: TextInput.AlignVCenter
        clip: true
        color: Tokens.foregroundBright
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontBody
        selectByMouse: true
        Keys.onReturnPressed: root.replaceAll()
        Keys.onEnterPressed: root.replaceAll()
        Keys.onEscapePressed: root.close()
        Keys.onTabPressed: editor.forceActiveFocus()
        Accessible.role: Accessible.EditableText
        Accessible.name: qsTr("Replace with")
    }

    property Item renameButton: null
    property Item cancelButton: null
    /// Button centres in window coordinates (qa clicks them).
    function buttonPoints() {
        const out = {}
        for (const [k, b] of [["rename", renameButton], ["cancel", cancelButton]]) {
            if (!b) continue
            const p = b.mapToItem(null, b.width / 2, b.height / 2)
            out[k] = [Math.round(p.x), Math.round(p.y)]
        }
        return out
    }

    Accessible.role: Accessible.Dialog
    Accessible.name: title.text
}
