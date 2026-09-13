import QtQml
import QtQuick
import QtQuick.Window
import com.gust.app

Window {
    id: win

    ListModel {
        id: tabList
        ListElement { startPath: ""; label: "…"; pane: 0 }
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

    /// Removable devices, refreshed when the window opens and after any
    /// mount or eject. Not polled: a rail that lies about what is attached
    /// is worse than one that says nothing (DESIGN §6.13).
    property var deviceList: []

    function refreshDevices() {
        try {
            win.deviceList = JSON.parse(dirModel.devices_json())
        } catch (e) {
            win.deviceList = []
        }
    }

    // --- network shares (DESIGN §6.14) -----------------------------------
    property string shareAddress: ""
    property string shareUser: ""

    /// Ask for the address first. A typo is caught here, before anybody
    /// types a secret.
    function askShareAddress() {
        win.shareAddress = ""
        win.shareUser = ""
        prompt.secret = false
        prompt.mode = "share-address"
        prompt.openWith(qsTr("Shared folder address"), "",
                        qsTr("//nas/media for Windows sharing, "
                             + "or nfs://server/export for NFS"))
        win.qaDump("share-prompt")
    }

    function shareAddressEntered(text) {
        const bad = dirModel.share_check(text)
        if (bad !== "") {
            prompt.errorText = bad
            win.qaDump("share-bad-address")
            return
        }
        win.shareAddress = text
        // NFS authorises by address, not by account. Asking for a username
        // and password it will never send would be theatre.
        if (text.trim().toLowerCase().startsWith("nfs://")) {
            prompt.close()
            win.shareUser = ""
            hints.status = qsTr("Connecting…")
            const nerr = dirModel.share_mount(text, "", "", "")
            if (nerr !== "") win.alertText = nerr
            win.shareAddress = ""
            win.qaDump("share-mount")
            return
        }
        prompt.secret = false
        prompt.mode = "share-user"
        prompt.openWith(qsTr("Username for that share"), "",
                        qsTr("Leave empty if the share needs no account"))
        win.qaDump("share-prompt")
    }

    function shareUserEntered(text) {
        win.shareUser = text
        // The only masked prompt in Gust. The value goes straight to GIO
        // and is not kept afterwards.
        prompt.secret = true
        prompt.mode = "share-password"
        prompt.openWith(qsTr("Password"), "",
                        qsTr("Sent to the server, never stored by Gust"))
        win.qaDump("share-prompt")
    }

    function sharePasswordEntered(text) {
        hints.status = qsTr("Connecting…")
        const err = dirModel.share_mount(win.shareAddress, win.shareUser, "", text)
        // Whatever happened, the secret does not stay in QML.
        prompt.secret = false
        win.shareAddress = ""
        win.shareUser = ""
        if (err !== "") win.alertText = err
        win.qaDump("share-mount")
    }

    // --- bulk rename (DESIGN §6.16) --------------------------------------
    property string bulkPattern: ""
    property string bulkLines: ""
    property string bulkSummary: ""

    /// Opens the preview once the rename prompt has finished fading, so the
    /// prompt's own refocus cannot steal the keyboard from the dialog.
    Timer {
        id: bulkConfirmAfterPrompt
        interval: Tokens.animFast + 60
        repeat: false
        onTriggered: {
            confirm.purpose = "bulk"
            confirm.open(win.bulkSummary, win.bulkLines, qsTr("Rename"), false)
            win.qaDump("bulk-confirm-open")
        }
    }

    function askBulkRename() {
        if (!dirModel || dirModel.count === 0) return
        prompt.secret = false
        prompt.mode = "bulk"
        prompt.openWith(qsTr("Rename these files"), "{name}.{ext}",
                        qsTr("{n} numbers them · {name} is the name · {ext} the extension"))
        win.qaDump("bulk-prompt")
    }

    /// Show the plan before anything happens. A bulk rename the user has
    /// not seen is the most dangerous ordinary operation there is.
    function bulkPreview(pattern) {
        win.bulkPattern = pattern
        let plan = null
        try {
            plan = JSON.parse(dirModel.bulk_preview(dirModel.cursor, pattern))
        } catch (e) {
            plan = null
        }
        if (!plan) {
            prompt.errorText = qsTr("That pattern could not be read")
            win.qaDump("bulk-bad")
            return
        }
        if (!plan.ok) {
            // Stay in the prompt with the reason, so the pattern can be
            // corrected without retyping it.
            prompt.errorText = plan.summary
                + (plan.problems.length > 0 ? "  ·  " + plan.problems[0] : "")
            win.qaDump("bulk-refused")
            return
        }

        // Build the old → new list the user confirms against.
        let lines = []
        for (let i = 0; i < plan.rows.length && i < 12; i++) {
            const r = plan.rows[i]
            if (r.unchanged) continue
            lines.push(r.from + "  →  " + r.to)
        }
        if (plan.changing > lines.length)
            lines.push("… and " + (plan.changing - lines.length) + " more")

        // NOT confirm.open() straight after prompt.close(): closing the
        // prompt schedules a focusList() for when its fade finishes, and
        // that would land after the dialog took focus and hand the keyboard
        // back to the file list. Open the dialog once the prompt is gone.
        win.bulkLines = lines.join("\n")
        win.bulkSummary = plan.summary
        prompt.close()
        bulkConfirmAfterPrompt.start()
        win.qaDump("bulk-confirm")
    }

    function bulkApply() {
        const err = dirModel.bulk_apply(dirModel.cursor, win.bulkPattern)
        if (err !== "") win.alertText = err
        win.bulkPattern = ""
        win.qaDump("bulk-done")
    }

    /// Eject a device. Refused while a transfer is running, because pulling
    /// a filesystem out from under a copy is how people lose data.
    function ejectDevice(path) {
        const err = dirModel.device_unmount(path)
        if (err !== "") win.alertText = err
        else hints.status = qsTr("Safe to remove")
        win.refreshDevices()
        win.qaDump("device-eject")
    }

    /// A device row was clicked: browse it if it is mounted, otherwise
    /// mount it. Gust never mounts anything on its own.
    function openDevice(path, mounted) {
        if (mounted) {
            const d = win.deviceList.find((x) => x.path === path)
            if (d && d.mountPoint) dirModel.navigate(d.mountPoint)
            else win.alertText = qsTr("That device is mounted, but Gust could not tell where.")
            return
        }
        hints.status = qsTr("Mounting…")
        const err = dirModel.device_mount(path)
        if (err !== "") win.alertText = err
        win.refreshDevices()
        win.qaDump("device-mount")
    }

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
    // --- panes ----------------------------------------------------------
    //
    // At most two, side by side (DESIGN §6.7). A pane owns a set of tabs and
    // which one of them is showing; a tab already owns everything else, so
    // there is no third kind of state.
    property int paneCount: 1
    property int activePane: 0
    property real paneRatio: 0.5
    /// Each pane's active tab, as an INDEX INTO tabList.
    property var paneTab: [0, -1]

    /// The active tab of the active pane. Every existing call site that says
    /// `activeTab` keeps meaning "the tab the user is working in".
    property int activeTab: 0

    /// Indices of the tabs belonging to a pane, in list order.
    function paneTabs(p) {
        const out = []
        for (let i = 0; i < tabList.count; i++)
            if (tabList.get(i).pane === p) out.push(i)
        return out
    }
    /// The model a pane is showing, or null if it has no tabs.
    function paneModel(p) {
        const t = win.paneTab[p]
        return (t >= 0 && t < tabList.count) ? tabs.objectAt(t) : null
    }
    /// A pane's view mode is the view of the tab it is showing.
    function paneViewMode(p) {
        const t = win.paneTab[p]
        return (t >= 0 && t < tabViews.length) ? (tabViews[t] || "rows") : "rows"
    }

    /// Make a pane active. Focus moves only on an explicit action, never
    /// because a job finished or a listing settled (DESIGN §6.7 rule 1).
    function focusPane(p) {
        if (p < 0 || p >= win.paneCount) return
        if (win.activePane === p) return
        win.activePane = p
        win.activeTab = win.paneTab[p]
        win.refreshActiveModel()
        win.viewMode = win.paneViewMode(p)
        win.focusList()
        win.refreshPaneTabCount()
        win.qaDump("pane-focus")
    }

    /// Open the second pane, moving the active tab into it when there is
    /// more than one to spare, or opening a fresh tab there when there is not.
    function splitPane() {
        if (win.paneCount > 1) {
            hints.status = qsTr("Already two panes")
            return
        }
        win.paneCount = 2
        const mine = win.paneTabs(0)
        if (mine.length > 1) {
            // Move the active tab across: the user asked for this location
            // beside the others, not for a duplicate of it.
            const moving = win.activeTab
            tabList.setProperty(moving, "pane", 1)
            win.paneTab = [win.firstTabOf(0), moving]
        } else {
            // The only tab stays put; the new pane opens where it is, so the
            // split shows something immediately rather than an empty half.
            const here = win.dirModel ? win.dirModel.location : ""
            tabList.append({ startPath: here, label: "…", pane: 1 })
            tabViews.push("rows")
            win.paneTab = [win.paneTab[0], tabList.count - 1]
        }
        win.activePane = 1
        win.activeTab = win.paneTab[1]
        win.activateTab(win.activeTab)
        win.refreshPaneTabCount()
        win.qaDump("pane-split")
    }

    /// Close a pane. Its tabs move to the survivor rather than being
    /// discarded: a pane is a layout, and closing a layout must not lose a
    /// location, still less a running job (DESIGN §6.7 rule 2).
    function closePane(p) {
        if (win.paneCount < 2) {
            hints.status = qsTr("This is the only pane")
            return
        }
        const keep = p === 0 ? 1 : 0
        for (let i = 0; i < tabList.count; i++)
            if (tabList.get(i).pane === p) tabList.setProperty(i, "pane", keep)
        win.paneCount = 1
        // Everything is pane 0 again once there is only one pane.
        for (let j = 0; j < tabList.count; j++) tabList.setProperty(j, "pane", 0)
        const showing = win.paneTab[keep]
        win.paneTab = [showing >= 0 ? showing : 0, -1]
        win.activePane = 0
        win.activeTab = win.paneTab[0]
        win.activateTab(win.activeTab)
        win.refreshPaneTabCount()
        win.qaDump("pane-close")
    }

    /// The last path segment, for naming a destination in a sentence.
    function lastSegment(p) {
        if (!p || p === "") return "?"
        const cut = p.lastIndexOf("/")
        return (cut >= 0 && cut < p.length - 1) ? p.substring(cut + 1) : p
    }

    function firstTabOf(p) {
        const t = win.paneTabs(p)
        return t.length > 0 ? t[0] : -1
    }

    /// Copy or move the selection to the other pane's folder. Both go
    /// through the same engine, journal and undo as any other operation
    /// (DESIGN §6.7 rule 4); with one pane there is no other side, and
    /// refusing with a reason beats doing nothing.
    function sendToOtherPane(asMove) {
        if (win.paneCount < 2) {
            win.alertText = qsTr("Only one pane is open — split first (:split)")
            win.qaDump("pane-send-refused")
            return
        }
        const other = win.activePane === 0 ? 1 : 0
        const dest = win.paneModel(other)
        const src = win.dirModel
        if (!dest || !src) return
        if (dest.location === src.location) {
            win.alertText = qsTr("Both panes are showing the same folder")
            win.qaDump("pane-send-refused")
            return
        }
        const err = asMove ? src.move_selection_to(dest.location)
                           : src.copy_selection_to(dest.location)
        if (err !== "") {
            win.alertText = err
            win.qaDump("pane-send-refused")
            return
        }
        // A job nobody polls never finishes, and then every later operation
        // is refused because one is "still running".
        hints.status = (asMove ? qsTr("Moving to ") : qsTr("Copying to "))
                     + win.lastSegment(dest.location)
        jobTimer.start()
        win.qaDump("pane-send")
    }

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
        win.refreshPaneTabCount()
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
    /// How many tabs the ACTIVE pane has — what the strip reflects.
    property int activePaneTabCount: 1

    // Each pane's model and view as PLAIN PROPERTIES. They were bindings
    // over paneModel()/paneViewMode(), which are function calls: QML cannot
    // track those, so they evaluated once against an empty Instantiator and
    // the panes rendered nothing at all (DESIGN §6.6.1).
    property var paneAModel: null
    property var paneBModel: null
    property string paneAView: "rows"
    property string paneBView: "rows"

    function refreshPanes() {
        const a = win.paneModel(0)
        if (a) win.paneAModel = a
        win.paneBModel = win.paneCount > 1 ? win.paneModel(1) : null
        win.paneAView = win.paneViewMode(0)
        win.paneBView = win.paneViewMode(1)
    }
    function refreshPaneTabCount() {
        win.activePaneTabCount = win.paneTabs(win.activePane).length
        win.refreshPanes()
    }

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
        tabList.insert(at, { startPath: here, label: "…",
                             pane: win.activePane })
        tabViews.splice(at, 0, "rows")
        activateTab(at)
        win.refreshPaneTabCount()
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
        const owner = tabList.get(i).pane
        tabList.remove(i)
        tabViews.splice(i, 1)
        // Every stored index above the removed one shifts down by one.
        const pt = win.paneTab.map(function (t) {
            return t > i ? t - 1 : (t === i ? -1 : t)
        })
        win.paneTab = pt
        if (activeTab >= tabList.count) activeTab = tabList.count - 1
        else if (i < activeTab) activeTab -= 1
        // A pane that just lost its last tab closes: an empty half of the
        // window shows nothing and does nothing.
        if (win.paneCount > 1 && win.paneTabs(owner).length === 0) {
            win.closePane(owner)
            return
        }
        if (win.paneTab[win.activePane] < 0) {
            const left = win.paneTabs(win.activePane)
            if (left.length > 0) activeTab = left[0]
        }
        activateTab(activeTab)
        win.refreshPaneTabCount()
        win.qaDump("tab-close")
    }

    property int pendingCloseTab: -1
    property string pendingCloudOff: ""

    function activateTab(i) {
        if (i < 0 || i >= tabList.count) return
        activeTab = i
        // The pane showing this tab is the one the user is now in.
        const owner = tabList.get(i).pane
        if (owner !== win.activePane && owner < win.paneCount)
            win.activePane = owner
        const pt = win.paneTab.slice()
        pt[win.activePane] = i
        win.paneTab = pt
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
        // Cycling stays inside the active pane: Ctrl+Tab should not throw the
        // user into the other half of the window.
        const mine = win.paneTabs(win.activePane)
        if (mine.length <= 1) return
        const at = mine.indexOf(win.activeTab)
        let k = (at + step) % mine.length
        if (k < 0) k += mine.length
        activateTab(mine[k])
    }

    // --- helpers -----------------------------------------------------------
    function activePaneItem() {
        if (win.activePane === 1 && paneBLoader.item) return paneBLoader.item
        return paneA
    }
    function currentView() { return win.activePaneItem().currentView() }
    function setView(m) {
        viewMode = m
        // Each tab keeps its own view, so switching tabs restores what that
        // tab was showing rather than what the last one chose.
        if (win.activeTab >= 0) win.tabViews[win.activeTab] = m
        win.refreshPanes()
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
        // `prompt.claimingFocus` as well as `prompt.visible`: the prompt's
        // visibility is an animated opacity, so for the first frames after
        // it opens it does not count as visible. The palette closing in that
        // window used to steal the keyboard, leaving a prompt on screen that
        // could not be typed into.
        if (palette.visible || confirm.visible
            || prompt.visible || prompt.claimingFocus)
            return
        listFocus.forceActiveFocus()
    }

    /// Open Quick Look for the cursor row, or close it if it is already up.
    /// Never launches the file: peek_row reads bytes and metadata only.
    function toggleQuickLook() {
        if (quickLook.open) {
            quickLook.hide()
            win.focusList()
            win.qaDump("quicklook")
            return
        }
        if (!dirModel || dirModel.count === 0) return
        quickLook.show(dirModel.peek_row(dirModel.cursor))
        win.qaDump("quicklook")
    }

    function activateRow(row) {
        // Several files selected: one viewer showing all of them beats five
        // viewers showing one each (DESIGN §6.18). Returns "" when this is
        // not a multi-file situation, so ordinary activation still runs.
        if (dirModel.sel_count() > 1) {
            const many = dirModel.activate_selection()
            if (many.startsWith("OK:")) {
                hints.status = many.substring(3)
                win.qaDump("activate-many")
                return
            }
            if (many !== "") {
                win.alertText = many
                win.qaDump("activate-refused")
                return
            }
        }
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
            height: win.activePaneTabCount > 1 ? Tokens.controlHeight : 0
            color: Tokens.barBackground
            visible: win.activePaneTabCount > 1

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
                        // Only this pane's tabs. A strip mixing both panes
                        // makes a click jump the user across the window.
                        required property int pane
                        visible: pane === win.activePane
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
                devices: win.deviceList
                onDeviceChosen: (path, mounted) => win.openDevice(path, mounted)
            }

            Item {
                id: paneArea
                x: sidebar.visible ? sidebar.width : 0
                width: parent.width - x
                height: parent.height

                // Two panes side by side, or one filling the area. With a
                // single pane there is no splitter and no pane border:
                // chrome that earns nothing is not shown (DESIGN §6.7).
                Pane {
                    id: paneA
                    x: 0
                    y: 0
                    width: win.paneCount > 1
                         ? Math.round((paneArea.width - paneSplit.width) * win.paneRatio)
                         : paneArea.width
                    height: paneArea.height
                    model: win.paneAModel
                    viewMode: win.paneAView
                    dropAsMove: win.dropAsMove
                    onActivated: (r) => { win.focusPane(0); win.activateRow(r) }
                    onSelectionTouched: { win.focusPane(0); win.qaDump("select") }
                    onDragBegan: { win.focusPane(0); win.qaDump("drag-begin") }
                    onDropPreview: (t) => { hints.status = t }
                    onDropFailed: (m) => { win.alertText = m; win.qaDump("drop") }
                    onDropDone: { hints.status = ""; jobTimer.start(); win.qaDump("drop") }
                }

                // The divider. It is a visible seam, not a decoration: the
                // user needs to see where one pane ends.
                Rectangle {
                    id: paneSplit
                    visible: win.paneCount > 1
                    width: visible ? Math.max(1, Tokens.borderWidth) : 0
                    height: paneArea.height
                    x: paneA.width
                    color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                }

                // A Loader, not a hidden Pane: an unopened pane holds a null
                // model, and every binding inside its three views throws on
                // the first frame. A pane that is not open does not exist.
                Loader {
                    id: paneBLoader
                    active: win.paneCount > 1
                    x: paneA.width + paneSplit.width
                    y: 0
                    width: active ? paneArea.width - x : 0
                    height: paneArea.height
                    sourceComponent: Pane {
                        model: win.paneBModel
                        viewMode: win.paneBView
                        dropAsMove: win.dropAsMove
                        onActivated: (r) => { win.focusPane(1); win.activateRow(r) }
                        onSelectionTouched: { win.focusPane(1); win.qaDump("select") }
                        onDragBegan: { win.focusPane(1); win.qaDump("drag-begin") }
                        onDropPreview: (t) => { hints.status = t }
                        onDropFailed: (m) => { win.alertText = m; win.qaDump("drop") }
                        onDropDone: { hints.status = ""; jobTimer.start(); win.qaDump("drop") }
                    }
                }

                // The active pane is marked by a border, not by colour alone:
                // a theme may make the two backgrounds nearly identical.
                // z: above the panes, or their own backgrounds cover it.
                Rectangle {
                    z: 10
                    visible: win.paneCount > 1
                    x: win.activePane === 0 ? paneA.x : paneBLoader.x
                    y: 0
                    width: win.activePane === 0 ? paneA.width : paneBLoader.width
                    height: paneArea.height
                    color: "transparent"
                    border.width: Math.max(1, Tokens.borderWidth)
                    border.color: Tokens.accent
                }
            }
        }

        // Filter bar, shown only while filtering.        // Filter bar, shown only while filtering.
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
            const step = win.activePaneItem().pageStep()
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
                // Space is Quick Look (the plan's mockups say so in three
                // places); selection toggling moved to Ctrl+Space.
                //
                // Only UNMODIFIED Space: this switch runs before the Ctrl
                // block below, so without the guard Ctrl+Space opened Quick
                // Look and keyboard multi-select silently stopped working.
                if (event.modifiers & Qt.ControlModifier) break
                win.toggleQuickLook()
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
            case Qt.Key_Tab:
                if (win.paneCount > 1) {
                    win.focusPane(win.activePane === 0 ? 1 : 0)
                    event.accepted = true; return
                }
                break
            case Qt.Key_F5:
                win.sendToOtherPane(false); event.accepted = true; return
            case Qt.Key_F6:
                win.sendToOtherPane(true); event.accepted = true; return
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
                //
                // Quick Look comes first: it is the most recently opened
                // thing, so Escape must close it before touching a job or
                // the selection (DESIGN §6.9 rule 4).
                if (quickLook.open) {
                    quickLook.hide()
                    win.focusList()
                    win.qaDump("quicklook")
                    event.accepted = true; return
                }
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
                    : "arrows move, Enter open, Space quick look, Backspace up, / filter, : command"
                event.accepted = true; return
            }

            if (event.modifiers & Qt.ControlModifier) {
                if (event.key === Qt.Key_Space) {
                    // Multi-select by keyboard: Space itself is now Quick
                    // Look, so the toggle moved here rather than vanishing.
                    dirModel.sel_toggle(dirModel.cursor)
                    win.qaDump("select")
                    event.accepted = true; return
                }
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
                case Qt.Key_R:
                    // Shift+R is bulk rename over the selection (mockup C);
                    // plain r renames the one under the cursor.
                    if (event.modifiers & Qt.ShiftModifier) win.askBulkRename()
                    else win.askRename()
                    event.accepted = true; return
                case Qt.Key_O:
                    // "o open with…" (mockup B). The palette shows the list,
                    // because which applications exist depends on the file.
                    palette.openWith("open with ")
                    event.accepted = true; return
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
            // The share steps chain: each one opens the next rather than
            // finishing, so they return early.
            if (prompt.mode === "bulk") {
                win.bulkPreview(text)
                return
            }
            if (prompt.mode === "share-address") {
                win.shareAddressEntered(text)
                return
            }
            if (prompt.mode === "share-user") {
                win.shareUserEntered(text)
                return
            }
            if (prompt.mode === "share-password") {
                prompt.close()
                win.sharePasswordEntered(text)
                return
            }

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
        // Also dump when the RESULTS change, not only when the palette
        // opens and closes: a test that reads the state file after typing
        // was seeing the snapshot from before anything was typed.
        onResultsChanged: win.qaDump("palette-results")
        onClosed: win.focusList()
        onNavigateRequested: (p) => { dirModel.navigate(p); win.focusList() }
        onActionRequested: (a) => {
            if (a === "view-rows") win.setView("rows")
            else if (a === "split") win.splitPane()
            else if (a === "unsplit") win.closePane(win.activePane)
            else if (a === "send-copy") win.sendToOtherPane(false)
            else if (a === "send-move") win.sendToOtherPane(true)
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
            else if (a.startsWith("openwith:")) {
                const msg = dirModel.open_with_id(dirModel.cursor, a.substring(9))
                // "OK:" marks success: the window does not change, so
                // without a line here nothing appears to have happened.
                if (msg.startsWith("OK:")) hints.status = msg.substring(3)
                else if (msg !== "") win.alertText = msg
                win.qaDump("open-with")
            }
            else if (a === "bulk") {
                win.askBulkRename()
            }
            else if (a === "compress") {
                const cerr = dirModel.compress_selection(dirModel.cursor)
                if (cerr !== "") win.alertText = cerr
                win.qaDump("compress")
            }
            else if (a === "extract") {
                // Only offer to extract something that looks like an archive:
                // "Extract here" on a text file would be a button that can
                // only fail.
                if (!dirModel.row_is_archive(dirModel.cursor)) {
                    win.alertText = qsTr("That is not an archive Gust can open.")
                    win.qaDump("extract-refused")
                } else {
                    const xerr = dirModel.extract_row(dirModel.cursor)
                    if (xerr !== "") win.alertText = xerr
                    win.qaDump("extract")
                }
            }
            else if (a === "share-add") {
                win.askShareAddress()
            }
            else if (a.startsWith("device-mount:")) {
                win.openDevice(a.substring(13), false)
            }
            else if (a.startsWith("device-eject:")) {
                win.ejectDevice(a.substring(13))
            }
            else if (a.startsWith("grep:")) {
                dirModel.grep_start(a.substring(5))
                win.qaDump("grep")
            }
            else if (a.startsWith("cloud-on:")) {
                const cname = a.substring(9)
                hints.status = qsTr("Connecting to ") + cname + "…"
                const cerr = dirModel.cloud_connect(cname)
                if (cerr !== "") win.alertText = cerr
                win.refreshPlaces()
                win.qaDump("cloud-connect")
            }
            else if (a.startsWith("cloud-off:")) {
                const cname2 = a.substring(10)
                // Never force on the first try: unfinished uploads must be
                // reported, not silently discarded (DESIGN §6.8).
                const cerr2 = dirModel.cloud_disconnect(cname2, false)
                // "ASK:" means a decision is available; anything else is a
                // failure that confirming would only repeat.
                if (cerr2 !== "" && !cerr2.startsWith("ASK:")) {
                    win.alertText = cerr2
                    win.qaDump("cloud-disconnect-failed")
                } else if (cerr2 !== "") {
                    win.pendingCloudOff = cname2
                    confirm.purpose = "clouddirty"
                    confirm.open(qsTr("Uploads are not finished"),
                                 cerr2.substring(4),
                                 qsTr("Disconnect anyway"), true)
                } else {
                    win.refreshPlaces()
                }
                win.qaDump("cloud-disconnect")
            }
            else if (a === "cloud-setup") {
                // rclone's own config tool, in a terminal. Gust never asks
                // for a cloud password and never sees one.
                const serr = dirModel.cloud_setup()
                if (serr !== "") win.alertText = serr
                else hints.status = qsTr("Set up your storage in the terminal, "
                                         + "then run :cloud again")
                win.qaDump("cloud-setup")
            }
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

    QuickLook {
        id: quickLook
        onClosed: win.focusList()
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
            if (confirm.purpose === "bulk") {
                win.bulkApply()
                return
            }
            if (confirm.purpose === "clouddirty") {
                // The user was told exactly how many files are unfinished
                // and chose to go anyway. Only now is force allowed.
                const cn = win.pendingCloudOff
                win.pendingCloudOff = ""
                if (cn === "") return
                const e = dirModel.cloud_disconnect(cn, true)
                if (e !== "") win.alertText = e
                win.refreshPlaces()
                win.qaDump("cloud-disconnect-forced")
                return
            }
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
            // Cancel means CANCEL. This handler used to carry a copy of the
            // accept branches, so cancelling "disconnect anyway?" force-
            // disconnected and cancelling "close this tab?" closed it —
            // exactly the opposite of what the button says.
            if (confirm.purpose === "clouddirty") {
                win.pendingCloudOff = ""
                win.qaDump("cloud-disconnect-cancelled")
                return
            }
            if (confirm.purpose === "bulk") {
                win.bulkPattern = ""
                win.qaDump("bulk-cancelled")
                return
            }
            if (confirm.purpose === "closetab") {
                // Keep the tab AND its transfer: that is what Cancel means.
                win.pendingCloseTab = -1
                hints.status = qsTr("Transfer continues")
                win.qaDump("tab-close-cancelled")
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
            paneCount: win.paneCount,
            activePane: win.activePane,
            paneLocations: (function () {
                let out = []
                for (let p = 0; p < win.paneCount; p++) {
                    const m = win.paneModel(p)
                    out.push(m ? m.location : "")
                }
                return out
            })(),
            paneTabs: (function () {
                let out = []
                for (let p = 0; p < win.paneCount; p++)
                    out.push(win.paneTabs(p).length)
                return out
            })(),
            quickLookOpen: quickLook.open,
            quickLookName: quickLook.info ? quickLook.info.name : "",
            quickLookKind: quickLook.info ? quickLook.info.kindWords : "",
            quickLookLines: quickLook.info && quickLook.info.lines
                            ? quickLook.info.lines.length : 0,
            quickLookNoPreview: quickLook.info ? quickLook.info.noPreview : "",
            paletteOpen: palette.visible,
            paletteResults: palette.visible ? palette.resultLabels() : [],
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
                win.refreshPaneTabCount()
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
        // A local read of the kernel's own view: no daemon, no network.
        win.refreshDevices()
        qaStatePath = qaWriter.env_path("GUST_QA_STATE")
        console.log("GUST_READY location=" + dirModel.location
                    + " count=" + dirModel.count
                    + " theme=" + Tokens.themeName)
        win.focusList()
        qaDump("ready")
    }
}
