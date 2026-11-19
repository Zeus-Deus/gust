import QtQuick
import com.gust.app

// One line icon, by name. Drawn from the Nerd Font Omarchy ships (the same
// Material set the folder glyphs and emblems use), with a plain character
// for a desktop without one, so no control is ever a blank square.
Text {
    id: root
    /// A key of `icons` below.
    property string name: ""
    property real size: Tokens.fontHeading

    readonly property var icons: ({
        sidebar: 0xF06FD, back: 0xF0141, forward: 0xF0142, up: 0xF005D,
        search: 0xF0349, "new-folder": 0xF0B9D, more: 0xF01D8,
        rows: 0xF0279, grid: 0xF11D9, columns: 0xF1487, settings: 0xF08BB,
        close: 0xF0156,
        // Sidebar rows. These have no plain fallback: without the icon
        // font a row is just its name, as it was before.
        home: 0xF06A1, documents: 0xF09EE, downloads: 0xF01DA,
        pictures: 0xF0976, videos: 0xF0BDC, music: 0xF0387,
        recent: 0xF0150, trash: 0xF0A7A, bookmark: 0xF0256,
        drive: 0xF129F, phone: 0xF011C, computer: 0xF0379, cloud: 0xF0163
    })
    readonly property var plain: ({
        sidebar: "☰", back: "‹", forward: "›", up: "↑",
        search: "⌕", "new-folder": "+", more: "⋯",
        rows: "▤", grid: "▦", columns: "⫼", settings: "⛭",
        close: "×"
    })
    readonly property bool drawn: Tokens.nerdGlyphs && icons[name] !== undefined

    textFormat: Text.PlainText
    text: drawn ? String.fromCodePoint(icons[name]) : (plain[name] || "")
    color: Tokens.foreground
    font.family: Tokens.fontFamily
    // A plain fallback character is a letterform, not an icon: body size.
    font.pixelSize: drawn ? Math.round(size) : Tokens.fontSubtitle
    horizontalAlignment: Text.AlignHCenter
    verticalAlignment: Text.AlignVCenter
}
