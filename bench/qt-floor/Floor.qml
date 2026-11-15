import QtQuick

Rectangle {
    color: floorTokens.background
    ListView {
        anchors.fill: parent
        model: floorRows
        clip: true
        delegate: Text {
            required property int index
            width: ListView.view.width
            height: floorTokens.rowHeight
            text: "file-" + index.toString().padStart(6, "0") + ".txt"
            color: floorTokens.foreground
            font.family: floorTokens.fontFamily
            font.pixelSize: floorTokens.fontSize
            verticalAlignment: Text.AlignVCenter
        }
        Component.onCompleted: console.info("FLOOR_MODEL", count)
    }
}
