import QtQuick
import com.gust.app

// The foot of another app's Open or Save dialog: the name to save as, how
// many files a multiple Save places, and the file type. Absent when the
// dialog has none of those, so a plain Open gets its height back.
Rectangle {
    id: root

    property string mode: ""
    property alias name: nameField.text
    /// A Save is waiting for the disk: the name it asked about must not change.
    property bool busy: false
    /// save-many: how many files land in this folder.
    property int fileCount: 0
    /// The app's file type names, and which one is showing.
    property var filters: []
    property int filterIndex: 0

    readonly property bool saving: mode === "save"
    readonly property bool nameFocused: nameField.activeFocus
    readonly property bool hasName: /\S/.test(root.name)
    readonly property string filterName: root.filters[root.filterIndex] || ""
    readonly property bool shown: mode === "save" || mode === "save-many" || filters.length > 0

    signal accepted()
    signal cancelled()
    signal filterMenuRequested()
    /// focusName() in a dialog with no name field: the keyboard goes home.
    signal listWanted()

    function focusName() {
        if (!root.saving) { root.listWanted(); return }
        nameField.forceActiveFocus()
        nameField.selectAll()
    }
    /// Where the file type menu opens, in window coordinates.
    function filterPoint() { return filterBtn.mapToItem(null, 0, -Tokens.spaceXs) }
    /// Centres in window coordinates (qa).
    function points() {
        const n = nameBox.mapToItem(null, nameBox.width / 2, nameBox.height / 2)
        const f = filterBtn.mapToItem(null, filterBtn.width / 2, filterBtn.height / 2)
        return { name: [Math.round(n.x), Math.round(n.y)], filter: [Math.round(f.x), Math.round(f.y)] }
    }

    height: shown ? Math.round(Tokens.controlHeight * 1.5) : 0
    visible: shown
    color: Tokens.barBackground

    Rectangle {
        anchors.top: parent.top
        width: parent.width
        height: Tokens.borderWidth
        color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
    }

    Text {
        id: nameCaption
        textFormat: Text.PlainText
        visible: root.saving
        x: Tokens.spaceLg
        anchors.verticalCenter: parent.verticalCenter
        text: qsTr("Name")
        color: Tokens.foregroundDim
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontSmall
    }

    Rectangle {
        id: nameBox
        visible: root.saving
        anchors.left: nameCaption.right
        anchors.leftMargin: Tokens.spaceLg
        anchors.right: typeArea.left
        anchors.rightMargin: Tokens.spaceLg
        anchors.verticalCenter: parent.verticalCenter
        height: Tokens.controlHeight
        radius: Tokens.radius
        color: Tokens.background
        border.width: Math.max(1, Tokens.borderWidth)
        border.color: nameField.activeFocus ? Tokens.accent
                                            : Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
        TextInput {
            id: nameField
            objectName: "chooserName"
            anchors.fill: parent
            anchors.leftMargin: Tokens.spaceMd
            anchors.rightMargin: Tokens.spaceMd
            verticalAlignment: TextInput.AlignVCenter
            color: Tokens.foreground
            selectionColor: Tokens.accent
            selectedTextColor: Tokens.background
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontBody
            clip: true
            readOnly: root.busy
            activeFocusOnTab: true
            Keys.onReturnPressed: root.accepted()
            Keys.onEnterPressed: root.accepted()
            Keys.onEscapePressed: root.cancelled()
            Accessible.role: Accessible.EditableText
            Accessible.name: qsTr("File name")
        }
    }

    Text {
        objectName: "chooserFileCount"
        textFormat: Text.PlainText
        visible: root.mode === "save-many"
        x: Tokens.spaceLg
        width: typeArea.x - x - Tokens.spaceLg
        anchors.verticalCenter: parent.verticalCenter
        text: root.fileCount === 1 ? qsTr("1 file goes in this folder")
                                   : qsTr("%1 files go in this folder").arg(root.fileCount)
        color: Tokens.foreground
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontSmall
        elide: Text.ElideRight
    }

    // The right end: a button for a choice of types, a quiet line for one,
    // nothing at all for none.
    Item {
        id: typeArea
        anchors.right: parent.right
        anchors.rightMargin: Tokens.spaceLg
        anchors.verticalCenter: parent.verticalCenter
        width: root.filters.length > 1 ? filterBtn.width
             : root.filters.length === 1 ? typeCaption.width : 0
        height: Tokens.controlHeight

        Rectangle {
            id: filterBtn
            objectName: "chooserFilter"
            visible: root.filters.length > 1
            anchors.right: parent.right
            width: Math.min(Math.ceil(filterLabel.implicitWidth) + Tokens.spaceLg * 2,
                            Math.round(root.width * 0.4))
            height: Tokens.controlHeight
            radius: Tokens.radius
            color: activeFocus ? Tokens.withAlpha(Tokens.foreground, 0.14)
                               : filterHover.hovered ? Tokens.withAlpha(Tokens.foreground, 0.08) : "transparent"
            border.width: Math.max(1, Tokens.borderWidth)
            border.color: activeFocus ? Tokens.accent : Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
            activeFocusOnTab: true
            HoverHandler { id: filterHover }
            Text {
                id: filterLabel
                objectName: "chooserFilterLabel"
                textFormat: Text.PlainText
                anchors.centerIn: parent
                width: Math.min(Math.ceil(implicitWidth), parent.width - Tokens.spaceLg * 2)
                text: root.filterName + " ▾"
                color: Tokens.foreground
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
                elide: Text.ElideMiddle
            }
            MouseArea {
                anchors.fill: parent
                cursorShape: Qt.PointingHandCursor
                onClicked: root.filterMenuRequested()
            }
            Keys.onReturnPressed: root.filterMenuRequested()
            Keys.onEnterPressed: root.filterMenuRequested()
            Keys.onSpacePressed: root.filterMenuRequested()
            Keys.onDownPressed: root.filterMenuRequested()
            Keys.onEscapePressed: root.cancelled()
            Accessible.role: Accessible.ComboBox
            Accessible.name: qsTr("File type: %1").arg(root.filterName)
        }

        Text {
            id: typeCaption
            objectName: "chooserFilterCaption"
            textFormat: Text.PlainText
            visible: root.filters.length === 1
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            width: Math.min(Math.ceil(implicitWidth), Math.round(root.width * 0.4))
            text: root.filterName
            color: Tokens.foregroundDim
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
            elide: Text.ElideMiddle
            Accessible.role: Accessible.StaticText
            Accessible.name: qsTr("File type: %1").arg(root.filterName)
        }
    }
}
