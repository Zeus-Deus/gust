import QtQuick
import com.gust.app

// Rubber-band select over a list or grid. Sits ABOVE the view but claims a
// press only on empty space: a press on an item is refused and falls through
// to the item (click, double-click, drag-to-move stay exactly as they were).
// Wheel and hover are never taken, so scrolling and hover highlights work.
MouseArea {
    id: band
    acceptedButtons: Qt.LeftButton
    preventStealing: true

    /// The Flickable (ListView or GridView) being banded.
    property var view: null
    property var model: null
    /// Item size in content coordinates, and items per row (1 for a list).
    property real cellWidth: 1
    property real cellHeight: 1
    property int columns: 1

    readonly property bool active: dragging
    property bool dragging: false
    property point startContent: Qt.point(0, 0)
    property point nowView: Qt.point(0, 0)
    signal bandChanged()

    function toContent(x, y) {
        return Qt.point(x + view.contentX - view.originX, y + view.contentY - view.originY)
    }

    onPressed: (mouse) => {
        // A press on an item belongs to the item (click, drag-to-move).
        if (!view || view.indexAt(mouse.x + view.contentX, mouse.y + view.contentY) >= 0) {
            mouse.accepted = false; return
        }
        // No forceActiveFocus() here: the window's key handler owns the
        // keyboard, and a view that took focus swallowed every arrow key
        // until something else gave it back. Views report the band through
        // bandChanged, and the window refocuses its key handler on that.
        startContent = toContent(mouse.x, mouse.y)
        nowView = Qt.point(mouse.x, mouse.y)
        dragging = false
        model.band_begin((mouse.modifiers & Qt.ControlModifier) !== 0)
    }
    onPositionChanged: (mouse) => {
        if (!pressed) return
        nowView = Qt.point(mouse.x, mouse.y)
        const now = toContent(mouse.x, mouse.y)
        if (!dragging && Math.abs(now.x - startContent.x) + Math.abs(now.y - startContent.y) < 6) return
        dragging = true
        const r1 = Math.floor(Math.min(startContent.y, now.y) / cellHeight)
        const r2 = Math.floor(Math.max(startContent.y, now.y) / cellHeight)
        const c1 = Math.floor(Math.min(startContent.x, now.x) / cellWidth)
        const c2 = Math.floor(Math.max(startContent.x, now.x) / cellWidth)
        model.band_update(r1, r2, c1, c2, columns)
        band.bandChanged()
    }
    onReleased: {
        // A plain click on empty space clears the selection, as everywhere.
        if (!dragging && model) model.sel_clear()
        dragging = false
        band.bandChanged()
    }

    // The band itself: accent outline, faint fill.
    Rectangle {
        visible: band.dragging
        readonly property point s: Qt.point(band.startContent.x - band.view.contentX + band.view.originX,
                                            band.startContent.y - band.view.contentY + band.view.originY)
        x: Math.min(s.x, band.nowView.x)
        y: Math.min(s.y, band.nowView.y)
        width: Math.abs(band.nowView.x - s.x)
        height: Math.abs(band.nowView.y - s.y)
        color: Tokens.withAlpha(Tokens.accent, 0.12)
        border.width: Math.max(1, Tokens.borderWidth)
        border.color: Tokens.accent
    }
}
