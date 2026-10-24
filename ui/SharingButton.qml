import QtQuick
import com.gust.app

Rectangle {
    id: root
    property string text: ""
    signal clicked()
    signal focusStep(int direction)
    width: Math.min(implicitWidth, parent ? parent.width : implicitWidth)
    implicitWidth: label.implicitWidth + Tokens.spaceLg * 2
    height: Tokens.controlHeight
    color: enabled && pointer.containsMouse ? Tokens.withAlpha(Tokens.accent, 0.18) : "transparent"
    border.color: activeFocus ? Tokens.foregroundBright : Tokens.accent
    border.width: Tokens.borderWidth
    opacity: enabled ? 1 : 0.5
    activeFocusOnTab: true
    Keys.onReturnPressed: if (enabled) root.clicked()
    Keys.onSpacePressed: if (enabled) root.clicked()
    Keys.onTabPressed: root.focusStep(1)
    Keys.onBacktabPressed: root.focusStep(-1)
    Text {
        id: label
        anchors.centerIn: parent
        width: Math.min(implicitWidth, root.width - Tokens.spaceLg)
        text: root.text
        textFormat: Text.PlainText
        elide: Text.ElideRight
        color: Tokens.foreground
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontSmall
    }
    MouseArea {
        id: pointer
        anchors.fill: parent
        enabled: root.enabled
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: root.clicked()
    }
    Accessible.role: Accessible.Button
    Accessible.name: root.text
    Accessible.onPressAction: if (root.enabled) root.clicked()
}
