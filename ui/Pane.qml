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
    /// A right-button drag was released: offer Copy, Move or Link here.
    signal dropMenu(int row, real x, real y)

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
    /// What the drag preview shows while it is up, else "" (qa).
    function ghostName() { return ghost.visible ? ghost.name : "" }
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
            outward.handedOff = false
            ghost.begin(r)
            pane.dragBegan()
        }
        // Say what a drop would do BEFORE the button is released, so the
        // operation and destination are never a surprise.
        onDropMovedOver: (r) => {
            if (!pane.model) return
            const t = rowsView.dragRight ? qsTr("Release to choose: Copy, Move or Link here")
                                    : pane.model.drag_preview(r, pane.dragMode(rowsView))
            ghost.action = t
            pane.dropPreview(t)
        }
        onDropReleased: (r) => pane.finishDrop(r)
        onDropMenuRequested: (r, x, y) => pane.requestDropMenu(r, x, y)
        onDragMovedTo: (x, y) => { ghost.follow(rowsView, x, y); pane.maybeHandOff(rowsView, x, y) }
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
            outward.handedOff = false
            ghost.begin(r)
            pane.dragBegan()
        }
        onDropMovedOver: (r) => {
            if (!pane.model) return
            const t = gridView.dragRight ? qsTr("Release to choose: Copy, Move or Link here")
                                    : pane.model.drag_preview(r, pane.dragMode(gridView))
            ghost.action = t
            pane.dropPreview(t)
        }
        onDropReleased: (r) => pane.finishDrop(r)
        onDropMenuRequested: (r, x, y) => pane.requestDropMenu(r, x, y)
        onDragMovedTo: (x, y) => { ghost.follow(gridView, x, y); pane.maybeHandOff(gridView, x, y) }
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
            outward.handedOff = false
            ghost.begin(r)
            pane.dragBegan()
        }
        onDropMovedOver: (r) => {
            if (!pane.model) return
            const t = columnsView.dragRight ? qsTr("Release to choose: Copy, Move or Link here")
                                    : pane.model.drag_preview(r, pane.dragMode(columnsView))
            ghost.action = t
            pane.dropPreview(t)
        }
        onDropReleased: (r) => pane.finishDrop(r)
        onDropMenuRequested: (r, x, y) => pane.requestDropMenu(r, x, y)
        onDragMovedTo: (x, y) => { ghost.follow(columnsView, x, y); pane.maybeHandOff(columnsView, x, y) }
        onContextRequested: (r, x, y) => pane.contextRequested(r, x, y)
        onMiddleClicked: (r) => pane.openInNewTab(r)
    }

    /// A drop is an ordinary copy or move through the same engine as any
    /// other (DESIGN §6.1). A refusal is reported, never swallowed.
    function requestDropMenu(row, x, y) {
        if (outward.handedOff) { outward.handedOff = false; return }
        pane.dropMenu(row, x, y)
    }
    function rowPoints() {
        const v = currentView()
        return v.rowPoints ? v.rowPoints() : ({})
    }
    /// How far the list is scrolled, in pixels (qa).
    function scrollTop() {
        const v = currentView()
        return v.scrollTop ? v.scrollTop() : -1
    }
    /// Per visible row: name, icon key, emblems, resolved URL and whether
    /// the delegate is drawing the icon (qa).
    function iconInfo() {
        const v = currentView()
        return v.iconInfo ? v.iconInfo() : []
    }

    /// What a release does (DESIGN §6.1): 0 = what suits where it lands
    /// (Move on the same drive, Copy onto another), 1 = Copy (Ctrl),
    /// 2 = Move (Shift).
    function dragMode(view) {
        if (view.dragMods & Qt.ShiftModifier) return 2
        if (view.dragMods & Qt.ControlModifier) return 1
        return pane.dropAsMove ? 2 : 0
    }
    function finishDrop(row) {
        if (!pane.model) return
        if (outward.handedOff) { outward.handedOff = false; return }
        const err = pane.model.drag_drop_on_row(row, pane.dragMode(pane.currentView()))
        pane.model.drag_end()
        if (err !== "") pane.dropFailed(err)
        else pane.dropDone()
    }

    // --- what is being dragged, under the pointer --------------------------
    //
    // A held file follows the pointer: its icon, its name and how many
    // items travel with it, so a drag is unmistakable before anything is
    // dropped. Drawn above the views; it never takes the mouse.
    Rectangle {
        id: ghost
        objectName: "dragGhost"
        z: 100
        visible: pane.currentView().dragActive && !outward.handedOff
        enabled: false
        property string name: ""
        property string look: ""
        property int count: 0
        /// What releasing here would do: "Move 1 item to inbox".
        property string action: ""
        function begin(row) {
            ghost.action = ""
            if (!pane.model) return
            ghost.name = pane.model.entry_name(row)
            ghost.look = Tokens.iconLook(pane.model.entry_icon_key(row))
            ghost.count = pane.model.drag_count()
        }
        function follow(view, x, y) {
            const p = view.mapToItem(pane, x, y)
            // Below-right of the pointer, so the drop target stays visible.
            ghost.x = Math.min(p.x + Tokens.spaceLg, pane.width - ghost.width)
            ghost.y = Math.min(p.y + Tokens.spaceLg, pane.height - ghost.height)
        }
        width: ghostRow.implicitWidth + Tokens.spaceLg * 2
        height: ghostRow.implicitHeight + Tokens.spaceSm * 2
        radius: Tokens.radius
        color: Tokens.withAlpha(Tokens.backgroundDark, 0.92)
        border.width: Math.max(1, Tokens.borderWidth)
        border.color: Tokens.accent
        Row {
            id: ghostRow
            anchors.centerIn: parent
            spacing: Tokens.spaceSm
            FileIcon {
                size: Tokens.rowHeight * 0.8
                look: ghost.look
                anchors.verticalCenter: parent.verticalCenter
            }
            Column {
                anchors.verticalCenter: parent.verticalCenter
                Text {
                    text: ghost.count > 1 ? qsTr("%1 items").arg(ghost.count) : ghost.name
                    color: Tokens.foregroundBright
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontSmall
                    textFormat: Text.PlainText
                    elide: Text.ElideMiddle
                    width: Math.min(implicitWidth, Tokens.rowHeight * 10)
                }
                // Move or Copy, and where: known before the button is let go.
                Text {
                    visible: ghost.action !== ""
                    text: ghost.action
                    color: ghost.action.indexOf("Move") === 0 ? Tokens.accent : Tokens.foregroundDim
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontCaption
                    textFormat: Text.PlainText
                    elide: Text.ElideMiddle
                    width: Math.min(implicitWidth, Tokens.rowHeight * 10)
                }
            }
        }
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
        let m = null
        try { m = JSON.parse(pane.model.drag_mime_json()) } catch (e) { m = null }
        if (!m || m.uris === "") return
        // Ctrl or Shift held as the drag leaves decides what another Gust
        // window does with it, as it would here (D60).
        const mode = pane.dragMode(viewItem)
        outward.handedOff = true
        rowsView.dragActive = false
        gridView.dragActive = false
        columnsView.dragActive = false
        outward.Drag.mimeData = {
            "text/uri-list": m.uris,
            "text/plain": m.text,
            "x-special/gnome-copied-files": m.gnome,
            // Marks the drag as Gust's own for another Gust window:
            // "gust\n<mode>". Other applications ignore the type.
            "application/x-gust-drag": "gust\n" + mode
        }
        // Offer Move only when the user asked for it (Shift); otherwise a
        // drop in another application is a copy and the original stays.
        outward.Drag.proposedAction = mode === 2 ? Qt.MoveAction : Qt.CopyAction
        outward.Drag.active = true
        pane.handedOff()
    }
    Item {
        id: outward
        // The sidebar and the path bar take Gust's own drag by this name;
        // a drop from another application never carries it.
        objectName: "gustDragProxy"
        property bool handedOff: false
        /// The model the drag came from, for drop targets outside the view.
        readonly property var paneModel: pane.model
        width: 1; height: 1
        Drag.dragType: Drag.Automatic
        Drag.supportedActions: Qt.CopyAction | Qt.MoveAction
        Drag.proposedAction: Qt.CopyAction
        Drag.onDragFinished: (action) => {
            outward.Drag.active = false
            outward.handedOff = false
            if (pane.model) pane.model.drag_end()
        }
    }

    /// The model a drag came from when it is Gust's own, else null.
    function gustSource(d) {
        return d && d.source && d.source.objectName === "gustDragProxy" ? d.source.paneModel : null
    }
    /// The folder a drop on `row` lands in: that folder, or this one.
    function folderAt(row) {
        if (!pane.model) return ""
        return row >= 0 && pane.model.is_dir(row) ? pane.model.entry_path(row) : pane.model.location
    }

    // --- dropping IN from other windows and applications ----------------
    //
    // Files dragged from another Gust window, a browser download bar,
    // another file manager or a chat app land here as text/uri-list and go
    // through the same planner and engine as Gust's own drops. Another Gust
    // window's drag follows the same rule as a drop inside one (D60: Move on
    // one drive, Copy across; Ctrl copies, Shift moves). Anything else
    // copies. The folder under the pointer is ringed, and the hint strip
    // says what releasing will do, before anything happens.
    /// The view whose folder ring a platform drag is lighting.
    function setDropRing(row) {
        for (const v of [rowsView, gridView, columnsView]) {
            v.platformDrop = row >= -1 && v === pane.currentView()
            v.dropRow = v === pane.currentView() ? row : -1
        }
    }
    function clearDropRing() {
        for (const v of [rowsView, gridView, columnsView]) { v.platformDrop = false; v.dropRow = -1 }
    }
    DropArea {
        id: inbound
        anchors.fill: parent
        keys: ["text/uri-list"]
        /// The drag in flight, read once when it enters (qa reads them too).
        property string uris: ""
        property bool fromGust: false
        property int mode: 0
        function read(d) {
            inbound.uris = d.hasUrls ? d.urls.map(u => u.toString()).join("\r\n") + "\r\n"
                                     : (d.getDataAsString("text/uri-list") || "")
            const mark = d.formats.indexOf("application/x-gust-drag") >= 0
                         ? (d.getDataAsString("application/x-gust-drag") || "") : ""
            inbound.fromGust = mark.indexOf("gust\n") === 0
            inbound.mode = inbound.fromGust ? (parseInt(mark.split("\n")[1]) || 0) : 0
        }
        onEntered: (drag) => {
            drag.accept(Qt.CopyAction)
            if (!pane.model || pane.gustSource(drag)) return
            pane.model.drop_enter()
            inbound.read(drag)
        }
        onExited: {
            pane.clearDropRing()
            pane.dropPreview("")
        }
        onPositionChanged: (drag) => {
            if (!pane.model) return
            const row = pane.currentView().rowAt ? pane.currentView().rowAt(drag.x, drag.y) : -1
            const src = pane.gustSource(drag)
            if (src) {
                pane.dropPreview(src === pane.model ? pane.model.drag_preview(row, 0)
                                 : src.drag_preview_into(pane.folderAt(row), 0))
                return
            }
            // Ring the folder that would receive the drop.
            pane.setDropRing(row >= 0 && pane.model.is_dir(row) ? row : -1)
            const t = pane.model.uri_preview(row, "", inbound.uris, inbound.mode, inbound.fromGust)
            pane.dropPreview(t)
        }
        onDropped: (drop) => {
            pane.clearDropRing()
            if (!pane.model) return
            // Gust's own drag that left the view and came back, or came
            // from the other pane: the same planner as a drop in the list
            // (Move on one drive, Copy across), not an outside paste.
            const src = pane.gustSource(drop)
            if (src) {
                const row = pane.currentView().rowAt ? pane.currentView().rowAt(drop.x, drop.y) : -1
                drop.accept(Qt.CopyAction)
                const e = src === pane.model ? pane.model.drag_drop_on_row(row, 0)
                                             : src.drag_drop_into(pane.folderAt(row), 0)
                if (e !== "") pane.dropFailed(e)
                else pane.dropDone()
                return
            }
            inbound.read(drop)
            const row = pane.currentView().rowAt ? pane.currentView().rowAt(drop.x, drop.y) : -1
            // The receiver does the move itself (journaled, undoable); the
            // source never deletes anything because of a drop.
            drop.accept(Qt.CopyAction)
            const err = pane.model.uri_drop(row, "", inbound.uris, inbound.mode, inbound.fromGust)
            if (err !== "") pane.dropFailed(err)
            else pane.dropDone()
        }
    }
}
