import QtQuick

// Only validated Normal cache PNGs reach this item. Decode inside their
// 128px box; fit the displayed geometry without asking Qt to enlarge pixels
// during decoding (Image.PreserveAspectFit changes the decode-size policy).
Image {
    id: root
    property real inset: 0
    readonly property real fitScale: parent && implicitWidth > 0 && implicitHeight > 0
        ? Math.max(0, Math.min((parent.width - inset * 2) / implicitWidth,
                              (parent.height - inset * 2) / implicitHeight))
        : 0
    width: implicitWidth * fitScale
    height: implicitHeight * fitScale
    visible: status === Image.Ready
    asynchronous: true
    // Live replacements reuse the URL. Never retain stale decoded pixels.
    cache: false
    fillMode: Image.Stretch
    sourceSize.width: 128
    sourceSize.height: 128
}
