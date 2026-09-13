import QtQml
import QtQuick
import QtQuick.Window
import com.gust.app

Window {
    id: win

    ListModel {
        id: tabList
        ListElement { startPath: ""; label: "…" }
    }

    // Instantiator, not Repeater: a Repeater needs a visual parent and
    // creates its items asynchronously, so every binding on `dirModel`
    // evaluated against null before the first tab existed and the whole
    // window came up empty.
    Instantiator {
        id: tabs
        model: tabList
        active: true
        asynchronous: false
        // The delegate IS the model. Wrapping it in a QtObject and exposing
        // the child through an alias left the alias unresolved at the moment
        // the window's bindings first ran, so `dirModel` was null and every
        // view came up empty.
        delegate: DirModel {
            required property string startPath
            // A background tab has not listed anything yet. Opening five
            // tabs must not start five listings (DESIGN §6.6).
            property bool everActivated: false
            // The tab's visible name, as a BINDING on this model. Computing
            // it in a handler read location_display before the model had
            // published the new value, so labels stayed at the old folder.
            readonly property string tabLabel: {
                const p = location_display
                if (p === "") return "…"
                const cut = p.lastIndexOf("/")
                return (cut >= 0 && cut < p.length - 1) ? p.substring(cut + 1) : p
            }
            onTabLabelChanged: win.refreshTabLabels()
        }
        onObjectAdded: win.refreshActiveModel()
        onObjectRemoved: win.refreshActiveModel()
    }


    // Snapshot of the running job, or null. Read by the drawer and QA.
    property var job: null

    // Set once the user has agreed to abandon a transfer, so the second
    // close attempt is not questioned again.
    property bool closing: false

    // A window close must not silently abandon pending transfers (plan §2).
    //
    // This handler covers a close the toolkit tells us about. It is NOT
    // sufficient on its own: Hyprland's `closewindow` was verified not to
    // reach onClosing on Qt 6.11/Wayland — a bare Qt Quick window behaves
    // the same, so it is a platform gap, not Gust's. The durable protection
    // is that a copy stages under a private name and is renamed atomically,
    // and that a listing sweeps staging files left by dead processes, so a
    // killed transfer can never publish a partial file or leave debris.
    onClosing: (close) => {
        if (win.closing || !dirModel.job_running())
            return
        close.accepted = false
        confirm.purpose = "close"
        const detail = (win.job && win.job.message) ? win.job.message + ". " : ""
        confirm.open(
            qsTr("A transfer is still running"),
            detail + qsTr("Closing now stops it. Files already copied are kept; "
                          + "the one in progress is discarded."),
            qsTr("Stop and close"),
            true)
        win.qaDump("close-blocked")
    }

    // Give the worker a moment to notice the cancel and remove its staging
    // file before the process exits, so a close leaves no debris.
    Timer {
        id: closeAfterCancel
        interval: 150
        repeat: true
        property int ticks: 0
        onTriggered: {
            ticks += 1
            if (!dirModel.job_running() || ticks > 20) {
                stop()
                Qt.quit()
            }
        }
    }

    // Live selection summary. Bound to the model, so no handler can leave a
    // stale count on screen by forgetting to refresh it.
    readonly property string selectionText: dirModel.selection_text

    // A refusal or failure the user must read. Any new action clears it, so
    // a stale warning never lingers over unrelated state.
    property string alertText: ""
    /// Aliases of remote places connected right now. Refreshed after every
    /// connect/disconnect rather than polled, so the rail always reflects
    /// what actually happened.
    property var connectedPlaces: []

    function refreshPlaces() {
        let names = []
        try {
            const hosts = JSON.parse(dirModel.remote_suggestions())
            for (let i = 0; i < hosts.length; i++)
                if (hosts[i].connected) names.push(hosts[i].alias)
        } catch (e) { names = [] }
        win.connectedPlaces = names
    }
    width: 1180
    height: 720
    minimumWidth: 520
    minimumHeight: 320
    visible: true
    title: dirModel.location_display + " — Gust"
    color: Tokens.background

    // "standard" (safe default) or "vim". Both keep / for filter, : for
    // actions. Standard uses type-ahead to SELECT, never to search.
    property string keyPreset: "standard"
    property string viewMode: "rows"
    // A cross-location drop defaults to Copy (plan §2). Holding Shift asks
    // for Move explicitly — it is never inferred from where things came from.
    readonly property bool dropAsMove: false
    property bool sidebarVisible: true

    // --- tabs ---------------------------------------------------------
    //
    // A tab IS a DirModel: the model already owns location, history,
    // selection and filter, so a tab needs no new state type (DESIGN §6.6).
    // `dirModel` resolves to the active one, so all 120-odd existing call
    // sites keep working and there is exactly one definition of "current".
    property int activeTab: 0
    // NOT `tabs.objectAt(activeTab)` as a binding: objectAt is a function
    // call with no change notification, so the binding evaluated once while
    // the Instantiator was still empty and stayed null forever. It is a
    // plain property, refreshed whenever the tab set or the active tab
    // changes.
    property var dirModel: null

    function refreshActiveModel() {
        // The Instantiator drops and rebuilds delegates when the tab set
        // changes; objectAt() is null for those frames. Every binding in the
        // tree would throw, so hold the last good model until a real one
        // exists. The UI is never handed a null.
        const m = tabs.objectAt(win.activeTab)
        if (m) win.dirModel = m
        win.refreshTabLabels()
    }

    /// The visible name of each tab: the last path segment, or "/" at the
    /// root. Recomputed on every navigation because a tab is named by where
    /// it currently is.
    function refreshTabLabels() {
        for (let i = 0; i < tabList.count; i++) {
            const m = tabs.objectAt(i)
            const name = m ? m.tabLabel : "…"
            if (tabList.get(i).label !== name)
                tabList.setProperty(i, "label", name)
        }
    }
    onActiveTabChanged: refreshActiveModel()
    // tabList.count, NOT tabs.count: Instantiator's count does not notify,
    // so the strip stayed hidden after a second tab appeared. The ListModel
    // is the source of truth for how many tabs there are.
    readonly property int tabCount: tabList.count

    // Per-tab view mode. Kept beside the tab rather than in the model,
    // because the view lives in QML (the model does not duplicate it).
    property var tabViews: ["rows"]

    function tabModelAt(i) { return tabs.objectAt(i) }

    function newTab(path) {
        const here = path && path !== "" ? path
                   : (dirModel ? dirModel.location : "")
        // Opens BESIDE the active tab, not at the end: "open in new tab"
        // should land where the eye already is.
        const at = activeTab + 1
        tabList.insert(at, { startPath: here, label: "…" })
        tabViews.splice(at, 0, "rows")
        activateTab(at)
        win.qaDump("tab-new")
    }

    function closeTab(i) {
        if (tabList.count <= 1) {
            // A file manager with no tab has nothing to show.
            hints.status = qsTr("This is the last tab")
            return
        }
        const m = tabModelAt(i)
        if (m && m.job_running()) {
            // A running job belongs to the tab that started it. Closing it
            // asks first, exactly as closing the window does.
            win.pendingCloseTab = i
            confirm.purpose = "closetab"
            confirm.open(qsTr("A transfer is still running"),
                         qsTr("Closing this tab stops it. Files already copied "
                              + "are kept; the one in progress is discarded."),
                         qsTr("Stop and close"), true)
            win.qaDump("tab-close-blocked")
            return
        }
        tabList.remove(i)
        tabViews.splice(i, 1)
        if (activeTab >= tabList.count) activeTab = tabList.count - 1
        else if (i < activeTab) activeTab -= 1
        activateTab(activeTab)
        win.qaDump("tab-close")
    }

    property int pendingCloseTab: -1

    function activateTab(i) {
        if (i < 0 || i >= tabList.count) return
        activeTab = i
        const it = tabs.objectAt(i)
        if (it && !it.everActivated) {
            it.everActivated = true
            const p = it.startPath
            if (p && p !== "") it.navigate(p)
            else it.reload()
        }
        win.viewMode = tabViews[i] || "rows"
        win.focusList()
        win.qaDump("tab-activate")
    }

    function nextTab(step) {
        if (tabList.count <= 1) return
        let i = (activeTab + step) % tabList.count
        if (i < 0) i += tabList.count
        activateTab(i)
    }

    // --- helpers -----------------------------------------------------------
    function currentView() {
        return viewMode === "grid" ? gridView
             : viewMode === "columns" ? columnsView
             : rowsView
    }
    function setView(m) {
        viewMode = m
        // Each tab keeps its own view, so switching tabs restores what that
        // tab was showing rather than what the last one chose.
        if (win.activeTab >= 0) win.tabViews[win.activeTab] = m
        // Transient feedback must not outlive what it describes.
        hints.status = ""
        // An EXPLICIT choice, so it is remembered for this location.
        // applyRememberedView() must never call this.
        dirModel.remember_view(m)
        // Only Grid shows thumbnails. Asking for them anywhere else would
        // spend a sandbox launch per file for something nobody can see.
        if (m === "grid")
            dirModel.request_thumbnails()
        focusList()
        qaDump("view")
    }
    // Adopt the view this location should use, without recording anything.
    //
    // A remembered explicit choice wins; otherwise a suggestion (Grid for
    // Pictures); otherwise whatever is already showing stays. Deliberately
    // does NOT call setView(), because that would write the suggestion back
    // as though the user had chosen it.
    function applyRememberedView() {
        const want = dirModel.view_for_here()
        if (want !== "" && want !== viewMode) {
            viewMode = want
            if (want === "grid")
                dirModel.request_thumbnails()
            qaDump("view")
        }
    }

    // Return keyboard control to the file list.
    //
    // Overlays (palette, confirm, filter) take focus into their own text
    // fields; if it is not handed back the window looks frozen to the
    // keyboard. Focus is only taken when no overlay is still up, so this can
    // be called freely.
    function focusList() {
        // Overlays own the keyboard while they are up. The filter BAR is not
        // an overlay: a committed filter keeps narrowing the list while the
        // list itself takes the keys again, so it must not block refocusing
        // or every key after Return goes nowhere.
        if (palette.visible || confirm.visible || prompt.visible)
            return
        listFocus.forceActiveFocus()
    }

    function activateRow(row) {
        const r = dirModel.activate(row)
        if (r.startsWith("blocked:"))
            hints.status = r.substring(8)
        else if (r === "file")
            hints.status = qsTr("Opened ") + dirModel.entry_name(row)
        win.qaDump("activate")
    }

    property int pendingTrashRow: -1

    // --- operations --------------------------------------------------
    function askRename() {
        const row = dirModel.cursor
        const name = dirModel.entry_name(row)
        if (name === "") { hints.status = "Nothing selected"; return }
        prompt.mode = "rename"
        prompt.openWith(qsTr("Rename"), name,
                        qsTr("The extension is kept unless you edit it."), row)
        win.qaDump("prompt")
    }

    function askNewFolder() {
        prompt.mode = "newfolder"
        prompt.openWith(qsTr("New folder"), "", qsTr("Created in this folder."), -1)
        win.qaDump("prompt")
    }

    function doPaste() {
        const err = dirModel.clip_paste()
        if (err !== "") { win.alertText = err; win.qaDump("paste-refused"); return }
        hints.status = qsTr("Started")
        jobTimer.start()
        win.qaDump("paste-started")
    }

    function doDuplicate() {
        const err = dirModel.duplicate_row(dirModel.cursor)
        if (err !== "") { win.alertText = err; win.qaDump("dup-refused"); return }
        hints.status = qsTr("Duplicating…")
        jobTimer.start()
        win.qaDump("dup-started")
    }

    function askTrash() {
        confirm.purpose = "trash"
        if (dirModel.count === 0) return
        const hard = dirModel.delete_needs_hard_confirm()
        pendingTrashRow = dirModel.cursor

        // The confirmation must describe what will ACTUALLY happen. Trash and
        // permanent delete both act on the whole selection, so naming a single
        // file while four are selected is a lie that destroys three more than
        // the user agreed to.
        const n = dirModel.sel_count()
        const subject = n > 1
            ? n + " selected items"
            : "\"" + dirModel.entry_name(dirModel.cursor) + "\""

        confirm.open(
            hard ? "Delete permanently?" : "Move to Trash?",
            hard
              ? subject + " cannot be moved to Trash from this location. "
                + "Deleting " + (n > 1 ? "them" : "it") + " permanently CANNOT be undone."
              : subject + " will be moved to Trash. You can restore "
                + (n > 1 ? "them" : "it") + " later.",
            dirModel.delete_action_label(),
            hard)
    }

    // --- layout ------------------------------------------------------------
    Column {
        anchors.fill: parent

        // Tab strip. Hidden with a single tab: one tab is not a choice, and
        // a strip showing it is chrome that earns nothing.
        Rectangle {
            id: tabStrip
            width: parent.width
            // Height and visibility both come from the SAME condition. Using
            // `height: visible ? X : 0` makes height depend on a sibling
            // binding's evaluation order: visible flipped true while height
            // stayed 0, so the strip was shown and occupied no space at all.
            height: win.tabCount > 1 ? Tokens.controlHeight : 0
            color: Tokens.barBackground
            visible: win.tabCount > 1

            Rectangle {
                anchors.bottom: parent.bottom
                width: parent.width
                height: Math.max(1, Tokens.borderWidth)
                color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
            }

            Row {
                anchors.left: parent.left
                anchors.leftMargin: Tokens.rowPaddingX
                anchors.verticalCenter: parent.verticalCenter
                spacing: Tokens.spaceSm

                Repeater {
                    model: tabList
                    delegate: Rectangle {
                        // `index` is supplied by the Repeater; `label` comes
                        // from the ListModel role. Both must be declared or
                        // the delegate fails to create — silently, leaving an
                        // empty strip and one line in the log.
                        required property int index
                        required property string label
                        readonly property bool current: index === win.activeTab

                        implicitWidth: Math.min(Math.round(Tokens.rowHeight * 7),
                                                Math.max(Math.round(Tokens.rowHeight * 3),
                                                         tabText.implicitWidth
                                                         + Tokens.spaceLg * 2))
                        width: implicitWidth
                        height: Math.round(Tokens.controlHeight * 0.8)
                        radius: Tokens.radius
                        color: current ? Tokens.withAlpha(Tokens.foreground, 0.12)
                                       : "transparent"
                        border.width: current ? Math.max(1, Tokens.borderWidth) : 0
                        border.color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)

                        Text {
                            id: tabText
                            anchors.centerIn: parent
                            width: parent.width - Tokens.spaceLg
                            horizontalAlignment: Text.AlignHCenter
                            elide: Text.ElideMiddle
                            text: parent.label
                            color: parent.current ? Tokens.foreground
                                                  : Tokens.foregroundDim
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontCaption
                        }

                        MouseArea {
                            anchors.fill: parent
                            acceptedButtons: Qt.LeftButton | Qt.MiddleButton
                            onClicked: (mouse) => {
                                if (mouse.button === Qt.MiddleButton)
                                    win.closeTab(parent.index)
                                else
                                    win.activateTab(parent.index)
                            }
                        }
                    }
                }
            }
        }

        // Header: navigation + breadcrumbs + view switch.
        Rectangle {
            id: headerBar
            width: parent.width
            height: Math.round(Tokens.controlHeight * 1.5)
            color: Tokens.barBackground

            Rectangle {
                anchors.bottom: parent.bottom
                width: parent.width
                height: Tokens.borderWidth
                color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
            }

            Row {
                anchors.left: parent.left
                anchors.leftMargin: Tokens.spaceLg
                anchors.verticalCenter: parent.verticalCenter
                spacing: Tokens.spaceSm

                // Sidebar toggle — the visible restore control for the rail.
                Rectangle {
                    width: Tokens.controlHeight
                    height: Tokens.controlHeight
                    color: sbHover.hovered ? Tokens.withAlpha(Tokens.foreground, 0.1) : "transparent"
                    radius: Tokens.radius
                    objectName: "sidebarToggle"
                    HoverHandler { id: sbHover }
                    Text {
                        anchors.centerIn: parent
                        text: "☰"
                        color: Tokens.foreground
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                    }
                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: win.sidebarVisible = !win.sidebarVisible
                    }
                    Accessible.role: Accessible.Button
                    Accessible.name: win.sidebarVisible ? "Hide places sidebar" : "Show places sidebar"
                }

                Repeater {
                    model: [
                        { glyph: "‹", act: "back",    nm: "Back" },
                        { glyph: "›", act: "forward", nm: "Forward" },
                        { glyph: "↑", act: "up",      nm: "Up one folder" }
                    ]
                    delegate: Rectangle {
                        width: Tokens.controlHeight
                        height: Tokens.controlHeight
                        radius: Tokens.radius
                        objectName: "nav-" + modelData.act
                        property bool enabledAct: modelData.act === "back" ? dirModel.can_go_back()
                                                : modelData.act === "forward" ? dirModel.can_go_forward()
                                                : true
                        color: navHover.hovered && enabledAct
                               ? Tokens.withAlpha(Tokens.foreground, 0.1) : "transparent"
                        HoverHandler { id: navHover }
                        Text {
                            anchors.centerIn: parent
                            text: modelData.glyph
                            color: parent.enabledAct ? Tokens.foreground : Tokens.withAlpha(Tokens.foreground, 0.3)
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSubtitle
                        }
                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: {
                                if (modelData.act === "back") dirModel.go_back()
                                else if (modelData.act === "forward") dirModel.go_forward()
                                else dirModel.navigate_up()
                                win.focusList()
                            }
                        }
                        Accessible.role: Accessible.Button
                        Accessible.name: modelData.nm
                    }
                }

                Item { width: Tokens.spaceLg; height: 1 }

                Breadcrumbs {
                    id: crumbs
                    model: dirModel
                    anchors.verticalCenter: parent.verticalCenter
                    onPathChosen: (p) => { dirModel.navigate(p); win.focusList() }
                }
            }

            // View switch: labelled, tooltipped, keyboard reachable.
            Row {
                anchors.right: parent.right
                anchors.rightMargin: Tokens.spaceLg
                anchors.verticalCenter: parent.verticalCenter
                spacing: Tokens.spaceXs

                Repeater {
                    model: [
                        { m: "rows",    glyph: "≡", nm: "Rows view" },
                        { m: "grid",    glyph: "▦", nm: "Grid view" },
                        { m: "columns", glyph: "⫼", nm: "Columns view" }
                    ]
                    delegate: Rectangle {
                        width: Tokens.controlHeight
                        height: Tokens.controlHeight
                        objectName: "viewswitch-" + modelData.m
                        color: win.viewMode === modelData.m
                               ? Tokens.withAlpha(Tokens.accent, 0.22)
                               : (vsHover.hovered ? Tokens.withAlpha(Tokens.foreground, 0.1) : "transparent")
                        border.width: win.viewMode === modelData.m ? Math.max(1, Tokens.borderWidth) : 0
                        border.color: Tokens.accent
                        radius: Tokens.radius

                        HoverHandler { id: vsHover }

                        Text {
                            anchors.centerIn: parent
                            text: modelData.glyph
                            color: win.viewMode === modelData.m ? Tokens.foregroundBright : Tokens.foreground
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontBody
                        }
                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: win.setView(modelData.m)
                        }
                        Accessible.role: Accessible.Button
                        Accessible.name: modelData.nm
                    }
                }
            }
        }

        // Body: sidebar + view.
        Item {
            id: body
            width: parent.width
            height: parent.height - headerBar.height - filterBar.height
                    - jobDrawer.height - hints.height

            Sidebar {
                id: sidebar
                width: win.sidebarVisible ? Math.round(180 * Tokens.spacingScale) : 0
                height: parent.height
                visible: win.sidebarVisible
                model: dirModel
                onPlaceChosen: (p) => { dirModel.navigate(p); win.focusList() }
                remotePlaces: win.connectedPlaces
            }

            Item {
                x: sidebar.visible ? sidebar.width : 0
                width: parent.width - x
                height: parent.height

                RowsView {
                    id: rowsView
                    anchors.fill: parent
                    visible: win.viewMode === "rows"
                    model: dirModel
                    onActivated: (r) => win.activateRow(r)
                    onSelectionTouched: {
                        win.qaDump("select")
                    }
                    onDragStarted: (r) => {
                        rowsView.dragActive = true
                        dirModel.drag_begin(r)
                        win.qaDump("drag-begin")
                    }
                    // Say what a drop would do BEFORE the button is released,
                    // so the operation and destination are never a surprise.
                    onDropMovedOver: (r) => {
                        hints.status = dirModel.drop_preview(r, win.dropAsMove)
                    }
                    onDropReleased: (r) => {
                        const err = dirModel.drop_on_row(r, win.dropAsMove)
                        if (err !== "") {
                            win.alertText = err
                        } else {
                            hints.status = ""
                        }
                        win.qaDump("drop")
                    }
                }
                IconGridView {
                    id: gridView
                    anchors.fill: parent
                    visible: win.viewMode === "grid"
                    model: dirModel
                    onActivated: (r) => win.activateRow(r)
                    onSelectionTouched: {
                        win.qaDump("select")
                    }
                    onDragStarted: (r) => {
                        gridView.dragActive = true
                        win.qaDump("drag-begin")
                    }
                    // Say what a drop would do BEFORE the button is released,
                    // so the operation and destination are never a surprise.
                    onDropMovedOver: (r) => {
                        hints.status = dirModel.drop_preview(r, win.dropAsMove)
                    }
                    onDropReleased: (r) => {
                        const err = dirModel.drop_on_row(r, win.dropAsMove)
                        if (err !== "") {
                            win.alertText = err
                        } else {
                            hints.status = ""
                        }
                        win.qaDump("drop")
                    }
                }
                ColumnsView {
                    id: columnsView
                    anchors.fill: parent
                    visible: win.viewMode === "columns"
                    model: dirModel
                    onActivated: (r) => win.activateRow(r)
                    onSelectionTouched: {
                        win.qaDump("select")
                    }
                    onDragStarted: (r) => {
                        columnsView.dragActive = true
                        win.qaDump("drag-begin")
                    }
                    // Say what a drop would do BEFORE the button is released,
                    // so the operation and destination are never a surprise.
                    onDropMovedOver: (r) => {
                        hints.status = dirModel.drop_preview(r, win.dropAsMove)
                    }
                    onDropReleased: (r) => {
                        const err = dirModel.drop_on_row(r, win.dropAsMove)
                        if (err !== "") {
                            win.alertText = err
                        } else {
                            hints.status = ""
                        }
                        win.qaDump("drop")
                    }
                }
            }
        }

        // Filter bar, shown only while filtering.
        Rectangle {
            id: filterBar
            width: parent.width
            height: filterActive ? Tokens.controlHeight : 0
            visible: filterActive
            color: Tokens.backgroundDark
            property bool filterActive: false
            onFilterActiveChanged: win.qaDump("filterbar")

            Row {
                anchors.fill: parent
                anchors.leftMargin: Tokens.rowPaddingX
                spacing: Tokens.spaceLg

                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    text: "/"
                    color: Tokens.accent
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                }
                TextInput {
                    id: filterInput
                    objectName: "filterInput"
                    anchors.verticalCenter: parent.verticalCenter
                    width: parent.width - Tokens.spaceXxl * 3
                    color: Tokens.foregroundBright
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                    selectByMouse: true
                    // While this field has focus it owns every key: h/j/k/l,
                    // / and : are text here, not commands.
                    onTextChanged: dirModel.set_filter_text(text)
                    Keys.onEscapePressed: {
                        text = ""
                        dirModel.set_filter_text("")
                        filterBar.filterActive = false
                        // Focus must go back to the list or every later key
                        // press lands in this hidden field.
                        win.focusList()
                    }
                    Keys.onReturnPressed: win.focusList()
                    Accessible.role: Accessible.EditableText
                    Accessible.name: "Filter this folder"
                }
            }
        }

        JobDrawer {
            id: jobDrawer
            width: parent.width
            model: dirModel
            job: win.job
            onRequestCancel: {
                dirModel.cancel_job()
                hints.status = qsTr("Cancelling…")
                win.qaDump("job-cancel")
            }
        }

        HintStrip {
            id: hints
            width: parent.width
            preset: win.keyPreset
            status: dirModel.status
            // Bound, not assigned: a selection is live state and must not go
            // stale behind a message some earlier keystroke left here.
            selection: win.selectionText
            alert: win.alertText !== "" ? win.alertText : dirModel.notice
            onStatusChanged: win.qaDump("hint")
        }
    }

    // --- keyboard ----------------------------------------------------------
    // A single focus item owns file-list keys. Text fields take focus away, so
    // typing in them can never trigger a file action.
    Item {
        id: listFocus
        objectName: "listFocus"
        anchors.fill: parent
        focus: true
        z: -1

        Keys.onPressed: (event) => {
            // A warning is about the action that produced it. The next
            // keystroke moves on, so it must not linger over unrelated state.
            if (win.alertText !== "" && event.key !== Qt.Key_Z)
                win.alertText = ""

            // Filter and palette are shared by both presets.
            if (event.text === "/") {
                filterBar.filterActive = true
                filterInput.forceActiveFocus()
                event.accepted = true; return
            }
            if (event.text === ":") {
                palette.open()
                event.accepted = true; return
            }

            // Navigation common to both presets.
            // In Grid, Up/Down move a whole row (two-dimensional selection);
            // Left/Right move one tile. Rows/Columns move one entry.
            const step = win.viewMode === "grid" ? gridView.columns() : 1
            const extend = (event.modifiers & Qt.ShiftModifier) !== 0

            // Shift+movement extends from the anchor; plain movement drops
            // the selection so the next command cannot act on a stale set.
            if (extend && (event.key === Qt.Key_Down || event.key === Qt.Key_Up
                        || event.key === Qt.Key_Home || event.key === Qt.Key_End)) {
                let target = dirModel.cursor
                if (event.key === Qt.Key_Down) target = dirModel.cursor + step
                else if (event.key === Qt.Key_Up) target = dirModel.cursor - step
                else if (event.key === Qt.Key_Home) target = 0
                else target = dirModel.count - 1
                target = Math.max(0, Math.min(dirModel.count - 1, target))
                dirModel.sel_range_to(target)
                win.qaDump("select")
                event.accepted = true; return
            }

            switch (event.key) {
            case Qt.Key_Space:
                dirModel.sel_toggle(dirModel.cursor)
                win.qaDump("select")
                event.accepted = true; return
            case Qt.Key_Down:  dirModel.move_cursor(step);  event.accepted = true; return
            case Qt.Key_Up:    dirModel.move_cursor(-step); event.accepted = true; return
            case Qt.Key_Right:
                if (win.viewMode === "grid") { dirModel.move_cursor(1); event.accepted = true; return }
                break
            case Qt.Key_Left:
                if (win.viewMode === "grid") { dirModel.move_cursor(-1); event.accepted = true; return }
                break
            case Qt.Key_Home:  dirModel.focus_row(0);   event.accepted = true; return
            case Qt.Key_End:   dirModel.focus_row(dirModel.count - 1); event.accepted = true; return
            case Qt.Key_PageDown: dirModel.move_cursor(10);  event.accepted = true; return
            case Qt.Key_PageUp:   dirModel.move_cursor(-10); event.accepted = true; return
            case Qt.Key_Return:
            case Qt.Key_Enter:
                win.activateRow(dirModel.cursor); event.accepted = true; return
            case Qt.Key_Backspace:
                dirModel.navigate_up(); event.accepted = true; return
            case Qt.Key_F5:
                dirModel.reload(); event.accepted = true; return
            case Qt.Key_Delete:
                win.askTrash(); event.accepted = true; return
            case Qt.Key_Escape:
                // Escape unwinds one layer at a time: a running job first,
                // then the selection, then the filter. The plan's mockup
                // shows "esc cancel" against a transfer, and a keyboard user
                // otherwise has no way to stop one.
                //
                // It cancels only THIS window's job — never a globally
                // pressed Escape reaching some unrelated transfer.
                if (dirModel.job_running()) {
                    dirModel.cancel_job()
                    hints.status = qsTr("Cancelling…")
                    win.qaDump("job-cancel")
                    event.accepted = true; return
                }
                if (dirModel.finding) {
                    // Leaving search results returns to the folder the user
                    // was browsing, not to wherever a result happened to be.
                    dirModel.find_clear()
                    win.qaDump("find-clear")
                    event.accepted = true; return
                }
                if (dirModel.sel_count() > 0) {
                    dirModel.sel_clear()
                    win.qaDump("select")
                    event.accepted = true; return
                }
                if (filterBar.filterActive || dirModel.filter !== "") {
                    filterInput.text = ""
                    dirModel.set_filter_text("")
                    filterBar.filterActive = false
                    hints.status = ""
                    win.qaDump("filter-cleared")
                    event.accepted = true; return
                }
                break
            case Qt.Key_F1:
                hints.status = win.keyPreset === "vim"
                    ? "vim: h/j/k/l move, l open, / filter, : command"
                    : "arrows move, Enter open, Backspace up, / filter, : command"
                event.accepted = true; return
            }

            if (event.modifiers & Qt.ControlModifier) {
                if (event.key === Qt.Key_A) {
                    dirModel.sel_all()
                    win.qaDump("select")
                    event.accepted = true; return
                }
                if (event.key === Qt.Key_H) { dirModel.toggle_hidden(); event.accepted = true; return }
                if (event.key === Qt.Key_L) { palette.open(); event.accepted = true; return }
                // Tabs (plan §2). Ctrl+T opens beside this one, Ctrl+W closes
                // it, Tab / Shift+Tab cycle.
                if (event.key === Qt.Key_T) { win.newTab(""); event.accepted = true; return }
                if (event.key === Qt.Key_W) { win.closeTab(win.activeTab); event.accepted = true; return }
                if (event.key === Qt.Key_Tab) {
                    win.nextTab((event.modifiers & Qt.ShiftModifier) ? -1 : 1)
                    event.accepted = true; return
                }
                if (event.key === Qt.Key_Backtab) {
                    win.nextTab(-1); event.accepted = true; return
                }
                if (event.key === Qt.Key_R) { dirModel.reload(); event.accepted = true; return }
                if (event.key === Qt.Key_Z) {
                    const label = dirModel.undo_label()
                    const err = dirModel.undo_last()
                    if (err === "") {
                        win.alertText = ""
                        hints.status = label.startsWith("Undo: remove") ? "Copies removed"
                                     : label.startsWith("Undo: move") ? "Moved back"
                                     : "Restored from Trash"
                    } else {
                        win.alertText = "Cannot undo: " + err
                    }
                    win.qaDump("undo")
                    event.accepted = true; return
                }
                if (event.key === Qt.Key_C) {
                    hints.status = dirModel.clip_copy(dirModel.cursor)
                    win.qaDump("clip"); event.accepted = true; return
                }
                if (event.key === Qt.Key_X) {
                    hints.status = dirModel.clip_cut(dirModel.cursor)
                    win.qaDump("clip"); event.accepted = true; return
                }
                if (event.key === Qt.Key_V) { win.doPaste(); event.accepted = true; return }
                if (event.key === Qt.Key_D) { win.doDuplicate(); event.accepted = true; return }
                if (event.key === Qt.Key_N) { win.askNewFolder(); event.accepted = true; return }
                return
            }
            if (event.key === Qt.Key_F2) { win.askRename(); event.accepted = true; return }

            if (win.keyPreset === "vim") {
                // Bare letters navigate. Type-ahead is NOT active in this
                // preset: two features cannot own the same keys.
                switch (event.key) {
                case Qt.Key_J: dirModel.move_cursor(1);  event.accepted = true; return
                case Qt.Key_K: dirModel.move_cursor(-1); event.accepted = true; return
                case Qt.Key_L: win.activateRow(dirModel.cursor); event.accepted = true; return
                case Qt.Key_H: dirModel.navigate_up(); event.accepted = true; return
                case Qt.Key_G: dirModel.focus_row(0); event.accepted = true; return
                case Qt.Key_V: win.setView(win.viewMode === "rows" ? "grid"
                                        : win.viewMode === "grid" ? "columns" : "rows")
                               event.accepted = true; return
                case Qt.Key_R: win.askRename(); event.accepted = true; return
                case Qt.Key_X: dirModel.sel_toggle(dirModel.cursor)
                               win.qaDump("select"); event.accepted = true; return
                case Qt.Key_Y: hints.status = dirModel.clip_copy(dirModel.cursor)
                               win.qaDump("clip"); event.accepted = true; return
                case Qt.Key_D: hints.status = dirModel.clip_cut(dirModel.cursor)
                               win.qaDump("clip"); event.accepted = true; return
                case Qt.Key_P: win.doPaste(); event.accepted = true; return
                }
                return
            }

            // Standard preset: printable keys select a matching row in THIS
            // folder. They never start a search (Nautilus #1157).
            if (event.text.length === 1 && event.text >= " ") {
                const row = dirModel.type_ahead(event.text)
                hints.status = row >= 0
                    ? "Selected " + dirModel.entry_name(row)
                    : "No file starting with \"" + event.text + "\""
                event.accepted = true
            }
        }
    }

    // Poll the worker thread. The UI thread never touches the filesystem;
    // it only reads a snapshot the job publishes.
    Timer {
        id: jobTimer
        interval: 100
        repeat: true
        running: false
        onTriggered: {
            const raw = dirModel.poll_job()
            if (raw === "") {
                // Job finished and was cleared by the model.
                jobTimer.stop()
                win.qaDump("job-done")
                return
            }
            try {
                win.job = JSON.parse(raw)
            } catch (e) {
                win.job = null
            }
            if (win.job && win.job.state !== "Running") {
                jobTimer.stop()
                hints.status = win.job.message
                win.qaDump("job-finished")
            }
        }
    }

    TextPrompt {
        id: prompt
        anchors.fill: parent
        property string mode: ""
        onVisibleChanged: win.qaDump("prompt")
        onClosed: win.focusList()
        onAccepted: (text) => {
            let err = ""
            if (prompt.mode === "rename")
                err = dirModel.rename_row(prompt.targetRow, text)
            else if (prompt.mode === "newfolder")
                err = dirModel.new_folder(text)

            if (err === "") {
                hints.status = prompt.mode === "rename"
                    ? qsTr("Renamed to ") + text
                    : qsTr("Created ") + text
                prompt.close()
                win.qaDump("prompt-ok")
            } else {
                // Stay open so the name can be corrected.
                prompt.showError(err)
                win.qaDump("prompt-error")
            }
        }
    }

    CommandPalette {
        id: palette
        model: dirModel
        onVisibleChanged: win.qaDump("palette")
        onClosed: win.focusList()
        onNavigateRequested: (p) => { dirModel.navigate(p); win.focusList() }
        onActionRequested: (a) => {
            if (a === "view-rows") win.setView("rows")
            else if (a === "view-grid") win.setView("grid")
            else if (a === "view-columns") win.setView("columns")
            else if (a === "toggle-hidden") dirModel.toggle_hidden()
            else if (a === "reload") dirModel.reload()
            else if (a === "up") dirModel.navigate_up()
            else if (a === "trash") win.askTrash()
            else if (a === "new-folder") win.askNewFolder()
            else if (a === "rename") win.askRename()
            else if (a === "duplicate") win.doDuplicate()
            else if (a === "copy") hints.status = dirModel.clip_copy(dirModel.cursor)
            else if (a === "cut") hints.status = dirModel.clip_cut(dirModel.cursor)
            else if (a === "paste") win.doPaste()
            else if (a === "terminal") hints.status = dirModel.open_terminal()
            else if (a === "keys") hints.status = win.keyPreset === "vim"
                ? "vim: h/j/k/l move, l open, r rename, y/d/p copy/cut/paste"
                : "arrows move, Enter open, F2 rename, Ctrl+C/X/V, Ctrl+N new folder"
            else if (a === "noop") { /* nothing to do */ }
            else if (a.startsWith("remote-on:")) {
                const alias = a.substring(10)
                hints.status = "Connecting to " + alias + "…"
                // Bounded inside the bridge; a stuck server fails rather
                // than freezing the window.
                const err = dirModel.remote_connect(alias)
                if (err !== "") win.alertText = err
                win.refreshPlaces()
                win.qaDump("remote-connect")
            }
            else if (a.startsWith("remote-off:")) {
                const alias2 = a.substring(11)
                const err2 = dirModel.remote_disconnect(alias2)
                if (err2 !== "") win.alertText = err2
                win.refreshPlaces()
                win.qaDump("remote-disconnect")
            }
            else if (a.startsWith("find:")) {
                dirModel.find_start(a.substring(5))
                win.qaDump("find-start")
            }
            else if (a.startsWith("filter:")) {
                filterBar.filterActive = true
                filterInput.text = a.substring(7)
            }
            else hints.status = a + " is not wired up yet"
            win.focusList()
        }
    }

    ConfirmDialog {
        id: confirm
        objectName: "confirmDialog"
        // What this confirmation is FOR. Two different questions must never
        // share one handler by accident.
        property string purpose: "trash"
        onVisibleChanged: win.qaDump("confirm")
        onClosed: win.focusList()
        onAccepted: {
            if (confirm.purpose === "closetab") {
                const i = win.pendingCloseTab
                win.pendingCloseTab = -1
                if (i < 0) return
                const m = win.tabModelAt(i)
                if (m) m.cancel_job()
                // Give the worker a moment to notice, then close the tab —
                // the same courtesy the window close already extends.
                closeTabAfterCancel.target = i
                closeTabAfterCancel.ticks = 0
                closeTabAfterCancel.start()
                win.qaDump("tab-close-confirmed")
                return
            }
            if (confirm.purpose === "close") {
                // The user chose to abandon the transfer. Cancel it first so
                // the worker stops and its staging file is cleaned up, then
                // close once it has actually stopped.
                dirModel.cancel_job()
                win.closing = true
                win.qaDump("close-confirmed")
                closeAfterCancel.start()
                return
            }
            if (win.pendingTrashRow < 0) return
            if (dirModel.delete_needs_hard_confirm()) {
                // Trash is genuinely unavailable here, and the dialog said
                // so in those words with Cancel focused. There is no undo.
                const perr = dirModel.delete_selection_permanently(win.pendingTrashRow)
                win.pendingTrashRow = -1
                // A refusal is an ALERT, not a status line: status describes
                // the last action and the reload that follows overwrites it,
                // which is how this silently did nothing before.
                if (perr !== "") win.alertText = perr
                win.qaDump("delete-permanent")
                return
            }
            const err = dirModel.trash_row(win.pendingTrashRow)
            win.pendingTrashRow = -1
            hints.status = err === ""
                ? dirModel.undo_label() + "  (Ctrl+Z)"
                : "Could not move to Trash: " + err
            win.qaDump("trashed")
        }
        onRejected: {
            if (confirm.purpose === "closetab") {
                const i = win.pendingCloseTab
                win.pendingCloseTab = -1
                if (i < 0) return
                const m = win.tabModelAt(i)
                if (m) m.cancel_job()
                // Give the worker a moment to notice, then close the tab —
                // the same courtesy the window close already extends.
                closeTabAfterCancel.target = i
                closeTabAfterCancel.ticks = 0
                closeTabAfterCancel.start()
                win.qaDump("tab-close-confirmed")
                return
            }
            if (confirm.purpose === "close") {
                confirm.purpose = "trash"
                hints.status = qsTr("Transfer continues")
                win.qaDump("close-cancelled")
                win.focusList()
                return
            }
            hints.status = "Cancelled — nothing was changed"
            win.pendingTrashRow = -1
            win.focusList()
        }
    }

    // QA state probe. Enabled ONLY when GUST_QA_STATE names a file, so an
    // ordinary run writes nothing and has no extra surface. The harness reads
    // this instead of guessing from OCR.
    property string qaStatePath: ""
    property int qaTick: 0

    function qaDump(tag) {
        if (qaStatePath === "") return
        const payload = {
            tag: tag,
            view: win.viewMode,
            preset: win.keyPreset,
            filterBarActive: filterBar.filterActive,
            finding: dirModel.finding,
            findNeedle: dirModel.findNeedle,
            tabCount: win.tabCount,
            activeTab: win.activeTab,
            tabPaths: (function () {
                let out = []
                for (let i = 0; i < win.tabCount; i++) {
                    const m = win.tabModelAt(i)
                    out.push(m ? m.location : "")
                }
                return out
            })(),
            paletteOpen: palette.visible,
            promptOpen: prompt.visible,
            promptMode: prompt.mode,
            promptError: prompt.errorText,
            clip: dirModel.clip_summary(),
            selCount: dirModel.sel_count(),
            selSummary: dirModel.sel_summary(),
            hintShown: hints.displayText,
            alertText: win.alertText,
            jobRunning: dirModel.job_running(),
            jobState: win.job ? win.job.state : "",
            jobMessage: win.job ? win.job.message : "",
            jobRefused: win.job ? win.job.refused : 0,
            confirmOpen: confirm.visible,
            confirmTitle: confirm.title,
            confirmBody: confirm.body,
            confirmIsDangerous: confirm.dangerous,
            hintStatus: hints.status,
            hintAlert: hints.alert,
            notice: dirModel.notice,
            undoLabel: dirModel.undo_label(),
            model: JSON.parse(dirModel.state_json())
        }
        qaWriter.write(qaStatePath, JSON.stringify(payload, null, 2))
    }

    // A cancelled job needs a moment to stop and clean up its staging file
    // before its tab disappears, or the debris outlives the tab that made it.
    Timer {
        id: closeTabAfterCancel
        interval: 150
        repeat: true
        property int ticks: 0
        property int target: -1
        onTriggered: {
            ticks += 1
            const m = win.tabModelAt(target)
            if (!m || !m.job_running() || ticks > 20) {
                stop()
                if (target >= 0 && tabList.count > 1) {
                    tabList.remove(target)
                    win.tabViews.splice(target, 1)
                    if (win.activeTab >= tabList.count)
                        win.activeTab = tabList.count - 1
                    win.activateTab(win.activeTab)
                }
                target = -1
                win.qaDump("tab-close")
            }
        }
    }

    QaWriter { id: qaWriter }

    // Anything that can change what the user sees re-dumps the state.
    Connections {
        target: dirModel
        function onLocationChanged() {
            win.refreshTabLabels()
            // A tab's view follows the location it is showing.
            if (win.activeTab >= 0) win.tabViews[win.activeTab] = win.viewMode
            // Adopt this location's remembered view before anything else
            // reacts to the new folder.
            win.applyRememberedView()
            win.qaDump("location")
        }
        function onCountChanged()    { win.qaDump("count") }
        function onCursorChanged()   { win.qaDump("cursor") }
        function onFilterChanged()   { win.qaDump("filter") }
        function onStatusChanged()   { win.qaDump("status") }
        function onSettledChanged()  { win.qaDump("settled") }
        function onShow_hiddenChanged() { win.qaDump("hidden") }
    }

    Component.onCompleted: {
        // The model does not list anything until it is told to: construction
        // must not do I/O. This is the one explicit kick at startup.
        dirModel.reload()
        // The folder Gust opens with gets its remembered view too.
        applyRememberedView()
        qaStatePath = qaWriter.env_path("GUST_QA_STATE")
        console.log("GUST_READY location=" + dirModel.location
                    + " count=" + dirModel.count
                    + " theme=" + Tokens.themeName)
        win.focusList()
        qaDump("ready")
    }
}
