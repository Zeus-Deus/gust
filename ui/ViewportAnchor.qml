import QtQuick

// Keeps a list or grid where the user was looking when the model replaces or
// reorders its rows (DESIGN §2.2.1). The model brackets the change with
// modelAboutToBeReset (or layoutAboutToBeChanged for a re-sort), then
// announces the cursor and emits rows_replaced(keep):
//   0  start of the folder (a new place),
//   1  follow the cursor,
//   2  keep what is on screen: a live change or a re-sort/re-filter.
// Geometry is read beforehand without calling the model. Afterwards
// published_row() maps a captured row to its new index, so the cursor row
// stays at the same height when it was visible and is still the cursor, else
// the top row does, else the cursor is brought into view as usual. The
// position is set directly: no animation, no focus or text-editing change.
// A hidden view has no model and so hears nothing.
QtObject {
    id: anchor
    /// The ListView or GridView to keep in place.
    required property Item view

    /// What was on screen at the last modelAboutToBeReset, until restored.
    /// Tagged with its model: a view that changed model in between ignores it.
    property var saved: null

    property Connections modelSignals: Connections {
        // A hidden view's model reads back as undefined, not null.
        target: anchor.view && anchor.view.model ? anchor.view.model : null
        // A model without rows_replaced still resets; it keeps Qt's default.
        ignoreUnknownSignals: true
        function onModelAboutToBeReset() { anchor.saved = anchor.capture() }
        // A re-sort keeps Qt's scroll position but not the cursor's place on
        // screen once the cursor follows its file; rows_replaced(2) restores it.
        function onLayoutAboutToBeChanged() { anchor.saved = anchor.capture() }
        function onRows_replaced(keep) { anchor.restore(keep) }
    }

    /// View geometry only: rows and pixel offsets from the top edge.
    function capture() {
        const v = anchor.view
        const top = v.contentY
        const cur = v.currentIndex
        const it = cur >= 0 ? v.itemAtIndex(cur) : null
        const onScreen = it && it.y < top + v.height && it.y + it.height > top
        const first = v.indexAt(v.contentX, top)
        const ti = first >= 0 ? v.itemAtIndex(first) : null
        return {
            model: v.model,
            cursor: onScreen ? { row: cur, offset: it.y - top } : null,
            top: ti ? { row: first, offset: ti.y - top } : null
        }
    }

    function restore(keep) {
        const s = anchor.saved
        anchor.saved = null
        const v = anchor.view
        if (!v || !v.model) return
        if (keep === 0) {
            v.positionViewAtBeginning()
            return
        }
        if (keep === 2 && s && s.model === v.model) {
            // The old cursor row's height is kept only if it IS the cursor
            // just published. If the cursor was deliberately moved elsewhere,
            // putting the old one back could hide the new one.
            if (s.cursor) {
                const row = v.model.published_row(s.cursor.row)
                if (row >= 0 && row === v.currentIndex && anchor.place(row, s.cursor.offset)) return
            }
            if (s.top) {
                const row = v.model.published_row(s.top.row)
                if (row >= 0 && anchor.place(row, s.top.offset)) return
            }
        }
        if (v.currentIndex >= 0) v.positionViewAtIndex(v.currentIndex, ListView.Contain)
        else v.positionViewAtBeginning()
    }

    /// Put `row` `offset` pixels below the top edge, clamped to the content.
    function place(row, offset) {
        const v = anchor.view
        // Realises the row's delegate so its laid-out position is known.
        v.positionViewAtIndex(row, ListView.Contain)
        const it = v.itemAtIndex(row)
        if (!it) return false
        const lowest = v.originY
        const highest = v.originY + Math.max(0, v.contentHeight - v.height)
        v.contentY = Math.max(lowest, Math.min(highest, it.y - offset))
        return true
    }
}
