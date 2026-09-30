pragma Singleton
import QtQuick
import com.gust.app

// Single source of theme truth for every QML file.
// No colour/font/radius/spacing literal may appear anywhere else in ui/.
QtObject {
    id: root

    // Watches Omarchy's theme folder: switching themes repaints Gust live.
    property ThemeTokens tokens: ThemeTokens {
        Component.onCompleted: watch_theme()
    }

    // Every token goes through these so it depends on `revision`: `color()`
    // is a call, and QML only re-evaluates a binding for PROPERTIES it read.
    function col(k) { return tokens.revision >= 0 ? tokens.color(k) : "" }
    function met(k) { return tokens.revision >= 0 ? tokens.metric(k) : 0 }

    readonly property string themeName: tokens.name
    readonly property bool isLight: tokens.is_light

    // --- colour tokens -----------------------------------------------------
    readonly property color background: root.col("background")
    readonly property color backgroundDark: root.col("dark_background")
    readonly property color backgroundLighter: root.col("lighter_background")
    readonly property color foreground: root.col("foreground")
    readonly property color foregroundDim: root.col("dark_foreground")
    readonly property color foregroundBright: root.col("bright_foreground")
    readonly property color accent: root.col("accent")
    readonly property color selection: root.col("selection")
    readonly property color muted: root.col("muted")
    readonly property color border: root.col("border")
    readonly property color danger: root.col("red")
    readonly property color ok: root.col("green")
    readonly property color warn: root.col("yellow")
    readonly property color barBackground: root.col("bar_background")
    readonly property color menuBackground: root.col("menu_background")
    readonly property color selectedBackground: root.col("selected_background")
    readonly property color selectedText: root.col("selected_text")

    // --- metrics -----------------------------------------------------------
    // Ctrl+= / Ctrl+- (Omarchy's zoom keys) scale type, and rows with it.
    property real zoom: 1.0
    readonly property real fontBase: root.met("font_base") * zoom
    // Density from Settings multiplies the theme's own spacing.
    property real density: 1.0
    readonly property real spacingScale: root.met("spacing_scale") * density
    readonly property real selectedAlpha: root.met("selected_background_alpha")
    readonly property real borderAlpha: root.met("normal_border_alpha")
    readonly property real borderWidth: Math.max(1, root.met("border_width"))

    // Derived type scale (shell.toml semantics: body = base).
    readonly property real fontCaption: Math.round(fontBase * 0.84)
    readonly property real fontSmall: Math.round(fontBase * 0.92)
    readonly property real fontBody: fontBase
    readonly property real fontSubtitle: Math.round(fontBase * 1.083)
    readonly property real fontTitle: Math.round(fontBase * 1.167)
    readonly property real fontHeading: Math.round(fontBase * 1.333)

    // Derived spacing scale.
    readonly property real spaceXs: Math.round(3 * spacingScale)
    readonly property real spaceSm: Math.round(4 * spacingScale)
    readonly property real spaceMd: Math.round(6 * spacingScale)
    readonly property real spaceLg: Math.round(8 * spacingScale)
    readonly property real spaceXl: Math.round(10 * spacingScale)
    readonly property real spaceXxl: Math.round(14 * spacingScale)
    readonly property real rowHeight: Math.round(26 * spacingScale * (fontBase / 12))
    readonly property real rowPaddingX: Math.round(12 * spacingScale)
    readonly property real controlHeight: Math.round(28 * spacingScale * (fontBase / 12))

    readonly property string fontFamily: "monospace"

    // Motion. Short enough to feel instant, long enough to read as motion.
    readonly property int animFast: 120

    // Quattro surfaces use sharp borders; radius is a token, not a constant.
    readonly property real radius: 0

    function withAlpha(c, a) {
        return Qt.rgba(c.r, c.g, c.b, a)
    }

    function reload() {
        tokens.reload()
    }
}
