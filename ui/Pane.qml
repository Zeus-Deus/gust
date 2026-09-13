// One pane: the three views over a single tab's model.
//
// Extracted so a second pane is a second instance rather than a second copy
// of ninety lines. The pane knows nothing about tabs, panes or the window —
// it shows a model in a view mode and reports what the user did. Everything
// that needs window state (status text, alerts, QA dumps) leaves as a signal.

import QtQuick
import com.gust.app

Item {
    id: pane

    /// The model this pane is currently showing. May be null for the frames
    /// between tab rebuilds; every binding below tolerates that.
    property var model: null
    /// "rows" | "grid" | "columns" — this pane's own choice.
    property string viewMode: "rows"
    /// Whether a drop should move rather than copy (the window decides).
    property bool dropAsMove: false

    signal activated(int row)
    signal selectionTouched()
    signal dragBegan()
    signal dropPreview(string text)
    signal dropFailed(string message)
    signal dropDone()

    /// The view the user is looking at, for focus and page-step maths.
    function currentView() {
        return viewMode === "grid" ? gridView
             : viewMode === "columns" ? columnsView
             : rowsView
    }
    /// How many items a Down key should skip in grid mode; 1 elsewhere.
    function pageStep() {
        return viewMode === "grid" ? gridView.columns() : 1
    }
    function focusList() { currentView().forceActiveFocus() }

    RowsView {
        id: rowsView
        anchors.fill: parent
        visible: pane.viewMode === "rows"
        model: pane.model
        onActivated: (r) => pane.activated(r)
        onSelectionTouched: pane.selectionTouched()
        onDragStarted: (r) => {
            rowsView.dragActive = true
            if (pane.model) pane.model.drag_begin(r)
            pane.dragBegan()
        }
        // Say what a drop would do BEFORE the button is released, so the
        // operation and destination are never a surprise.
        onDropMovedOver: (r) => {
            if (pane.model)
                pane.dropPreview(pane.model.drop_preview(r, pane.dropAsMove))
        }
        onDropReleased: (r) => pane.finishDrop(r)
    }

    IconGridView {
        id: gridView
        anchors.fill: parent
        visible: pane.viewMode === "grid"
        model: pane.model
        onActivated: (r) => pane.activated(r)
        onSelectionTouched: pane.selectionTouched()
        onDragStarted: (r) => {
            gridView.dragActive = true
            if (pane.model) pane.model.drag_begin(r)
            pane.dragBegan()
        }
        onDropMovedOver: (r) => {
            if (pane.model)
                pane.dropPreview(pane.model.drop_preview(r, pane.dropAsMove))
        }
        onDropReleased: (r) => pane.finishDrop(r)
    }

    ColumnsView {
        id: columnsView
        anchors.fill: parent
        visible: pane.viewMode === "columns"
        model: pane.model
        onActivated: (r) => pane.activated(r)
        onSelectionTouched: pane.selectionTouched()
        onDragStarted: (r) => {
            columnsView.dragActive = true
            if (pane.model) pane.model.drag_begin(r)
            pane.dragBegan()
        }
        onDropMovedOver: (r) => {
            if (pane.model)
                pane.dropPreview(pane.model.drop_preview(r, pane.dropAsMove))
        }
        onDropReleased: (r) => pane.finishDrop(r)
    }

    /// A drop is an ordinary copy or move through the same engine as any
    /// other (DESIGN §6.1). A refusal is reported, never swallowed.
    function finishDrop(row) {
        if (!pane.model) return
        const err = pane.model.drop_on_row(row, pane.dropAsMove)
        if (err !== "") pane.dropFailed(err)
        else pane.dropDone()
    }
}
