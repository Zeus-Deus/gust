import QtQuick
import com.gust.app

// The header of another app's Open or Save dialog (DESIGN §6.27): Cancel on
// the left, what the app asked in the middle, and on the right a primary
// button that names exactly what accepting returns. The Save name and the
// file type live in ChooserFooter.
Rectangle {
    id: root

    property string mode: ""          // open | open-many | open-folder | save | save-many
    property string title: ""
    /// The app's own word for the button, used as the verb. "" when it sent none.
    property string acceptLabel: ""
    /// chooser_target(): {kind: here|folder|file|files|none, name, count}.
    property var target: ({ kind: "none", name: "", count: 0 })
    /// The folder on screen, which is what `here` returns.
    property string hereName: ""
    /// Save: the name field holds something.
    property bool hasName: false
    /// A Save is waiting for the disk. Cancel still works.
    property bool busy: false
    property string error: ""

    readonly property bool active: mode !== ""
    readonly property bool canAccept: !busy
        && (mode === "save" ? hasName
            : mode === "open" || mode === "open-many" ? target.kind === "file" || target.kind === "files"
            : active)

    readonly property string heading: root.title !== "" ? root.title
        : root.mode === "save" ? qsTr("Save as")
        : root.mode === "save-many" ? qsTr("Save files")
        : root.mode === "open-folder" ? qsTr("Choose a folder")
        : root.mode === "open-many" ? qsTr("Choose files")
        : qsTr("Choose a file")

    readonly property string primaryLabel: {
        if (root.busy) return qsTr("Checking…")
        const verb = root.acceptLabel
        const kind = root.target.kind
        if (root.mode === "save") return verb !== "" ? verb : qsTr("Save")
        if (root.mode === "save-many") return verb !== "" ? verb : qsTr("Save here")
        const quoted = "“" + root.target.name + "”"
        if (root.mode === "open-folder") {
            if (kind === "folder") return (verb !== "" ? verb : qsTr("Choose")) + " " + quoted
            if (verb === "") return qsTr("Choose this folder")
            return root.hereName !== "" ? verb + " “" + root.hereName + "”" : verb
        }
        const open = verb !== "" ? verb : qsTr("Open")
        if (kind === "file") return open + " " + quoted
        if (kind === "files") return open + " " + qsTr("%1 files").arg(root.target.count)
        return open
    }

    signal accepted()
    signal cancelled()

    /// Button centres in window coordinates (qa).
    function points() {
        const c = cancelBtn.mapToItem(null, cancelBtn.width / 2, cancelBtn.height / 2)
        const k = okBtn.mapToItem(null, okBtn.width / 2, okBtn.height / 2)
        return { cancel: [Math.round(c.x), Math.round(c.y)], accept: [Math.round(k.x), Math.round(k.y)] }
    }

    readonly property real barHeight: Math.round(Tokens.controlHeight * 1.5)
    height: active ? barHeight + (error !== "" ? Tokens.fontCaption + Tokens.spaceMd : 0) : 0
    visible: active
    color: Tokens.barBackground

    Rectangle {
        anchors.bottom: parent.bottom
        width: parent.width
        height: Tokens.borderWidth
        color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
    }

    Rectangle {
        id: cancelBtn
        objectName: "chooserCancel"
        x: Tokens.spaceLg
        y: Math.round((root.barHeight - height) / 2)
        width: Math.ceil(cancelLabel.implicitWidth) + Tokens.spaceXl * 2
        height: Tokens.controlHeight
        radius: Tokens.radius
        color: activeFocus ? Tokens.withAlpha(Tokens.foreground, 0.14)
                           : cancelHover.hovered ? Tokens.withAlpha(Tokens.foreground, 0.08) : "transparent"
        border.width: Math.max(1, Tokens.borderWidth)
        border.color: activeFocus ? Tokens.accent : Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
        activeFocusOnTab: true
        HoverHandler { id: cancelHover }
        Text {
            id: cancelLabel
            textFormat: Text.PlainText
            anchors.centerIn: parent
            text: qsTr("Cancel")
            color: Tokens.foreground
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontSmall
        }
        MouseArea {
            anchors.fill: parent
            cursorShape: Qt.PointingHandCursor
            onClicked: root.cancelled()
        }
        Keys.onReturnPressed: root.cancelled()
        Keys.onEnterPressed: root.cancelled()
        Keys.onSpacePressed: root.cancelled()
        Keys.onEscapePressed: root.cancelled()
        Accessible.role: Accessible.Button
        Accessible.name: qsTr("Cancel")
    }

    // Centred on the window while there is room, and pushed aside rather
    // than under a button when the primary label is long.
    Text {
        objectName: "chooserTitle"
        textFormat: Text.PlainText
        readonly property real minX: cancelBtn.x + cancelBtn.width + Tokens.spaceXl
        readonly property real maxX: okBtn.x - Tokens.spaceXl
        width: Math.max(0, Math.min(Math.ceil(implicitWidth), maxX - minX))
        x: Math.max(minX, Math.min(maxX - width, Math.round((root.width - width) / 2)))
        y: Math.round((root.barHeight - height) / 2)
        text: root.heading
        color: Tokens.foregroundBright
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontBody
        font.bold: true
        elide: Text.ElideRight
    }

    Rectangle {
        id: okBtn
        objectName: "chooserAccept"
        // Never so wide that it reaches Cancel; a long name elides instead.
        readonly property real widest: Math.max(0, Math.min(Math.round(root.width * 0.5),
            root.width - (cancelBtn.x + cancelBtn.width) - Tokens.spaceLg * 2))
        width: Math.min(Math.ceil(okLabel.implicitWidth) + Tokens.spaceXl * 2, widest)
        x: root.width - width - Tokens.spaceLg
        y: Math.round((root.barHeight - height) / 2)
        height: Tokens.controlHeight
        radius: Tokens.radius
        color: !root.canAccept ? "transparent"
             : okHover.hovered || activeFocus ? Tokens.withAlpha(Tokens.accent, 0.35)
             : Tokens.withAlpha(Tokens.accent, 0.22)
        border.width: Math.max(1, Tokens.borderWidth)
        border.color: root.canAccept || activeFocus ? Tokens.accent
                                                     : Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
        activeFocusOnTab: true
        HoverHandler { id: okHover }
        Text {
            id: okLabel
            objectName: "chooserAcceptLabel"
            textFormat: Text.PlainText
            anchors.centerIn: parent
            width: Math.min(Math.ceil(implicitWidth), parent.width - Tokens.spaceLg * 2)
            horizontalAlignment: Text.AlignHCenter
            text: root.primaryLabel
            color: root.canAccept ? Tokens.foregroundBright : Tokens.foregroundDim
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontSmall
            font.bold: root.canAccept
            elide: Text.ElideMiddle
        }
        MouseArea {
            anchors.fill: parent
            cursorShape: root.canAccept ? Qt.PointingHandCursor : Qt.ArrowCursor
            onClicked: if (root.canAccept) root.accepted()
        }
        Keys.onReturnPressed: if (root.canAccept) root.accepted()
        Keys.onEnterPressed: if (root.canAccept) root.accepted()
        Keys.onSpacePressed: if (root.canAccept) root.accepted()
        Keys.onEscapePressed: root.cancelled()
        Accessible.role: Accessible.Button
        Accessible.name: root.primaryLabel
        Accessible.description: root.canAccept ? "" : qsTr("Not available yet")
    }

    Text {
        objectName: "chooserError"
        textFormat: Text.PlainText
        x: Tokens.spaceLg
        y: root.barHeight
        width: root.width - Tokens.spaceLg * 2
        visible: root.error !== ""
        text: root.error
        color: Tokens.danger
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontCaption
        elide: Text.ElideRight
        Accessible.role: Accessible.AlertMessage
        Accessible.name: root.error
    }

    Accessible.role: Accessible.ToolBar
    Accessible.name: root.heading
}
