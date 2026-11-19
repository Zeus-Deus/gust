import QtQuick
import com.gust.app

// One disclosure for the scrolling strip. A very long path stays under the
// pointer while the user moves into the tip to scroll through its lines.
Rectangle {
    id: root
    required property real availableHeight
    required property real tabCenter
    property Item tab: null
    readonly property alias text: pathText.text
    readonly property alias atEnd: viewport.atYEnd
    onTabChanged: if (!tab) visible = false

    function inspect(item) {
        dismiss()
        tab = item
        showDelay.start()
    }
    function leave(item) {
        if (tab !== item) return
        if (visible && viewport.interactive) hideDelay.restart()
        else dismiss()
    }
    function dismiss() {
        showDelay.stop()
        hideDelay.stop()
        visible = false
        tab = null
    }

    visible: false
    x: Math.max(0, Math.min(tabCenter - width / 2, parent.width - width))
    width: Math.min(parent.width, pathText.implicitWidth + Tokens.spaceLg * 2)
    height: Math.min(availableHeight, pathText.implicitHeight + Tokens.spaceSm * 2)
    radius: Tokens.radius
    color: Tokens.menuBackground
    border.width: Math.max(1, Tokens.borderWidth)
    border.color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)

    Timer {
        id: showDelay
        interval: 600
        onTriggered: {
            viewport.contentY = 0
            root.visible = !!root.tab && root.tab.visible && root.text !== ""
        }
    }
    Timer {
        id: hideDelay
        interval: 250
        onTriggered: if (!tipHover.hovered) root.dismiss()
    }
    HoverHandler {
        id: tipHover
        onHoveredChanged: if (!hovered && root.visible) hideDelay.restart()
    }
    Flickable {
        id: viewport
        anchors.fill: parent
        anchors.margins: Tokens.spaceSm
        anchors.leftMargin: Tokens.spaceLg
        anchors.rightMargin: Tokens.spaceLg
        clip: true
        contentHeight: pathText.height
        interactive: contentHeight > height
        flickableDirection: Flickable.VerticalFlick
        boundsBehavior: Flickable.StopAtBounds
        Text {
            id: pathText
            width: viewport.width
            text: root.tab ? root.tab.fullPath : ""
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: Tokens.foreground
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
        }
    }
}
