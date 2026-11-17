pragma Singleton
import QtQuick

// Deterministic colours without ThemeTokens, theme watchers or user settings.
QtObject {
    readonly property bool isLight: false
    readonly property bool nerdGlyphs: false
    readonly property string fontFamily: "monospace"
    readonly property color foreground: "#dddddd"
    readonly property color foregroundDim: "#999999"
    readonly property color accent: "#80bfff"
    readonly property color ok: "#80ff80"
    readonly property color cyan: "#80ffff"
    readonly property color blue: "#8080ff"
    property color magenta: "#ff88aa"
    readonly property color orange: "#ffb080"
    readonly property color warn: "#ffff80"
    readonly property color danger: "#ff8080"
    readonly property color foregroundBright: "#ffffff"
    readonly property color border: "#666666"
    readonly property color menuBackground: "#303030"
    readonly property real borderAlpha: 0.4
    readonly property real borderWidth: 1
    readonly property real radius: 0
    readonly property real controlHeight: 28
    readonly property real fontSmall: 11
    readonly property real fontSubtitle: 13
    readonly property real fontHeading: 16
    readonly property real spaceSm: 4
    readonly property real spaceLg: 8

    function withAlpha(c, a) { return Qt.rgba(c.r, c.g, c.b, a) }
}
