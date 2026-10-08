import QtQuick
import com.gust.app

// A small square badge for one emblem: read-only, no access, mount point,
// link, broken link (DESIGN §6.21). Square, like every Quattro surface.
Rectangle {
    id: root
    /// `readonly`, `unreadable`, `mount`, `link` or `broken-link`.
    property string kind: ""
    property real size: 16
    /// Grid draws a framed badge over the icon; Rows draws the bare glyph.
    property bool framed: true
    width: size
    height: size
    radius: Tokens.radius
    color: framed ? Tokens.background : "transparent"
    border.width: framed ? Tokens.borderWidth : 0
    border.color: Tokens.muted

    // Nerd Font glyphs, with plain characters for a desktop without one.
    readonly property var glyphs: ({
        readonly: 0xF033E, unreadable: 0xF073A, mount: 0xF01EA,
        link: 0xF0339, "broken-link": 0xF033A
    })
    readonly property var plain: ({
        readonly: "ro", unreadable: "×", mount: "⏏", link: "↗", "broken-link": "↯"
    })

    Text {
        textFormat: Text.PlainText
        anchors.centerIn: parent
        text: Tokens.nerdGlyphs && root.glyphs[root.kind] !== undefined
              ? String.fromCodePoint(root.glyphs[root.kind]) : (root.plain[root.kind] || "")
        color: root.kind === "readonly" ? Tokens.warn
             : (root.kind === "unreadable" || root.kind === "broken-link") ? Tokens.danger
             : root.kind === "mount" ? Tokens.cyan : Tokens.accent
        font.family: Tokens.fontFamily
        // Bare (Rows) glyphs fill their box; framed ones leave a margin.
        font.pixelSize: Math.max(1, Math.round(root.size * (root.framed ? 0.72 : 1.0)))
    }
}
