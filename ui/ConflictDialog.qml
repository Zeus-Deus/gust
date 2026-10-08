import QtQuick
import com.gust.app

// "This name is already taken" — asked BEFORE anything is written.
//
// Three answers, never a silent default: Replace (the old item goes to the
// Trash first, so Undo brings it back), Keep both (the incoming item gets a
// free "(copy)" name) and Skip. With several clashes, "apply to all" answers
// the rest in one go. Escape or clicking outside cancels the whole transfer
// and nothing has been touched.
Rectangle {
    id: root
    /// The clash being shown, as parsed from DirModel.conflict_json().
    property var clash: null
    property bool applyAll: false
    signal answered(string choice, bool all)
    signal cancelled()
    signal closed()

    anchors.fill: parent
    color: Tokens.withAlpha(Tokens.background, 0.72)
    visible: false
    z: 100

    function show(c) {
        clash = c
        visible = true
        // Keep both is the one answer that can never lose anything.
        keepButton.forceActiveFocus()
    }
    function close() {
        visible = false
        clash = null
        applyAll = false
        closed()
    }
    function pick(choice) {
        if (choice === "replace" && !(clash && clash.canReplace)) return
        answered(choice, applyAll)
    }
    function cancel() {
        cancelled()
        close()
    }

    readonly property int remaining: clash ? clash.count - clash.index : 0

    MouseArea {
        anchors.fill: parent
        onClicked: root.cancel()
    }

    Rectangle {
        anchors.centerIn: parent
        width: Math.min(parent.width * 0.8, Math.round(460 * Tokens.spacingScale))
        height: content.implicitHeight + Tokens.spaceXxl * 2
        color: Tokens.menuBackground
        border.width: Math.max(1, Tokens.borderWidth)
        border.color: Tokens.border
        radius: Tokens.radius

        MouseArea { anchors.fill: parent } // swallow clicks

        Column {
            id: content
            anchors.centerIn: parent
            width: parent.width - Tokens.spaceXxl * 2
            spacing: Tokens.spaceXl

            Text {
                textFormat: Text.PlainText
                width: parent.width
                objectName: "conflictTitle"
                text: root.clash
                      ? qsTr("\u201c%1\u201d already exists").arg(root.clash.name)
                      : ""
                color: Tokens.foregroundBright
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSubtitle
                font.bold: true
                elide: Text.ElideMiddle
            }

            // Old and new, side by side: the two facts the choice turns on.
            Grid {
                columns: 3
                columnSpacing: Tokens.spaceXl
                rowSpacing: Tokens.spaceSm
                Repeater {
                    model: root.clash ? [
                        qsTr("There now"), root.clash.existingSize, root.clash.existingTime,
                        root.clash.verb === "move" ? qsTr("Moving in") : qsTr("Copying in"),
                        root.clash.incomingSize, root.clash.incomingTime
                    ] : []
                    Text {
                        textFormat: Text.PlainText
                        text: modelData
                        color: index % 3 === 0 ? Tokens.foregroundDim : Tokens.foreground
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                    }
                }
            }

            Text {
                textFormat: Text.PlainText
                width: parent.width
                visible: root.clash && !root.clash.canReplace
                text: qsTr("Replace is off here: this place has no Trash, so the old item could not be brought back.")
                color: Tokens.foregroundDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
                wrapMode: Text.Wrap
            }

            // Apply to all — only when there is more than one question.
            Item {
                visible: root.remaining > 1
                objectName: "conflictApplyAll"
                width: allRow.implicitWidth
                height: allRow.implicitHeight
                Row {
                    id: allRow
                    spacing: Tokens.spaceMd
                    Rectangle {
                        width: Tokens.fontBody; height: width
                        anchors.verticalCenter: parent.verticalCenter
                        radius: Math.max(2, Tokens.radius / 2)
                        color: root.applyAll ? Tokens.accent : "transparent"
                        border.width: Math.max(1, Tokens.borderWidth)
                        border.color: root.applyAll ? Tokens.accent : Tokens.foregroundDim
                        Text {
                            textFormat: Text.PlainText
                            anchors.centerIn: parent
                            visible: root.applyAll
                            text: "\u2713"
                            color: Tokens.background
                            font.pixelSize: Tokens.fontSmall
                            font.bold: true
                        }
                    }
                    Text {
                        textFormat: Text.PlainText
                        text: qsTr("Do this for all %1 clashes").arg(root.remaining)
                        color: Tokens.foreground
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                    }
                }
                // The box and its label are one click target.
                MouseArea {
                    anchors.fill: parent
                    onClicked: root.applyAll = !root.applyAll
                }
            }

            Row {
                anchors.right: parent.right
                spacing: Tokens.spaceLg

                Repeater {
                    model: [
                        { id: "cancel", label: qsTr("Cancel") },
                        { id: "skip", label: qsTr("Skip") },
                        { id: "replace", label: qsTr("Replace") },
                        { id: "keep-both", label: qsTr("Keep both") }
                    ]
                    delegate: Rectangle {
                        id: btn
                        readonly property bool enabledChoice:
                            modelData.id !== "replace" || (root.clash && root.clash.canReplace)
                        objectName: "conflict-" + modelData.id
                        width: lbl.implicitWidth + Tokens.spaceXxl
                        height: Tokens.controlHeight
                        radius: Tokens.radius
                        opacity: enabledChoice ? 1.0 : 0.4
                        color: activeFocus || hover.containsMouse
                               ? Tokens.withAlpha(Tokens.accent, 0.2) : "transparent"
                        border.width: Math.max(1, Tokens.borderWidth)
                        border.color: activeFocus ? Tokens.accent
                                      : modelData.id === "keep-both" ? Tokens.accent
                                      : Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                        activeFocusOnTab: enabledChoice
                        Component.onCompleted: if (modelData.id === "keep-both") keepButton.target = btn

                        Text {
                            textFormat: Text.PlainText
                            id: lbl
                            anchors.centerIn: parent
                            text: modelData.label
                            color: modelData.id === "keep-both" ? Tokens.foregroundBright : Tokens.foreground
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontBody
                        }
                        MouseArea {
                            id: hover
                            anchors.fill: parent
                            hoverEnabled: true
                            enabled: btn.enabledChoice
                            onClicked: modelData.id === "cancel" ? root.cancel() : root.pick(modelData.id)
                        }
                        Keys.onReturnPressed: modelData.id === "cancel" ? root.cancel() : root.pick(modelData.id)
                        Keys.onSpacePressed: modelData.id === "cancel" ? root.cancel() : root.pick(modelData.id)
                        Accessible.role: Accessible.Button
                        Accessible.name: modelData.label
                    }
                }
            }

            Text {
                textFormat: Text.PlainText
                anchors.right: parent.right
                text: qsTr("R replace · K keep both · S skip · A all · Esc cancel")
                color: Tokens.foregroundDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontCaption
            }
        }
    }

    // Holds the Keep both button so show() can focus it.
    QtObject {
        id: keepButton
        property Item target: null
        function forceActiveFocus() { if (target) target.forceActiveFocus(); else root.forceActiveFocus() }
    }

    Keys.onEscapePressed: root.cancel()
    Keys.onPressed: (e) => {
        if (e.key === Qt.Key_R) { root.pick("replace"); e.accepted = true }
        else if (e.key === Qt.Key_K) { root.pick("keep-both"); e.accepted = true }
        else if (e.key === Qt.Key_S) { root.pick("skip"); e.accepted = true }
        else if (e.key === Qt.Key_A && root.remaining > 1) { root.applyAll = !root.applyAll; e.accepted = true }
    }

    Accessible.role: Accessible.Dialog
    Accessible.name: clash ? clash.name + " already exists" : ""
}
