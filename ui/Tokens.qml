pragma Singleton
import QtQuick
import com.gust.app

// Single source of theme truth for every QML file.
// No colour/font/radius/spacing literal may appear anywhere else in ui/.
QtObject {
    id: root

    property ThemeTokens tokens: ThemeTokens {}

    readonly property string themeName: tokens.name
    readonly property bool isLight: tokens.is_light

    // --- colour tokens -----------------------------------------------------
    readonly property color background: tokens.color("background")
    readonly property color backgroundDark: tokens.color("dark_background")
    readonly property color backgroundLighter: tokens.color("lighter_background")
    readonly property color foreground: tokens.color("foreground")
    readonly property color foregroundDim: tokens.color("dark_foreground")
    readonly property color foregroundBright: tokens.color("bright_foreground")
    readonly property color accent: tokens.color("accent")
    readonly property color selection: tokens.color("selection")
    readonly property color muted: tokens.color("muted")
    readonly property color border: tokens.color("border")
    readonly property color danger: tokens.color("red")
    readonly property color ok: tokens.color("green")
    readonly property color warn: tokens.color("yellow")
    readonly property color barBackground: tokens.color("bar_background")
    readonly property color menuBackground: tokens.color("menu_background")
    readonly property color selectedBackground: tokens.color("selected_background")
    readonly property color selectedText: tokens.color("selected_text")

    // --- metrics -----------------------------------------------------------
    readonly property real fontBase: tokens.metric("font_base")
    readonly property real spacingScale: tokens.metric("spacing_scale")
    readonly property real selectedAlpha: tokens.metric("selected_background_alpha")
    readonly property real borderAlpha: tokens.metric("normal_border_alpha")
    readonly property real borderWidth: Math.max(1, tokens.metric("border_width"))

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
