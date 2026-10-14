import QtQuick
import com.gust.app

// A file's colour tags as small dots, in tag order (DESIGN §6.25). Shown
// after the name in rows and under the icon in the grid; nothing at all
// when the file has no colours.
Row {
    id: root
    /// "red,blue" — the row's tagKeys role.
    property string keys: ""
    property int dot: Math.round(Tokens.fontBody * 0.6)
    spacing: Math.max(1, Math.round(dot * 0.35))
    visible: keys !== ""

    Repeater {
        model: root.keys === "" ? [] : root.keys.split(",")
        delegate: Rectangle {
            required property string modelData
            width: root.dot
            height: root.dot
            radius: root.dot / 2
            // Spelled out (not a function call) so a theme change repaints it.
            color: modelData === "red" ? Tokens.danger : modelData === "orange" ? Tokens.orange
                 : modelData === "yellow" ? Tokens.warn : modelData === "green" ? Tokens.ok
                 : modelData === "blue" ? Tokens.blue : modelData === "purple" ? Tokens.magenta
                 : Tokens.muted
            border.width: Math.max(1, Tokens.borderWidth)
            border.color: Tokens.withAlpha(Tokens.background, 0.6)
            Accessible.role: Accessible.StaticText
            Accessible.name: modelData + " tag"
        }
    }
}
