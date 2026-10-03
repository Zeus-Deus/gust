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
    /// Right-click or Menu key: row -1 means the folder itself. x/y are in
    /// window coordinates.
    signal contextRequested(int row, real x, real y)
    signal openInNewTab(int row)

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
    function cursorAnchor() { return currentView().cursorAnchor() }
    function nameHeaderPoint() {
        const v = currentView()
        return v.nameHeaderPoint ? v.nameHeaderPoint() : null
    }
    function headerTexts() {
        const v = currentView()
        return v.headerTexts ? v.headerTexts() : []
    }

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
        onDragMovedTo: (x, y) => pane.maybeHandOff(rowsView, x, y)
        onContextRequested: (r, x, y) => pane.contextRequested(r, x, y)
        onMiddleClicked: (r) => pane.openInNewTab(r)
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
        onDragMovedTo: (x, y) => pane.maybeHandOff(gridView, x, y)
        onContextRequested: (r, x, y) => pane.contextRequested(r, x, y)
        onMiddleClicked: (r) => pane.openInNewTab(r)
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
        onDragMovedTo: (x, y) => pane.maybeHandOff(columnsView, x, y)
        onContextRequested: (r, x, y) => pane.contextRequested(r, x, y)
        onMiddleClicked: (r) => pane.openInNewTab(r)
    }

    /// A drop is an ordinary copy or move through the same engine as any
    /// other (DESIGN §6.1). A refusal is reported, never swallowed.
    function finishDrop(row) {
        if (!pane.model) return
        if (outward.handedOff) { outward.handedOff = false; return }
        const err = pane.model.drop_on_row(row, pane.dropAsMove)
        if (err !== "") pane.dropFailed(err)
        else pane.dropDone()
    }

    // --- dragging OUT to other applications ------------------------------
    //
    // Inside Gust a drag is Gust's own gesture (it previews what a drop
    // will do). When the pointer leaves the pane while dragging, the same
    // payload is handed to the compositor as a real drag with the standard
    // text/uri-list and GNOME types, so a browser upload box, a chat app, an
    // editor or another file manager can take it. The receiving app decides
    // what to do; Gust never deletes a source because of an outside drop.
    signal handedOff()
    function maybeHandOff(viewItem, x, y) {
        if (outward.handedOff || !pane.model) return
        const p = viewItem.mapToItem(pane, x, y)
        if (p.x >= 0 && p.y >= 0 && p.x < pane.width && p.y < pane.height) return
        const uris = pane.model.drag_uri_list()
        if (uris === "") return
        outward.handedOff = true
        rowsView.dragActive = false
        gridView.dragActive = false
        columnsView.dragActive = false
        outward.Drag.mimeData = {
            "text/uri-list": uris,
            "text/plain": uris.split("\r\n").filter(u => u !== "").map(u => decodeURIComponent(u.replace("file://", ""))).join("\n"),
            "x-special/gnome-copied-files": "copy\n" + uris.split("\r\n").filter(u => u !== "").join("\n")
        }
        outward.Drag.active = true
        pane.handedOff()
    }
    Item {
        id: outward
        property bool handedOff: false
        width: 1; height: 1
        Drag.dragType: Drag.Automatic
        Drag.supportedActions: Qt.CopyAction
        Drag.proposedAction: Qt.CopyAction
        Drag.onDragFinished: (action) => { outward.Drag.active = false }
    }

    // --- dropping IN from other applications -----------------------------
    //
    // Files dragged from a browser download bar, another file manager or a
    // chat app land here as text/uri-list and go through the same planner
    // and engine as Gust's own drops: copy by default, conflicts asked,
    // refusals explained.
    DropArea {
        anchors.fill: parent
        keys: ["text/uri-list"]
        // Gust's own internal gesture never produces a platform drag inside
        // its window, so anything arriving here came from outside — except
        // the outward proxy, which is ignored.
        onEntered: (drag) => {
            if (drag.source === outward) { drag.accepted = false; return }
            drag.accept(Qt.CopyAction)
        }
        onPositionChanged: (drag) => {
            if (!pane.model) return
            const row = pane.currentView().rowAt ? pane.currentView().rowAt(drag.x, drag.y) : -1
            pane.dropPreview(pane.model.drop_preview(row, false))
        }
        onDropped: (drop) => {
            if (!pane.model || drop.source === outward) return
            const uris = drop.hasUrls ? drop.urls.map(u => u.toString()).join("\r\n") + "\r\n"
                                      : (drop.getDataAsString("text/uri-list") || "")
            const row = pane.currentView().rowAt ? pane.currentView().rowAt(drop.x, drop.y) : -1
            const err = pane.model.drop_uris(row, uris, false)
            drop.accept(Qt.CopyAction)
            if (err !== "") pane.dropFailed(err)
            else pane.dropDone()
        }
    }
}
