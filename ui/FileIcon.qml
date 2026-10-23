import QtQuick
import QtQuick.Shapes
import com.gust.app

// One entry's icon, drawn rather than loaded (DESIGN §6.21): a folder in
// thin accent lines with a solid tab, or a page outlined in its family's
// colour with the extension printed on it. Theme colours only, so a theme
// switch restyles every icon with the rest of the window.
//
// `look` is `Tokens.iconLook(iconKey)`: shape, family, label, glyph.
Item {
    id: root
    property string look: ""
    /// Height of the box the icon fits in; the width is the same.
    property real size: 32
    width: size
    height: size

    readonly property var parts: { return root.look.split("\t") }
    readonly property string shape: parts[0] || "page"
    readonly property string family: parts.length > 1 ? parts[1] : ""
    readonly property string label: parts.length > 2 ? parts[2] : ""
    readonly property string glyph: parts.length > 3 ? parts[3] : ""
    readonly property bool isFolder: shape === "folder"
    /// Small icons (Rows, Columns) drop the label and the folder glyph:
    /// below this they turn into smudges.
    readonly property bool detailed: size >= 28
    readonly property real line: Math.max(1, size * 0.032)

    // A family's colour. One per family, from the theme palette. A broken
    // link is drawn neutral: its red badge carries the error, and red stays
    // free for programs.
    readonly property color hue: {
        if (shape === "broken") return Tokens.foregroundDim
        if (shape === "special") return Tokens.foregroundDim
        switch (family) {
        case "code": return Tokens.ok
        case "data": return Tokens.cyan
        case "doc": return Tokens.blue
        case "image": return Tokens.magenta
        case "video": return Tokens.magenta
        case "audio": return Tokens.orange
        case "archive": return Tokens.warn
        case "app": return Tokens.danger
        case "other": return Tokens.foregroundDim
        default: return Tokens.foreground
        }
    }
    // Light themes' pastel accents (pink, peach) wash out on a white page:
    // draw them darker there.
    readonly property color tint: Tokens.isLight ? Qt.darker(hue, 1.45) : hue

    // Special folders: Nerd Font glyphs (Omarchy's own icon font).
    readonly property var folderGlyphs: ({
        documents: 0xF0219, downloads: 0xF01DA, music: 0xF0387,
        pictures: 0xF02E9, videos: 0xF0230, home: 0xF02DC,
        desktop: 0xF0379, templates: 0xF1517, "public": 0xF000E
    })

    // --- folder: 50 x 42 units, bottom-aligned in the box ----------------
    Shape {
        id: folder
        visible: root.isFolder
        width: root.size
        height: Math.round(root.size * 0.84)
        // Bottom-aligned with pages in Grid; centred on the text line in
        // Rows and Columns.
        anchors.bottom: root.detailed ? parent.bottom : undefined
        anchors.verticalCenter: root.detailed ? undefined : parent.verticalCenter
        anchors.horizontalCenter: parent.horizontalCenter
        preferredRendererType: Shape.CurveRenderer
        readonly property size k: Qt.size(width / 50, height / 42)
        ShapePath {
            scale: folder.k
            strokeColor: Tokens.accent
            strokeWidth: root.line
            fillColor: Tokens.withAlpha(Tokens.accent, 0.16)
            joinStyle: ShapePath.MiterJoin
            PathSvg { path: "M1,5 H18 L22,9 H49 V41 H1 Z" }
        }
        ShapePath {
            scale: folder.k
            strokeColor: "transparent"
            fillColor: Tokens.accent
            PathSvg { path: "M1,5 H18 L22,9 H1 Z" }
        }
        Text {
            textFormat: Text.PlainText
            anchors.centerIn: parent
            anchors.verticalCenterOffset: parent.height * 0.08
            visible: root.detailed && Tokens.nerdGlyphs && root.glyph !== ""
                     && root.folderGlyphs[root.glyph] !== undefined
            text: visible ? String.fromCodePoint(root.folderGlyphs[root.glyph]) : ""
            color: Tokens.accent
            font.family: Tokens.fontFamily
            font.pixelSize: Math.round(root.size * 0.36)
        }
    }

    // --- page: 40 x 50 units -------------------------------------------
    Shape {
        id: page
        visible: !root.isFolder
        width: Math.round(root.size * 0.8)
        height: root.size
        anchors.bottom: parent.bottom
        anchors.horizontalCenter: parent.horizontalCenter
        preferredRendererType: Shape.CurveRenderer
        readonly property size k: Qt.size(width / 40, height / 50)
        ShapePath {
            scale: page.k
            strokeColor: root.tint
            strokeWidth: root.line
            fillColor: Tokens.withAlpha(root.tint, 0.12)
            joinStyle: ShapePath.MiterJoin
            PathSvg { path: "M1,1 H28 L39,12 V49 H1 Z" }
        }
        ShapePath {
            scale: page.k
            strokeColor: "transparent"
            fillColor: Tokens.withAlpha(root.tint, 0.6)
            PathSvg { path: "M28,1 V12 H39 Z" }
        }
        // The extension, the exact type at a glance.
        Text {
            textFormat: Text.PlainText
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.bottom: parent.bottom
            anchors.bottomMargin: Math.round(parent.height * 0.14)
            visible: root.detailed && root.label !== ""
            text: root.label === "" ? "" : root.label.toUpperCase()
            color: root.tint
            font.family: Tokens.fontFamily
            font.bold: true
            // One size for every label up to four letters (JSON, TOML);
            // only a five-letter one (WOFF2) steps down.
            font.pixelSize: Math.max(1, Math.round(root.size * (root.label.length > 4 ? 0.16 : 0.19)))
        }
        // No extension (LICENSE, .bashrc): a few lines of text.
        Column {
            anchors.left: parent.left
            anchors.leftMargin: Math.round(parent.width * 0.18)
            anchors.top: parent.top
            anchors.topMargin: Math.round(parent.height * 0.4)
            spacing: Math.round(parent.height * 0.1)
            visible: root.detailed && root.label === "" && root.glyph !== "empty"
            Repeater {
                model: [0.64, 0.64, 0.44]
                delegate: Rectangle {
                    required property real modelData
                    width: Math.round(page.width * modelData)
                    height: root.line
                    color: Tokens.withAlpha(root.tint, 0.7)
                }
            }
        }
    }
}
