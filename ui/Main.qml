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
            id: tabModel
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
            onLocationChanged: {
                if (clipboard_busy()) {
                    cancel_clip_paste()
                    hints.message = qsTr("Paste cancelled")
                }
                sessionSaveTimer.restart()
            }
            // Mounts and connections finish on a worker; every tab reports
            // here, so a result still lands if the user switched tabs.
            onPlace_done: (op, name, message) => win.placeDone(op, name, message, tabModel)
            onPaste_ready: (message, allowImage) => win.pasteFinished(tabModel, message, allowImage)
            onPermissions_done: (token, error, message, partial) => {
                if (token !== 0) return
                if (error !== "") win.alertText = error
                else hints.message = message
                win.qaDump("program-permissions-done")
            }
            onDevices_changed: {
                win.refreshDevices()
                win.qaDump("devices-changed")
            }
            Component.onCompleted: hold_live_updates(win.liveHold)
        }
        onObjectAdded: win.refreshActiveModel()
        onObjectRemoved: (index, object) => {
            win.forgetConnectionOwner(object)
            win.forgetConflictOwner(object)
            win.refreshActiveModel()
        }
    }


    // Snapshot of the running job, or null. Read by the drawer and QA.
    property var job: null

    // Set once the user has agreed to stop the transfers and close. The
    // window then waits for them to actually stop; "stop requested" is not
    // "stopped" while a slow drive or remote call has not returned.
    property bool closing: false
    // Set only when the user chose to close without waiting for that stop.
    property bool closeNow: false

    // A window close must not silently abandon pending transfers (plan §2).
    //
    // This handler covers a close the toolkit tells us about. A graceful
    // compositor close (an xdg_toplevel close, e.g. Hyprland 0.56's
    // `hl.dsp.window.close` on Qt 6.12) does reach it; an older pairing
    // (Qt 6.11) was seen not to. A forced kill never does. So it is not
    // the only protection: a copy stages under a private name and is
    // renamed atomically, so a killed transfer never publishes a partial
    // file. Its temporary files can remain, and a remote place may keep
    // them on purpose.
    /// Every tab whose transfer is still running — not just the one on
    /// screen. A copy started in another tab is just as much the user's.
    function runningJobModels() {
        const out = []
        for (let i = 0; i < tabList.count; i++) {
            const m = tabs.objectAt(i)
            if (m && (m.job_running() || m.sharing_busy)) out.push(m)
        }
        return out
    }

    /// Every tab still holding files open: a transfer (running or still
    /// stopping), archive work or a permissions change. The last two cannot
    /// be stopped part-way; a permissions change also still owes its undo.
    function busyModels() {
        const out = []
        for (let i = 0; i < tabList.count; i++) {
            const m = tabs.objectAt(i)
            if (m && (m.job_running() || m.archive_running() || m.permissions_running() || m.sharing_busy))
                out.push(m)
        }
        return out
    }

    onClosing: (close) => {
        if (win.closeNow)
            return
        const busy = win.busyModels()
        if (busy.length === 0)
            return
        close.accepted = false
        if (win.closing) {
            // Stop was already asked for and something has not finished:
            // usually a remote call that has not come back yet. Closing
            // anyway is the user's call, so ask rather than wait silently
            // or quit behind their back.
            confirm.purpose = "closewait"
            confirm.open(qsTr("Still stopping"),
                qsTr("Waiting for the drive. Closing now may leave temporary files. Check the destination.")
                    + (busy.some(m => m.permissions_running())
                       ? qsTr(" A permissions change may be left part-way, and Ctrl+Z cannot put it back.") : ""),
                qsTr("Close anyway"),
                true)
            win.qaDump("close-still-stopping")
            return
        }
        const running = busy.filter(m => m.job_running() || m.sharing_busy)
        const archiving = busy.some(m => m.archive_running())
        const permissions = busy.some(m => m.permissions_running())
        confirm.purpose = "close"
        if (busy.some(m => m.sharing_busy)) {
            confirm.open(qsTr("File sharing is still running"),
                qsTr("Closing asks it to stop. Changes may already have been accepted."),
                qsTr("Stop and close"), true)
            win.qaDump("close-blocked")
            return
        }
        if (running.length === 0) {
            // Neither can be stopped part-way: say so, and only wait.
            if (archiving && permissions)
                confirm.open(qsTr("Archive work and a permissions change are still running"),
                    qsTr("They can't stop part-way. Gust closes when they're done."),
                    qsTr("Close when finished"),
                    false)
            else if (permissions)
                confirm.open(qsTr("Permissions are still being changed"),
                    qsTr("Gust cannot stop it part-way. It closes when finished."),
                    qsTr("Close when finished"),
                    false)
            else
                confirm.open(qsTr("Archive work is still running"),
                    qsTr("Gust cannot stop it part-way. It closes when finished."),
                    qsTr("Close when finished"),
                    false)
            win.qaDump("close-blocked")
            return
        }
        const detail = running.length > 1
            ? qsTr("%1 in different tabs. ").arg(running.length)
            : (win.job && win.job.message && running[0] === dirModel) ? win.job.message + ". "
            : qsTr("In another tab. ")
        const archive = (archiving
            ? qsTr(" Archive work finishes first.") : "")
            + (permissions
            ? qsTr(" A permissions change cannot be stopped; Gust waits for it.") : "")
        confirm.open(
            running.length > 1 ? qsTr("Transfers are still running") : qsTr("A transfer is still running"),
            detail + qsTr("Closing asks it to stop; the current item may leave temporary files.") + archive,
            qsTr("Stop and close"),
            true)
        win.qaDump("close-blocked")
    }

    // Close only once every worker has actually stopped. A
    // stop can wait on a remote call for as long as the connection takes to
    // answer, so there is no deadline here: quitting at one would drop the
    // work while it still holds files open. The user can still choose to
    // close without waiting (onClosing above).
    Timer {
        id: closeAfterCancel
        interval: 150
        repeat: true
        property int ticks: 0
        onTriggered: {
            ticks += 1
            if (win.busyModels().length === 0) {
                stop()
                win.closeNow = true
                Qt.quit()
                return
            }
            if (ticks === 8)
                hints.message = qsTr("Still stopping…")
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

    // Pinned folders from ~/.config/gtk-3.0/bookmarks (shared with the
    // desktop). Re-read when Gust changes them; another app's edit shows on
    // the next start or pin.
    property var bookmarkList: []
    property string bookmarkMenuPath: ""
    function refreshBookmarks() {
        try { win.bookmarkList = JSON.parse(dirModel.bookmarks_json()) }
        catch (e) { win.bookmarkList = [] }
    }

    // --- settings --------------------------------------------------------
    /// Apply saved settings to this window. Called at start and after every
    /// change; the renderer choice alone waits for the next start.
    function applySettings() {
        let s = {}
        try { s = JSON.parse(dirModel.settings_json()) } catch (e) { return }
        win.keyPreset = ["vim", "mac", "windows"].includes(s.key_preset) ? s.key_preset : "standard"
        win.sidebarVisible = s.sidebar !== false
        Tokens.density = s.spacing > 0 ? s.spacing : 1.0
        win.defaultShowHidden = s.show_hidden === true
    }
    property bool defaultShowHidden: false
    function openSettings() {
        settingsPanel.show(dirModel.settings_json())
        win.qaDump("settings")
    }
    // qaDump for close lives in the panel's onClosed as well.

    // The tab being handed to a new window, while it starts.
    property var detachModel: null
    Timer {
        id: detachTimer
        interval: 100
        repeat: true
        onTriggered: {
            const m = win.detachModel
            if (!m) { stop(); return }
            const r = m.poll_detach()
            if (r === "wait") return
            stop()
            win.detachModel = null
            if (r === "ready") {
                // Find the tab again: others may have moved meanwhile.
                for (let i = 0; i < tabList.count; i++) {
                    if (tabs.objectAt(i) === m) { win.closeTab(i); break }
                }
                hints.message = qsTr("Opened in a new window")
                win.qaDump("tab-detached")
            } else {
                win.alertText = r
                win.qaDump("tab-detach-failed")
            }
        }
    }

    /// Visible tab centres in window coordinates, for the mouse QA harness.
    function tabCenters() {
        let out = []
        for (let i = 0; i < tabRepeater.count; i++) {
            const t = tabRepeater.itemAt(i)
            if (!t || !t.visible || !tabStrip.visible) continue
            const p = t.mapToItem(null, t.width / 2, t.height / 2)
            out.push([Math.round(p.x), Math.round(p.y)])
        }
        return out
    }

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
        const supportKey = text.trim().toLowerCase().startsWith("nfs://") ? "nfs" : "smb"
        if (!win.requireSupport(supportKey)) { prompt.close(); return }
        win.shareAddress = text
        // NFS authorises by address, not by account. Asking for a username
        // and password it will never send would be theatre.
        if (text.trim().toLowerCase().startsWith("nfs://")) {
            prompt.close()
            win.shareUser = ""
            win.connectShare(text, "", "")
            win.shareAddress = ""
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
        win.connectShare(win.shareAddress, win.shareUser, text)
        // Whatever happened, the secret does not stay in QML.
        prompt.secret = false
        win.shareAddress = ""
        win.shareUser = ""
    }
    function connectShare(address, user, password) {
        const supportKey = address.trim().toLowerCase().startsWith("nfs://") ? "nfs" : "smb"
        if (!win.requireSupport(supportKey)) return
        if (!win.beginPlaceFeedback("share", "", qsTr("Connecting to shared folder…"))) return
        connectPanel.supportKey = supportKey
        const model = win.connectionOwner
        const error = model.share_mount(address, user, "", password)
        if (error !== "") { win.alertText = error; win.finishPlaceFeedback("share", "", error, model) }
    }

    /// A mount, connect, eject or disconnect finished on its worker.
    /// "" is success; "ASK:" is a decision only the user can make.
    function placeDone(op, name, message, owner) {
        if (op === "paste-image") {
            if (message !== "") win.alertText = message
            else { win.alertText = ""; hints.message = name }
            win.qaDump("paste-image" + (message === "" ? "" : "-failed"))
            return
        }
        if (!message.startsWith("ASKKEY:") && !message.startsWith("ASK:"))
            win.finishPlaceFeedback(op, name, message, owner)
        if (op === "tailnet-connect" && message.startsWith("ASKKEY:")) {
            // First connection to this device: show its SSH key and let the
            // user decide. Nothing is trusted on their behalf.
            let k = {}
            try { k = JSON.parse(message.substring(7)) } catch (e) { k = {} }
            const model = owner || win.connectionOwner
            win.finishPlaceFeedback(op, name, qsTr("Check the fingerprints."), model)
            connectPanel.feedbackTitle = qsTr("Verify this computer")
            connectPanel.feedbackFailed = false
            win.pendingTailnet = { name: k.name, target: k.target, model: model }
            win.pendingTailnetUser = k.user || ""
            confirm.purpose = "tailnetkey"
            confirm.open(qsTr("First connection to %1").arg(k.name),
                         qsTr("New SSH key. Trust it only if you expect these fingerprints.\n\n%1")
                             .arg((k.fingerprints || []).join("\n\n")),
                         qsTr("Trust and connect"), false)
            win.refreshPlaces()
            win.qaDump("place-tailnet-askkey")
            return
        }
        if (op === "device-unlock" && message.indexOf("passphrase did not unlock") >= 0) {
            // Wrong passphrase: ask again, saying why. Nothing changed.
            win.askUnlock(name)
            prompt.showError(message)
            dirModel.request_devices()
            win.qaDump("place-device-unlock-failed")
            return
        }
        if (op === "cloud-disconnect" && message.startsWith("ASK:")) {
            win.pendingCloudOff = name
            confirm.purpose = "clouddirty"
            confirm.open(qsTr("Uploads are not finished"),
                         message.substring(4),
                         qsTr("Disconnect anyway"), true)
        } else if (message !== "") {
            win.alertText = message
        }
        if (op.startsWith("device") || op.startsWith("phone") || op === "setup-install") dirModel.request_devices()
        win.refreshPlaces()
        win.qaDump("place-" + op + (message === "" ? "" : "-failed"))
    }

    // --- dragging a tab (DESIGN §6.20) -----------------------------------
    //
    // Where a drag currently is: "" (nowhere), "left", "right" or "outside".
    // A drop target is shown only for the first two, because "outside" has
    // no rectangle to highlight.
    property string tabDropZone: ""
    property int tabDragIndex: -1
    /// The last position the window actually saw. Wayland stops delivering
    /// motion once the pointer leaves the surface, so a release outside is
    /// reported at the edge — the last seen position is what decides.
    property point tabDragLast: Qt.point(0, 0)
    /// True once the drag has been seen leaving the window's own bounds.
    property bool tabDragLeft: false

    /// Which zone is this window position in?
    ///
    /// Left and right thirds of the PANE AREA split; anywhere beyond the
    /// window's own edges detaches. The middle is deliberately nothing: a
    /// drag that ends there puts the tab back, because losing a tab to an
    /// imprecise gesture is how people stop dragging tabs.
    function tabZoneAt(x, y) {
        if (x < 0 || y < 0 || x > win.width || y > win.height) return "outside"
        // Only over the file area — a drop on the header or the strip itself
        // is not a split.
        if (y < paneArea.mapToItem(null, 0, 0).y) return ""
        const left = paneArea.mapToItem(null, 0, 0).x
        const w = paneArea.width
        if (w <= 0) return ""
        const rel = (x - left) / w
        if (rel < 0 || rel > 1) return ""
        if (rel < 0.33) return "left"
        if (rel > 0.67) return "right"
        return ""
    }

    function tabDragMoved(index, x, y) {
        win.tabDragIndex = index
        win.tabDragLast = Qt.point(x, y)
        const zone = win.tabZoneAt(x, y)
        // Whether the drag is outside RIGHT NOW, not whether it ever was.
        // Latching on any excursion meant dragging out and back still
        // detached, which loses a tab to a gesture the user reversed.
        win.tabDragLeft = (zone === "outside")
        if (zone !== win.tabDropZone) {
            win.tabDropZone = zone
            hints.message = zone === "outside"
                ? qsTr("Release to open in a new window")
                : zone === "" ? ""
                : qsTr("Release to put this tab on the ") + zone
            win.qaDump("tab-drag")
        }
    }

    function tabDragEnded() {
        win.tabDropZone = ""
        win.tabDragIndex = -1
        win.tabDragLeft = false
        hints.message = ""
    }

    function tabDragDropped(index, x, y) {
        // A release outside the window arrives clamped to the edge, so the
        // drag having LEFT at any point is what decides — not where the
        // release happened to be reported.
        const zone = win.tabDragLeft ? "outside" : win.tabZoneAt(x, y)
        win.tabDragEnded()

        if (zone === "outside") {
            // The last tab detaching would close the window it came from,
            // which is a window-close gesture wearing a tab-drag costume.
            if (win.tabCount <= 1) {
                hints.message = qsTr("This is the only tab")
                win.qaDump("tab-detach-refused")
                return
            }
            const m = win.tabModelAt(index)
            if (!m) return
            const err = m.detach_here(win.tabViews[index] || win.viewMode)
            if (err !== "") {
                win.alertText = err
                win.qaDump("tab-detach-refused")
                return
            }
            // The tab closes only once the new window says it is on screen:
            // a window that fails to start must not take the tab with it.
            hints.message = qsTr("Opening in a new window…")
            win.detachModel = m
            detachTimer.start()
            win.qaDump("tab-detach-waiting")
            return
        }

        if (zone === "left" || zone === "right") {
            const wantPane = zone === "left" ? 0 : 1
            // Opening the second pane, but NOT through splitPane(): that
            // moves the ACTIVE tab across, which is right for :split and
            // wrong here — the drop already knows which tab to move, and
            // calling it first sent two tabs right and left pane 0 empty.
            if (win.paneCount < 2) {
                if (win.tabCount <= 1) {
                    hints.message = qsTr("Open another tab first")
                    win.qaDump("tab-split-refused")
                    return
                }
                win.paneCount = 2
                // The dragged tab goes to the new side; everything else
                // stays where it is.
                win.paneTab = [win.paneTab[0], index]
            }
            if (tabList.get(index).pane === wantPane) {
                // Already where it was dropped. Nothing happened, so nothing
                // is said.
                return
            }
            const cameFrom = tabList.get(index).pane
            tabList.setProperty(index, "pane", wantPane)
            // The pane it left must still show one of its OWN tabs, or it
            // points at a tab that now belongs to the other side.
            const left = win.paneTabs(cameFrom)
            const pt = win.paneTab.slice()
            pt[cameFrom] = left.length > 0 ? left[0] : -1
            pt[wantPane] = index
            win.paneTab = pt
            win.activePane = wantPane
            win.activateTab(index)
            win.refreshPaneTabCount()
            win.refreshPanes()
            hints.message = qsTr("Moved to the ") + zone + qsTr(" pane")
            win.qaDump("tab-split-drop")
        }
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
        const names = dirModel.bulk_names(dirModel.cursor)
        if (names !== "") {
            renameCard.openWith(dirModel, dirModel.cursor, names)
            win.qaDump("rename-card")
            return
        }
        // A name with a line break cannot be one line of the card: fall
        // back to the pattern prompt, which never shows names as lines.
        askBulkPattern()
    }

    /// Rename by pattern ({n}, {name}, {ext}): the palette's "Bulk rename
    /// by pattern", and the fallback for names the card cannot show.
    function askBulkPattern() {
        if (!dirModel || dirModel.count === 0) return
        prompt.secret = false
        prompt.mode = "bulk"
        prompt.openWith(qsTr("Rename these files"), "{name}.{ext}",
                        qsTr("{n} number · {name} name · {ext} extension"))
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
    }

    /// The drive a sidebar menu was opened on.
    property string deviceMenuPath: ""
    /// The phone a sidebar menu was opened on.
    property string phoneMenuId: ""
    /// A phone row was clicked: browse it if it is open; while it waits for
    /// Trust, a second click stops waiting; otherwise open it (gio asks the
    /// phone; Gust answers the Trust question until the person taps it).
    function openPhone(id, stateKey, apps, fromMenu) {
        const d = win.deviceList.find((x) => x.name === id)
        const requirement = d && (win.placesInfo.setup || []).find(r => r.key === d.phone)
        if (d && (requirement ? requirement.packages.length > 0 : stateKey === "needs-package")) {
            connectPanel.showSupport(d.phone)
            return
        }
        if (!apps && !fromMenu && stateKey === "open" && d && d.mountPoint) { dirModel.navigate(d.mountPoint); return }
        if (stateKey === "trust" || stateKey === "unlock" || stateKey === "opening") {
            if (fromMenu) {
                connectPanel.showFeedback(qsTr("Waiting for your device"), qsTr("Keep it unlocked and tap Trust if asked."), false)
                return
            }
            dirModel.phone_cancel(id)
            hints.message = qsTr("Stopped waiting for the phone")
            return
        }
        if (!d) {
            connectPanel.showFeedback(qsTr("Device is no longer connected"), qsTr("Reconnect it, then choose it under Devices."), false)
            return
        }
        if (!win.beginPlaceFeedback("phone-open", id, qsTr("Opening %1…").arg(d.label))) return
        connectPanel.supportKey = d.phone
        connectPanel.feedbackBody = d.phone === "apple"
            ? qsTr("Unlock it and tap Trust if asked.")
            : qsTr("Unlock it and choose File transfer in its USB notification.")
        const model = win.connectionOwner
        const error = model.phone_open(id, apps === true)
        if (error !== "") win.finishPlaceFeedback("phone-open", id, error, model)
        win.qaDump("phone-open")
    }

    /// A device row was clicked: browse it if it is mounted, otherwise
    /// mount it. Gust never mounts anything on its own.
    function openDevice(path, mounted) {
        const dev = win.deviceList.find((x) => x.path === path)
        if (dev && dev.locked) { win.askUnlock(path); return }
        if (mounted) {
            const d = win.deviceList.find((x) => x.path === path)
            if (d && d.mountPoint) dirModel.navigate(d.mountPoint)
            else win.alertText = qsTr("Mounted, but its folder is unknown")
            return
        }
        const err = dirModel.device_mount(path)
        if (err !== "") win.alertText = err
    }

    /// The encrypted drive whose passphrase the prompt is asking for.
    property string unlockPath: ""
    /// Ask for an encrypted drive's passphrase (DESIGN §6.13.2).
    function askUnlock(path) {
        const d = win.deviceList.find((x) => x.path === path)
        win.unlockPath = path
        prompt.secret = true
        prompt.mode = "unlock"
        prompt.openWith(qsTr("Unlock %1").arg(d ? d.label : qsTr("drive")), "",
                        qsTr("Never stored"))
        win.qaDump("unlock-prompt")
    }
    function unlockEntered(text) {
        const path = win.unlockPath
        const err = dirModel.device_unlock(path, text)
        prompt.secret = false
        if (err !== "") win.alertText = err
    }

    /// Ask for a fresh list of every place (on a worker); the sidebar and
    /// the Connect panel follow from places_json when it arrives.
    function refreshPlaces() {
        if (dirModel) dirModel.request_places()
    }
    /// Every connectable place, parsed (DESIGN §6.4.1).
    property var placesInfo: ({})
    function placesArrived() {
        let info = {}
        try { info = JSON.parse(dirModel.places_json) } catch (e) { info = {} }
        win.placesInfo = info
        const on = []
        const lost = []
        for (const kind of ["tailnet", "ssh", "cloud"]) {
            for (const p of (info[kind] || [])) {
                if (!p.connected) continue
                on.push({ kind: kind, name: p.name, path: p.path, lost: p.lost })
                if (p.lost) lost.push(p.name)
            }
        }
        const sidebarPlaces = []
        for (const c of (info.computers || [])) {
            const mounted = c.routes.filter(r => r.connected)
            if (mounted.length === 0) continue
            const r = mounted.find(r => !r.lost) || mounted[0]
            sidebarPlaces.push({kind: r.kind, name: r.name, label: c.name, path: r.path, lost: r.lost})
            for (const other of mounted) {
                if (other === r || !other.lost) continue
                sidebarPlaces.push({kind: other.kind, name: other.name,
                    label: qsTr("%1 · %2").arg(c.name).arg(other.name), path: other.path, lost: true})
            }
        }
        for (const p of (info.cloud || []))
            if (p.connected) sidebarPlaces.push({kind: "cloud", name: p.name, path: p.path, lost: p.lost})
        win.connectedPlaces = sidebarPlaces
        win.lostPlaces = lost
        const lostKeys = on.filter(p => p.lost).map(p => p.kind + ":" + p.name)
        const recovery = connectPanel.recoveryCandidate(win.seenLostRoutes, dirModel.location)
        win.seenLostRoutes = lostKeys
        // Never replace a pending decision or interrupt a running file job.
        // The Connections picker remains available when a suggestion is deferred.
        if (recovery !== "" && !confirm.visible && !connectionRecovery.visible && !prompt.visible && !prompt.claimingFocus
            && !connectPanel.open && !palette.visible && !quickLook.open && !conflictDialog.visible
            && !contextMenu.open && !settingsPanel.open && !renameCard.open && !permissionsDialog.open && !trashPanel.open
            && !recentPanel.open && !filterInput.activeFocus && !searchInput.activeFocus
            && win.runningJobModels().length === 0) {
            connectionRecovery.routeKey = recovery
            connectionRecovery.open(qsTr("Connection lost"),
                qsTr("Another connection to this computer is known. Choose a connection to browse again. "
                     + "Its own SSH identity, account and folder will be checked; interrupted file operations are not retried."),
                qsTr("Choose connection"), false)
        }
        win.qaDump("places")
    }
    Connections {
        target: win.dirModel
        function onPlaces_jsonChanged() { win.placesArrived() }
    }

    /// Connect a place from the Connect panel. A Tailscale device first
    /// asks which login to use; the rest connect as configured.
    property var pendingTailnet: null
    property var connectionOwner: null
    function beginPlaceFeedback(op, name, title, owner) {
        if (connectPanel.busy) { connectPanel.showFeedback("", "", false); return false }
        const model = owner || dirModel
        if (!model) return false
        if (model.place_busy) {
            connectPanel.showFeedback(qsTr("Another connection is in progress"),
                qsTr("Wait for it to finish."), false)
            return false
        }
        if (!connectPanel.beginFeedback(op, name, title)) return false
        win.connectionOwner = model
        win.alertText = ""
        return true
    }
    function finishPlaceFeedback(op, name, message, owner) {
        if (!win.connectionOwner || (owner && owner !== win.connectionOwner)) return
        if (connectPanel.completeFeedback(op, name, message)) {
            win.connectionOwner = null
            if (op === "setup-install") {
                connectPanel.feedbackTitle = message === "" ? qsTr("Connection support is installed") : qsTr("Setup did not finish")
                if (message === "") connectPanel.feedbackBody = name === "apple"
                    ? qsTr("Reconnect your device, then choose it under Devices.")
                    : qsTr("Choose what to connect to.")
                win.refreshPlaces()
            }
        }
    }
    function installSupport(key) {
        if (connectPanel.busy) { connectPanel.showFeedback("", "", false); return }
        connectPanel.showSupport(key)
        if (!connectPanel.missingSupport || !connectPanel.requirement.installable) return
        if (!win.beginPlaceFeedback("setup-install", key, qsTr("Installing connection support…"))) return
        connectPanel.feedbackBody = qsTr("Approve the system prompt and keep Gust open.")
        const model = win.connectionOwner
        const error = model.install_support(key)
        if (error !== "") win.finishPlaceFeedback("setup-install", key, error, model)
    }
    function forgetConnectionOwner(model) {
        if (win.pendingTailnet && win.pendingTailnet.model === model) win.pendingTailnet = null
        if (win.connectionOwner !== model) return
        const pending = connectPanel.operation
        if (pending) connectPanel.completeFeedback(pending.op, pending.name,
            qsTr("Its tab was closed; the outcome is unknown."))
        win.connectionOwner = null
    }
    function requireSupport(key) {
        const requirement = (win.placesInfo.setup || []).find(r => r.key === key)
        if ((requirement && requirement.packages.length > 0) || (key === "ssh" && win.placesInfo.sshfs === false)) {
            connectPanel.showSupport(key)
            return false
        }
        return true
    }
    function connectPlace(kind, name, target, user) {
        if (kind === "mobile") { win.runAction("device-share:" + name); return }
        if (!win.requireSupport(kind === "cloud" ? "cloud" : "ssh")) return
        if (kind === "ssh" || kind === "cloud") {
            const op = kind === "ssh" ? "remote-connect" : "cloud-connect"
            if (!win.beginPlaceFeedback(op, name, qsTr("Connecting to %1…").arg(name))) return
            connectPanel.supportKey = kind === "cloud" ? "cloud" : "ssh"
            const model = win.connectionOwner
            const error = kind === "ssh" ? model.remote_connect(name) : model.cloud_connect(name)
            if (error !== "") { win.alertText = error; win.finishPlaceFeedback(op, name, error, model) }
            return
        }
        if (connectPanel.busy) { connectPanel.showFeedback("", "", false); return }
        win.pendingTailnet = { name: name, target: target, model: dirModel }
        prompt.mode = "tailnet-user"
        prompt.secret = false
        prompt.openWith(qsTr("Log in to %1 as").arg(name), user,
                        qsTr("Uses your SSH key"))
        win.qaDump("tailnet-user-prompt")
    }
    function tailnetUserEntered(text) {
        const t = win.pendingTailnet
        prompt.close()
        win.pendingTailnetUser = text
        win.startTailnetAttempt(t, text, false)
    }
    function startTailnetAttempt(t, user, trust) {
        win.pendingTailnet = null
        if (!t || !t.model) {
            connectPanel.showFeedback(qsTr("Connection not started"),
                qsTr("Choose the computer again."), false)
            return
        }
        if (!win.beginPlaceFeedback("tailnet-connect", t.name, qsTr("Connecting to %1…").arg(t.name), t.model)) return
        connectPanel.supportKey = "ssh"
        const error = t.model.tailnet_connect(t.name, t.target, user, trust)
        if (error !== "") { win.alertText = error; win.finishPlaceFeedback("tailnet-connect", t.name, error, t.model) }
    }
    property string pendingTailnetUser: ""
    /// Why `name` cannot be disconnected yet, or "" when nothing in any tab
    /// is still using it. A transfer that was asked to stop still holds the
    /// place until its worker has stopped: unmounting under a remote call
    /// that has not returned turns a clean stop into a failure.
    function placeWorkBlocker(kind, name) {
        // A stopping transfer is named first: it is the one the user may
        // already be waiting on.
        const rank = ["", "archive", "permissions", "transfer", "stopping"]
        let found = ""
        for (let i = 0; i < tabList.count; i++) {
            const m = tabs.objectAt(i)
            const w = m ? m.work_on_place(kind, name) : ""
            if (rank.indexOf(w) > rank.indexOf(found)) found = w
        }
        if (found === "stopping")
            return qsTr("A transfer on %1 is still stopping").arg(name)
        if (found === "transfer")
            return qsTr("A transfer on %1 is still running").arg(name)
        if (found === "permissions")
            return qsTr("Permissions on %1 are still being changed").arg(name)
        if (found === "archive")
            return qsTr("Archive work on %1 is still running").arg(name)
        return ""
    }
    /// Refuse a disconnect while work still uses the place, saying so where
    /// the user asked for it.
    function refuseBusyDisconnect(kind, name) {
        const why = win.placeWorkBlocker(kind, name)
        if (why === "") return false
        if (connectPanel.open)
            connectPanel.showFeedback(qsTr("Not disconnected yet"), why, false)
        win.alertText = why
        win.qaDump("disconnect-refused-busy")
        return true
    }
    function disconnectPlace(kind, name) {
        if (kind === "ssh") win.runAction("remote-off:" + name)
        else if (kind === "cloud") win.runAction("cloud-off:" + name)
        else {
            if (win.refuseBusyDisconnect("tailnet", name)) return
            const e = dirModel.tailnet_disconnect(name)
            if (e !== "") win.alertText = e
        }
    }
    property var remoteMenuPlace: null
    // Mounted places whose server no longer answers. Still mounted (so
    // Disconnect is what clears them) but never shown as "connected".
    property var lostPlaces: []
    property var seenLostRoutes: []
    Timer {
        interval: 10000
        repeat: true
        running: win.connectedPlaces.length > 0 && win.active && !connectPanel.open && !connectionRecovery.visible
        onTriggered: if (dirModel && !dirModel.place_busy) win.refreshPlaces()
    }
    width: 1180
    height: 720
    minimumWidth: 520
    minimumHeight: 320
    visible: true
    title: (win.adminMode ? qsTr("Administrator — ") : "") + dirModel.location_display + " — Gust"
    /// The root window started by "Open as administrator".
    readonly property bool adminMode: dirModel ? dirModel.is_admin() : false
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
            hints.message = qsTr("Already two panes")
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
            hints.message = qsTr("This is the only pane")
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
        if (err === "?conflict") { win.transferStarted(src, err, "pane-send"); return }
        if (err !== "") {
            win.alertText = err
            win.qaDump("pane-send-refused")
            return
        }
        // A job nobody polls never finishes, and then every later operation
        // is refused because one is "still running".
        hints.message = (asMove ? qsTr("Moving to ") : qsTr("Copying to "))
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
    // Each tab has its own model: a newly active one fetches the place list
    // so the sidebar never shows another tab's stale connections.
    onDirModelChanged: {
        if (dirModel && dirModel.places_json === "{}") win.refreshPlaces()
        hints.message = ""
    }

    // Back/Forward availability. can_go_back() is a call, which QML never
    // re-evaluates on its own: the arrows kept whatever look they had at
    // start-up (Back dim while it worked). Reading `location` makes the
    // binding re-run on every navigation.
    readonly property bool canBack: dirModel ? (dirModel.location, dirModel.can_go_back()) : false
    readonly property bool canForward: dirModel ? (dirModel.location, dirModel.can_go_forward()) : false

    // While an overlay refers to a row BY INDEX (the rename prompt, a
    // context menu, Quick Look, a confirm or conflict dialog), a file created or deleted
    // by another program must not shift the rows under it: the rename would
    // hit the wrong file. Live updates wait, and land when it closes.
    readonly property bool liveHold: palette.visible || confirm.visible || connectionRecovery.visible || settingsPanel.open || trashPanel.open || recentPanel.open || connectPanel.open || deviceSharing.open
                                     || contextMenu.open || conflictDialog.visible || quickLook.open
                                     || prompt.visible || prompt.claimingFocus
    onLiveHoldChanged: {
        for (let i = 0; i < tabList.count; i++) {
            const m = tabs.objectAt(i)
            if (m) m.hold_live_updates(win.liveHold)
        }
    }

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
            // A tab not opened yet (restored, or opened in the background)
            // keeps the name it was given until it lists its folder.
            const keep = m && !m.everActivated && tabList.get(i).startPath !== "" && i !== win.activeTab
            const name = keep ? tabList.get(i).label : (m ? m.tabLabel : "…")
            if (tabList.get(i).label !== name)
                tabList.setProperty(i, "label", name)
        }
    }
    onActiveTabChanged: {
        sessionSaveTimer.restart()
        refreshActiveModel()
        // The transfer card belongs to the tab on screen.
        win.job = null
        if (win.runningJobModels().length > 0) jobTimer.start()
    }
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
        // Restored/same-view tabs never pass through setView(). The pane
        // owners, including the inactive split, decide which models render.
        for (let i = 0; i < tabList.count; i++) {
            const m = tabs.objectAt(i)
            if (m) m.request_thumbnails((m === win.paneAModel && win.paneAView === "grid")
                                        || (m === win.paneBModel && win.paneBView === "grid"))
        }
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
        // Other panes still show their original tab after the shared list shifts.
        win.paneTab = win.paneTab.map(function (t) {
            return t >= at ? t + 1 : t
        })
        activateTab(at)
        win.refreshPaneTabCount()
        win.qaDump("tab-new")
    }

    function closeTab(i) {
        if (tabList.count <= 1) {
            // A file manager with no tab has nothing to show.
            hints.message = qsTr("This is the last tab")
            return
        }
        const m = tabModelAt(i)
        if (m && closeTabAfterCancel.models.indexOf(m) >= 0) {
            // Already asked and confirmed: the tab is waiting for its work to
            // stop, and a second request must not ask again or skip the wait.
            hints.message = qsTr("Still stopping…")
            win.qaDump("tab-close-waiting")
            return
        }
        if (m && (m.job_running() || m.archive_running() || m.permissions_running() || m.sharing_busy)) {
            // Work belongs to the tab that started it. Closing it asks
            // first, exactly as closing the window does.
            win.pendingCloseTab = i
            confirm.purpose = "closetab"
            if (m.sharing_busy)
                confirm.open(qsTr("File sharing is still running"),
                    qsTr("Closing asks it to stop. Changes may already have been accepted."),
                    qsTr("Stop and close"), true)
            else if (m.job_running())
                confirm.open(qsTr("A transfer is still running"),
                             qsTr("Closing asks it to stop; the current item may leave temporary files."),
                             qsTr("Stop and close"), true)
            else if (m.permissions_running() && !m.archive_running())
                confirm.open(qsTr("Permissions are still being changed"),
                             qsTr("It can't stop part-way. The tab closes when it's done."),
                             qsTr("Close when finished"), false)
            else if (m.permissions_running())
                confirm.open(qsTr("Archive work and a permissions change are still running"),
                             qsTr("They can't stop part-way. The tab closes when they're done."),
                             qsTr("Close when finished"), false)
            else
                confirm.open(qsTr("Archive work is still running"),
                             qsTr("It can't stop part-way. The tab closes when it's done."),
                             qsTr("Close when finished"), false)
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
            if (p && p.startsWith("token:")) it.navigate_token(p.substring(6))
            else if (p && p !== "") it.navigate(p)
            else it.reload()
        }
        win.viewMode = tabViews[i] || "rows"
        // activeTab's synchronous handler ran before paneTab was updated.
        // Publish the final pane owner even when the tab/view did not change.
        win.refreshPaneTabCount()
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
        hints.message = ""
        // An EXPLICIT choice, so it is remembered for this location.
        // applyRememberedView() must never call this.
        dirModel.remember_view(m)
        sessionSaveTimer.restart()
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
            // The pane shows tabViews, not viewMode: update both, or the
            // switcher says one view while the pane shows another.
            if (win.activeTab >= 0) win.tabViews[win.activeTab] = want
            win.refreshPanes()
            qaDump("view")
        }
    }

    // Return keyboard control to the file list.
    //
    // Overlays (palette, confirm, filter) take focus into their own text
    // fields; if it is not handed back the window looks frozen to the
    // keyboard. Focus is only taken when no overlay is still up, so this can
    // be called freely.
    /// Ctrl+F: search this folder and below as you type.
    function openSearch() {
        // The last action's words ("Selected Work") outrank the model's
        // status, so the result count would never show while searching.
        hints.message = ""
        if (filterBar.filterActive) {
            filterInput.text = ""
            dirModel.set_filter_text("")
            filterBar.filterActive = false
        }
        filterBar.searchActive = true
        searchInput.text = dirModel.finding ? dirModel.findNeedle : ""
        searchInput.selectAll()
        searchInput.forceActiveFocus()
        win.qaDump("search-open")
    }

    /// Leave the search field. `keep`: stay on the results with the list
    /// focused (Escape there goes back); otherwise return to the folder and
    /// the row the search started from.
    function closeSearch(keep) {
        searchDebounce.stop()
        if (keep) {
            const t = searchInput.text.trim()
            // Enter before the pause ran: search for what is typed now.
            if (t !== "" && t !== dirModel.findNeedle) dirModel.find_start(t)
        } else {
            dirModel.find_clear()
        }
        filterBar.searchActive = false
        win.focusList()
        win.qaDump(keep ? "search-kept" : "search-closed")
    }

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
        if (palette.visible || confirm.visible || connectionRecovery.visible || contextMenu.open
            || conflictDialog.visible || settingsPanel.open || trashPanel.open || recentPanel.open || connectPanel.open
            || prompt.visible || prompt.claimingFocus || renameCard.open || permissionsDialog.open
            || deviceSharing.open)
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
        quickLook.show(dirModel.peek_row(dirModel.cursor), dirModel.cursor)
        win.qaDump("quicklook")
    }

    /// Choosing for another app: hand back the pick and close. Nothing is
    /// opened, copied or changed — the app does what it wants with it.
    function chooserAccept() {
        // A Save is never answered by a stray Return meant for the list:
        // Return in the list opens folders; only the name field or the
        // Save button accept (DESIGN: dialogs never act on a guess).
        const err = dirModel.chooser_accept(chooserBar.name)
        chooserBar.error = err
        win.qaDump("chooser-accept")
        if (err === "") Qt.quit()
    }

    function activateRow(row) {
        // Saving: Enter on a file puts its name in the field (to replace it,
        // the user still presses Save); folders open as usual.
        if (chooserBar.active && chooserBar.mode === "save" && !dirModel.is_dir(row)) {
            chooserBar.name = dirModel.entry_name(row)
            chooserBar.focusName()
            return
        }
        // In an Open dialog, Enter on a file is "Open"; folders still open.
        if (chooserBar.active && chooserBar.mode !== "save" && chooserBar.mode !== "open-folder"
                && !dirModel.is_dir(row)) {
            dirModel.focus_row(row)
            win.chooserAccept()
            return
        }
        // Several files selected: one viewer showing all of them beats five
        // viewers showing one each (DESIGN §6.18). Returns "" when this is
        // not a multi-file situation, so ordinary activation still runs.
        if (dirModel.sel_count() > 1) {
            const many = dirModel.activate_selection()
            if (many.startsWith("OK:")) {
                hints.message = many.substring(3)
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
        // A program is never opened by a click; the Run prompt explains it
        // and, if it is not allowed to run yet, offers that too.
        if (r === "program") { win.askRun(row); return }
        if (r.startsWith("blocked:"))
            hints.message = r.substring(8)
        else if (r === "file")
            hints.message = qsTr("Opened ") + dirModel.entry_name(row)
        win.qaDump("activate")
    }

    /// Run an action from the key table. Returns false when the action does
    /// not apply right now, so the key falls through to the usual handling.
    /// Toggle a colour tag ("1"–"7") or remove the colours ("clear") on the
    /// selection, else the cursor row (DESIGN §6.25).
    function applyTag(op) {
        const err = dirModel.set_tags(op)
        if (err !== "") win.alertText = err
        else win.alertText = ""
        // Said as a message, not left in the status: the reload that shows
        // the new dots finishes after this and puts "N items" back.
        hints.message = err === "" ? dirModel.status : ""
        win.qaDump("tags")
    }

    function runKeyAction(a) {
        if (a.startsWith("tag-")) {
            win.applyTag(a.substring(4))
            return true
        }
        if (a.startsWith("tab-")) {
            const n = parseInt(a.substring(4))
            const mine = win.paneTabs(win.activePane)
            if (n >= 1 && n <= mine.length) activateTab(mine[n - 1])
            else hints.message = qsTr("There is no tab ") + n
            win.qaDump("tab-number")
            return true
        }
        if (a === "invert-selection") {
            dirModel.sel_invert()
            win.qaDump("select")
            return true
        }
        if (a === "deselect-all") { dirModel.sel_clear(); win.qaDump("select"); return true }
        if (a === "keymap") { win.showKeyMap(); return true }
        if (a === "toggle-split") {
            if (win.paneCount > 1) win.closePane(win.activePane)
            else win.splitPane()
            return true
        }
        if (a === "zoom-in" || a === "zoom-out" || a === "zoom-reset") {
            win.zoom(a)
            return true
        }
        if (a === "new-file") { win.askNewFile(); return true }
        if (a === "new-folder") { win.askNewFolder(); return true }
        if (a === "jump") { palette.openJump(); return true }
        if (a === "search") { win.openSearch(); return true }
        if (a === "eject") {
            const dev = dirModel.device_here()
            if (dev === "") hints.message = qsTr("This folder is not on a removable drive")
            else {
                const err = dirModel.device_unmount(dev)
                if (err !== "") win.alertText = err
            }
            win.qaDump("eject")
            return true
        }
        if (a === "back") { dirModel.go_back(); return true }
        if (a === "forward") { dirModel.go_forward(); return true }
        if (a === "up") { dirModel.navigate_up(); return true }
        if (a === "open") { win.activateRow(dirModel.cursor); return true }
        if (a === "rename") { win.askRename(); return true }
        if (a === "trash") { win.askTrash(); return true }
        return false
    }

    /// Ctrl+= / Ctrl+- / Ctrl+0, as in the rest of Omarchy. In Grid the
    /// tiles grow and shrink (what a user zooming a photo folder wants);
    /// elsewhere text and rows do. Not saved: a zoom is for now.
    function zoom(a) {
        if (win.viewMode === "grid") {
            const g = win.activePaneItem().currentView()
            if (a === "zoom-in") g.zoomIn()
            else if (a === "zoom-out") g.zoomOut()
            else g.zoomReset()
            hints.message = qsTr("Tile size ") + g.tileSize
        } else {
            const steps = [0.8, 0.9, 1.0, 1.1, 1.25, 1.4, 1.6]
            let i = steps.indexOf(Tokens.zoom)
            if (i < 0) i = 2
            if (a === "zoom-in") i = Math.min(steps.length - 1, i + 1)
            else if (a === "zoom-out") i = Math.max(0, i - 1)
            else i = 2
            Tokens.zoom = steps[i]
            hints.message = qsTr("Zoom ") + Math.round(Tokens.zoom * 100) + "%"
        }
        win.qaDump("zoom")
    }

    function showKeyMap() {
        keyMapPanel.show(dirModel.keymap_json(win.keyPreset), win.keyPreset)
        win.qaDump("keymap")
    }

    /// The permissions card reads the same targets every dialog names
    /// (D46) on a worker, then shows them as checkboxes (DESIGN §6.26).
    function askPermissions() {
        win.alertText = ""
        permissionsDialog.openFor(dirModel)
        win.qaDump("permissions")
    }

    /// The safe prompt before running a program: names the file; Cancel has
    /// the focus, so Enter cancels. A program that arrived without its run
    /// permission (a downloaded AppImage) is offered that permission here,
    /// in the same deliberate step, rather than sent to Permissions….
    property var pendingRunModel: null
    property int pendingRunToken: 0
    function askRun(row) {
        const model = dirModel
        const target = JSON.parse(model.run_prepare(row))
        if (target.error) {
            win.alertText = target.error
            win.qaDump("run-refused")
            return
        }
        const allow = target.state === "allow"
        win.pendingRunModel = model
        win.pendingRunToken = target.token
        confirm.purpose = allow ? "allow-run" : "run"
        confirm.open(allow ? qsTr("Allow %1 to run?").arg(target.name) : qsTr("Run %1?").arg(target.name),
                     qsTr("Only run programs you trust."),
                     allow ? qsTr("Allow and run") : qsTr("Run"), true)
        win.qaDump("run-confirm-open")
    }

    function askNewFile() {
        prompt.mode = "newfile"
        prompt.openWith(qsTr("New file"), qsTr("untitled.txt"), qsTr("An empty file, created in this folder."), -1)
        win.qaDump("prompt")
    }

    /// Middle-click on a folder: open it in a new tab behind this one, as a
    /// browser does (Flea #219). Appended, so no other tab index moves.
    function openRowInBackgroundTab(model, row) {
        if (!model || !model.is_dir(row)) return
        tabList.append({ startPath: model.entry_path(row), label: model.entry_name(row),
                         pane: win.activePane })
        tabViews.push("rows")
        win.refreshPaneTabCount()
        hints.message = qsTr("Opened in a new tab: ") + model.entry_name(row) + qsTr("  ·  Ctrl+Tab to go there")
        win.qaDump("tab-new-background")
    }

    /// Every command the palette and the right-click menu can run. One
    /// dispatcher, so a menu entry and its palette twin can never drift.
    function runAction(a) {
        if (a === "open") win.activateRow(dirModel.cursor)
        else if (a === "quicklook") win.toggleQuickLook()
        else if (a === "settings") win.openSettings()
        else if (a === "trash-view") { trashPanel.show(dirModel.trash_list()); win.qaDump("trash-open") }
        else if (a === "recent") { recentPanel.show(dirModel.recent_list()); win.qaDump("recent-open-panel") }
        else if (a === "connect") { win.refreshPlaces(); connectPanel.show(); win.qaDump("connect-open") }
        else if (a === "tag-menu") {
            // The seven colours, in the place the menu was (DESIGN §6.25).
            const names = [qsTr("Red"), qsTr("Orange"), qsTr("Yellow"), qsTr("Green"),
                           qsTr("Blue"), qsTr("Purple"), qsTr("Gray")]
            const have = dirModel.entry_tags(dirModel.cursor).split(",")
            const keys = ["red", "orange", "yellow", "green", "blue", "purple", "gray"]
            const items = names.map((n, i) => ({
                label: (have.indexOf(keys[i]) >= 0 ? "\u2713 " : "") + n,
                hint: "Alt+" + (i + 1), action: "tag-" + (i + 1) }))
            items.push({ separator: true })
            items.push({ label: qsTr("Remove colours"), hint: "Alt+0", action: "tag-clear" })
            const at = contextMenu.lastAt
            contextMenu.popup(at.x, at.y, items)
            win.qaDump("tag-menu")
        }
        else if (a.startsWith("tag-")) win.applyTag(a.substring(4))
        else if (a === "dv-open") {
            const d = win.deviceList.find((x) => x.path === win.deviceMenuPath)
            if (d) win.openDevice(d.path, d.mounted)
        }
        else if (a === "dv-eject") win.ejectDevice(win.deviceMenuPath)
        else if (a === "ph-open" || a === "ph-apps") {
            const phone = win.deviceList.find(d => d.name === win.phoneMenuId)
            win.openPhone(win.phoneMenuId, phone ? phone.stateKey : "", a === "ph-apps", true)
        }
        else if (a === "ph-eject") {
            const err = dirModel.phone_eject(win.phoneMenuId)
            if (err !== "") win.alertText = err
        }
        else if (a === "rp-off" && win.remoteMenuPlace) win.disconnectPlace(win.remoteMenuPlace.kind, win.remoteMenuPlace.name)
        else if (a === "rp-open-tab" && win.remoteMenuPlace) {
            const rp = win.connectedPlaces.find(x => x.name === win.remoteMenuPlace.name)
            if (rp) win.newTab(rp.path)
        }
        else if (a === "pin-row" || a === "unpin-row") {
            const p = dirModel.entry_path(dirModel.cursor)
            const err = a === "pin-row" ? dirModel.bookmark_add(p) : dirModel.bookmark_remove(p)
            if (err !== "") win.alertText = err
            else hints.message = (a === "pin-row" ? qsTr("Added to Bookmarks: ") : qsTr("Removed from Bookmarks: ")) + p
            win.refreshBookmarks()
            win.qaDump(a)
        }
        else if (a === "pin-here" || a === "unpin-here") {
            const err = a === "pin-here" ? dirModel.bookmark_add(dirModel.location)
                                         : dirModel.bookmark_remove(dirModel.location)
            if (err !== "") win.alertText = err
            else hints.message = (a === "pin-here" ? qsTr("Added to Bookmarks: ") : qsTr("Removed from Bookmarks: ")) + dirModel.location
            win.refreshBookmarks()
            win.qaDump(a)
        }
        else if (a === "bm-open") { dirModel.navigate(win.bookmarkMenuPath); win.focusList() }
        else if (a === "bm-open-tab") win.newTab(win.bookmarkMenuPath)
        else if (a === "bm-remove") {
            const err = dirModel.bookmark_remove(win.bookmarkMenuPath)
            if (err !== "") win.alertText = err
            else hints.message = qsTr("Removed from Bookmarks: ") + win.bookmarkMenuPath
            win.refreshBookmarks()
            win.qaDump(a)
        }
        else if (a === "open-with") { palette.openWith("open with "); return }
        else if (a === "open-tab") win.newTab(dirModel.entry_path(dirModel.cursor))
        else if (a === "copy-path") hints.message = dirModel.copy_path(dirModel.cursor)
        else if (a === "copy-path-folder") hints.message = dirModel.copy_path(-1)
        else if (a === "paste-into") {
            win.transferStarted(dirModel, dirModel.clip_paste_into(dirModel.cursor), "paste")
        }
        else if (a === "view-rows") win.setView("rows")
        else if (a === "split") win.splitPane()
        else if (a === "unsplit") win.closePane(win.activePane)
        else if (a === "send-copy") win.sendToOtherPane(false)
        else if (a === "send-move") win.sendToOtherPane(true)
        else if (a === "view-grid") win.setView("grid")
        else if (a === "view-columns") win.setView("columns")
        else if (a === "toggle-hidden") dirModel.toggle_hidden()
        else if (a === "reload") dirModel.reload_all()
        else if (a === "up") dirModel.navigate_up()
        else if (a === "trash") win.askTrash()
        else if (a === "new-folder") win.askNewFolder()
        else if (a === "rename") win.askRename()
        else if (a === "duplicate") win.doDuplicate()
        else if (a === "copy") hints.message = dirModel.clip_copy(dirModel.cursor)
        else if (a === "cut") hints.message = dirModel.clip_cut(dirModel.cursor)
        else if (a === "paste") win.doPaste()
        else if (a === "terminal") hints.message = dirModel.open_terminal()
        else if (a === "keys") win.showKeyMap()
        else if (a === "run") win.askRun(dirModel.cursor)
        else if (a.startsWith("custom:")) {
            const err = dirModel.run_custom_action(a.substring(7))
            if (err !== "") win.alertText = err
            win.qaDump("custom-action")
        }
        else if (a.startsWith("sortto-")) {
            // The sort menu: a field keeps the current direction.
            dirModel.set_sort_to(a.substring(7), dirModel.sort_ascending)
            win.qaDump("sort")
        }
        else if (a === "sortdir-asc" || a === "sortdir-desc") {
            dirModel.set_sort_to(dirModel.sort_field, a === "sortdir-asc")
            win.qaDump("sort")
        }
        else if (a.startsWith("sort-")) { dirModel.set_sort(a.substring(5)); win.qaDump("sort") }
        else if (a === "make-link") {
            const err = dirModel.make_link()
            if (err !== "") win.alertText = err
            win.qaDump("make-link")
        }
        else if (a === "permissions") win.askPermissions()
        else if (a === "new-file") win.askNewFile()
        else if (a === "invert-selection") { dirModel.sel_invert(); win.qaDump("select") }
        else if (a === "deselect-all") { dirModel.sel_clear(); win.qaDump("select") }
        else if (a === "owner") win.askOwner()
        else if (a === "paste-image") win.pasteImage()
        else if (a === "open-admin") {
            const e = dirModel.open_as_admin()
            if (e !== "") win.alertText = e
            else hints.message = qsTr("Asked for administrator rights")
            win.qaDump("open-admin")
        }
        else if (a === "drop-copy" || a === "drop-move") {
            const m = win.dropMenuModel
            if (m) win.transferStarted(m, m.drop_on_row(win.dropMenuRow, a === "drop-move"), "drop")
        }
        else if (a === "drop-link") {
            const m = win.dropMenuModel
            if (m) {
                win.alertText = ""
                hints.message = ""
                const e = m.drop_link_on_row(win.dropMenuRow)
                if (e !== "") win.alertText = e
            }
            win.qaDump("drop-link")
        }
        else if (a === "noop") { /* nothing to do */ }
        else if (a.startsWith("openwith:")) {
            const msg = dirModel.open_with_id(dirModel.cursor, a.substring(9))
            // "OK:" marks success: the window does not change, so
            // without a line here nothing appears to have happened.
            if (msg.startsWith("OK:")) hints.message = msg.substring(3)
            else if (msg !== "") win.alertText = msg
            win.qaDump("open-with")
        }
        else if (a === "bulk") {
            win.askBulkRename()
        }
        else if (a === "bulk-pattern") {
            win.askBulkPattern()
        }
        else if (a === "search") {
            win.openSearch()
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
        else if (a === "device-sharing" || a.startsWith("device-share:")) {
            connectPanel.hide()
            // The whole sheet offers Send only for files someone explicitly
            // selected; the cursor alone is not a choice to send anything.
            const selection = dirModel.sel_count() > 0 ? dirModel.prepare_device_send(-1) : ""
            deviceSharing.show(dirModel, selection, "", a.startsWith("device-share:") ? a.substring(13) : "", "")
        }
        else if (a === "device-send") {
            const selection = dirModel.prepare_device_send(dirModel.cursor)
            deviceSharing.show(dirModel, selection, "", "", "send")
        }
        else if (a === "device-share-here" || a === "device-share-folder") {
            const folder = JSON.parse(dirModel.device_folder(a === "device-share-here" ? -1 : dirModel.cursor))
            deviceSharing.show(dirModel, "", folder, "", "folder")
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
            win.connectPlace("cloud", a.substring(9), "", "")
        }
        else if (a.startsWith("cloud-off:")) {
            const cname2 = a.substring(10)
            if (win.refuseBusyDisconnect("cloud", cname2)) return
            // Never force on the first try: unfinished uploads must be
            // reported, not silently discarded (DESIGN §6.8).
            // The answer ("ASK:" or a failure) arrives in placeDone.
            const cerr2 = dirModel.cloud_disconnect(cname2, false)
            if (cerr2 !== "") win.alertText = cerr2
        }
        else if (a === "cloud-setup") {
            if (connectPanel.busy) { connectPanel.showFeedback("", "", false); return }
            if (!win.requireSupport("cloud")) return
            connectPanel.supportKey = "cloud"
            // rclone's own config tool, in a terminal. Gust never asks
            // for a cloud password and never sees one.
            const serr = dirModel.cloud_setup()
            if (serr !== "") win.alertText = serr
            const instruction = qsTr("Finish rclone config in the terminal, then check again.")
            connectPanel.showFeedback(serr !== "" ? qsTr("Cloud storage setup needs attention") : qsTr("Continue setup in your terminal"),
                serr || instruction, serr !== "")
            if (serr === "") hints.message = instruction
            win.qaDump("cloud-setup")
        }
        else if (a.startsWith("remote-on:")) {
            win.connectPlace("ssh", a.substring(10), "", "")
        }
        else if (a.startsWith("remote-off:")) {
            const alias2 = a.substring(11)
            if (win.refuseBusyDisconnect("ssh", alias2)) return
            const err2 = dirModel.remote_disconnect(alias2)
            if (err2 !== "") win.alertText = err2
        }
        else if (a.startsWith("find:")) {
            dirModel.find_start(a.substring(5))
            win.qaDump("find-start")
        }
        else if (a.startsWith("filter:")) {
            filterBar.filterActive = true
            filterInput.text = a.substring(7)
        }
        else hints.message = a + " is not wired up yet"
        win.qaDump("action")
        win.focusList()
    }

    /// What a right-click offers. `row` -1 is the folder itself.
    function contextItems(row) {
        const sep = { separator: true }
        const canPaste = dirModel.can_paste()
        if (row < 0) {
            return [
                { label: qsTr("New folder"), hint: "Ctrl+N", action: "new-folder" },
                { label: qsTr("New file"), hint: "Ctrl+Shift+N", action: "new-file" },
                { label: qsTr("Paste"), hint: "Ctrl+V", action: "paste", enabled: canPaste },
                sep,
                { label: qsTr("Open terminal here"), hint: "", action: "terminal" },
                { label: qsTr("Open as administrator"), hint: "", action: "open-admin" },
                { label: qsTr("Copy folder path"), hint: "", action: "copy-path-folder" },
                { label: qsTr("Let devices open this folder…"), hint: "", action: "device-share-here" },
                win.bookmarkList.some(b => b.path === dirModel.location)
                    ? { label: qsTr("Remove this folder from Bookmarks"), hint: "", action: "unpin-here" }
                    : { label: qsTr("Add this folder to Bookmarks"), hint: "Ctrl+B", action: "pin-here" },
                sep,
                { label: dirModel.show_hidden ? qsTr("Hide hidden files") : qsTr("Show hidden files"),
                  hint: "Ctrl+H", action: "toggle-hidden" },
                { label: qsTr("Reload"), hint: "Ctrl+R", action: "reload" }
            ]
        }
        const many = dirModel.sel_count() > 1
        const dir = dirModel.is_dir(row)
        let list = []
        if (many) list.push({ label: qsTr("Open ") + dirModel.sel_count() + qsTr(" items"), hint: "Enter", action: "open" })
        else list.push({ label: qsTr("Open"), hint: "Enter", action: "open" })
        if (!many && dir) list.push({ label: qsTr("Open in new tab"), hint: "", action: "open-tab" })
        if (!many && dir) list.push({ label: qsTr("Let devices open this folder…"), hint: "", action: "device-share-folder" })
        if (many || !dir) list.push({ label: qsTr("Send a copy to a device…"), hint: "", action: "device-send" })
        if (!many && dir) {
            const p = dirModel.entry_path(row)
            const pinned = win.bookmarkList.some(b => b.path === p)
            list.push(pinned
                ? { label: qsTr("Remove from Bookmarks"), hint: "", action: "unpin-row" }
                : { label: qsTr("Add to Bookmarks"), hint: "Ctrl+B", action: "pin-row" })
        }
        if (!many && !dir) list.push({ label: qsTr("Open with…"), hint: win.keyPreset === "vim" ? "o" : "", action: "open-with" })
        // Programs start only from here or the palette, after a prompt.
        if (!many && !dir && dirModel.program_state(row) !== "")
            list.push({ label: qsTr("Run…"), hint: "", action: "run" })
        list.push({ label: qsTr("Quick Look"), hint: "Space", action: "quicklook" })
        if (!many) list.push({ label: qsTr("Properties"), hint: "Alt+Enter", action: "quicklook" })
        list.push(sep)
        list.push({ label: qsTr("Cut"), hint: "Ctrl+X", action: "cut" })
        list.push({ label: qsTr("Copy"), hint: "Ctrl+C", action: "copy" })
        // On a folder, Paste goes INTO that folder — the one that was
        // clicked — never silently into the folder around it.
        if (!many && dir) list.push({ label: qsTr("Paste into folder"), hint: "", action: "paste-into", enabled: canPaste })
        else list.push({ label: qsTr("Paste"), hint: "Ctrl+V", action: "paste", enabled: canPaste })
        list.push({ label: many ? qsTr("Copy paths") : qsTr("Copy path"), hint: "Ctrl+Shift+C", action: "copy-path" })
        list.push(sep)
        if (many) list.push({ label: qsTr("Rename ") + dirModel.sel_count() + qsTr(" items…"), hint: "", action: "bulk" })
        else list.push({ label: qsTr("Rename…"), hint: "F2", action: "rename" })
        list.push({ label: qsTr("Duplicate"), hint: "Ctrl+D", action: "duplicate" })
        list.push({ label: qsTr("Tag…"), hint: "Alt+1–7", action: "tag-menu" })
        if (!many) list.push({ label: qsTr("Make link"), hint: "", action: "make-link" })
        // The user's own actions (~/.config/gust/actions/*.toml) that fit
        // this selection.
        let custom = []
        try { custom = JSON.parse(dirModel.custom_actions_json()).actions } catch (e) { custom = [] }
        if (custom.length > 0) {
            list.push(sep)
            for (const c of custom) list.push({ label: c.name, hint: "", action: "custom:" + c.id })
        }
        list.push({ label: qsTr("Permissions…"), hint: "", action: "permissions" })
        list.push({ label: qsTr("Owner…"), hint: "", action: "owner" })
        list.push({ label: qsTr("Compress"), hint: "", action: "compress" })
        if (!many && dirModel.row_is_archive(row)) list.push({ label: qsTr("Extract here"), hint: "", action: "extract" })
        list.push(sep)
        list.push({ label: dirModel.delete_action_label(), hint: "Delete", action: "trash", danger: true })
        return list
    }

    function openContextMenu(row, x, y) {
        if (!dirModel) return
        if (quickLook.open) quickLook.hide()
        win.alertText = ""
        contextMenu.popup(x, y, win.contextItems(row))
        win.qaDump("context-menu")
    }

    /// The toolbar's sort menu, under its button. The check marks what is
    /// active now; choosing re-sorts this folder and is remembered for it.
    function openSortMenu() {
        if (!dirModel) return
        if (quickLook.open) quickLook.hide()
        const f = dirModel.sort_field
        const asc = dirModel.sort_ascending
        const mark = (on) => on ? "✓" : ""
        const p = sortButton.mapToItem(null, 0, sortButton.height)
        contextMenu.popup(p.x, p.y, [
            { label: qsTr("Name"), hint: mark(f === "name"), action: "sortto-name" },
            { label: qsTr("Kind"), hint: mark(f === "kind"), action: "sortto-kind" },
            { label: qsTr("Size"), hint: mark(f === "size"), action: "sortto-size" },
            { label: qsTr("Date modified"), hint: mark(f === "modified"), action: "sortto-modified" },
            { separator: true },
            { label: qsTr("Ascending"), hint: mark(asc), action: "sortdir-asc" },
            { label: qsTr("Descending"), hint: mark(!asc), action: "sortdir-desc" }
        ])
        win.qaDump("sort-menu")
    }

    /// Menu key / Shift+F10: the same menu, anchored under the cursor row.
    function openContextMenuFromKeyboard() {
        const row = dirModel.count > 0 ? dirModel.cursor : -1
        if (row >= 0 && !dirModel.sel_contains(row) && dirModel.sel_count() > 0)
            dirModel.sel_only(row)
        const p = row >= 0 ? win.activePaneItem().cursorAnchor() : Qt.point(win.width / 3, win.height / 3)
        win.openContextMenu(row, p.x, p.y)
        contextMenu.current = contextMenu.step(-1, 1)
    }

    property int pendingTrashRow: -1

    // --- operations --------------------------------------------------
    function askRename() {
        // Several selected: rename them all at once in the card.
        if (dirModel.sel_count() > 1) { win.askBulkRename(); return }
        const row = dirModel.cursor
        const name = dirModel.entry_name(row)
        if (name === "") { hints.message = "Nothing selected"; return }
        prompt.mode = "rename"
        prompt.openWith(qsTr("Rename"), name,
                        qsTr("Enter rename · Esc cancel"), row)
        win.qaDump("prompt")
    }

    function askNewFolder() {
        prompt.mode = "newfolder"
        prompt.openWith(qsTr("New folder"), "", qsTr("Created in this folder."), -1)
        win.qaDump("prompt")
    }

    function doPaste() {
        const r = dirModel.clip_paste()
        // No files to paste: an image on the clipboard becomes a file.
        if (r === "nothing to paste") { win.pasteImage(); return }
        win.transferStarted(dirModel, r, "paste")
    }
    function pasteFinished(model, message, allowImage) {
        if (message === "nothing to paste" && allowImage) { win.pasteImage(model); return }
        win.transferStarted(model, message, "paste")
    }
    function pasteImage(model) {
        const e = (model || dirModel).paste_image()
        if (e !== "") { win.alertText = e; return }
        hints.message = qsTr("Pasting the image…")
        win.qaDump("paste-image-started")
    }

    function askOwner() {
        const n = dirModel.sel_count()
        prompt.mode = "owner"
        prompt.openWith(n > 1 ? qsTr("Owner for ") + n + qsTr(" items") : qsTr("Owner"), "",
                        qsTr("Now: ") + dirModel.owner_text() + qsTr("  ·  user, :group or user:group"), -1)
        win.qaDump("prompt")
    }

    /// Where a right-button drag was released, and in which pane's model.
    property var dropMenuModel: null
    property int dropMenuRow: -1
    function openDropMenu(model, row, x, y) {
        win.dropMenuModel = model
        win.dropMenuRow = row
        contextMenu.popup(x, y, [
            { label: qsTr("Copy here"), hint: "", action: "drop-copy" },
            { label: qsTr("Move here"), hint: "", action: "drop-move" },
            { label: qsTr("Link here"), hint: "", action: "drop-link" },
            { separator: true },
            { label: qsTr("Cancel"), hint: "Esc", action: "noop" }
        ])
        hints.message = qsTr("Choose Copy, Move or Link · Esc cancels")
        win.qaDump("drop-menu")
    }

    // --- tabs at start ------------------------------------------------------
    Timer { id: sessionSaveTimer; interval: 600; onTriggered: win.saveSession() }
    onTabCountChanged: sessionSaveTimer.restart()
    function saveSession() {
        if (win.adminMode) return
        const lines = []
        for (let i = 0; i < tabList.count; i++) {
            const m = tabs.objectAt(i)
            const sp = tabList.get(i).startPath
            let tok = ""
            if (m && (m.everActivated || sp === "")) tok = m.location_token()
            else if (sp.startsWith("token:")) tok = sp.substring(6)
            else tok = dirModel.token_for(sp)
            lines.push(tok + " " + (win.tabViews[i] || "rows"))
        }
        dirModel.session_save(lines.join("\n"), win.activeTab)
    }
    /// Reopen last time's tabs (Settings → Tabs at start). Only the active
    /// one lists its folder now; the others when they are first shown.
    function restoreSession() {
        const raw = dirModel.session_json()
        if (raw === "") return false
        let s
        try { s = JSON.parse(raw) } catch (e) { return false }
        if (!s.tabs || s.tabs.length === 0) return false
        dirModel.start_at_token(s.tabs[0].token)
        win.tabViews[0] = s.tabs[0].view
        for (let i = 1; i < s.tabs.length; i++) {
            tabList.append({ startPath: "token:" + s.tabs[i].token, label: s.tabs[i].name, pane: 0 })
            win.tabViews.push(s.tabs[i].view)
        }
        win.refreshPaneTabCount()
        if (s.active > 0 && s.active < tabList.count) Qt.callLater(() => win.activateTab(s.active))
        else { win.viewMode = win.tabViews[0]; win.refreshPanes() }
        hints.message = qsTr("Reopened ") + s.tabs.length + qsTr(" tabs from last time")
        return true
    }

    /// Every copy/move funnel returns "" (started), "?conflict" (names are
    /// taken: ask first) or a refusal. One handler, so paste, drop and
    /// send-to-pane cannot answer the same situation three different ways.
    function transferStarted(model, err, tag) {
        // A new mouse action must not keep displaying an older refusal.
        // Keyboard actions already clear it, but a drop need not send a key.
        win.alertText = ""
        if (err === "?clipboard") {
            hints.message = qsTr("Reading the clipboard…")
            win.qaDump("clipboard-reading")
            return
        }
        if (err === "?conflict") {
            win.queueConflict(model)
            return
        }
        if (err !== "") { win.alertText = err; win.qaDump(tag + "-refused"); return }
        hints.message = qsTr("Started")
        jobTimer.start()
        win.qaDump(tag + "-started")
    }

    /// The model whose transfer is waiting on the conflict dialog.
    property var conflictModel: null
    property var conflictQueue: []

    function conflictOwnerLive(model) {
        if (!model) return false
        for (let i = 0; i < tabList.count; i++) {
            if (tabs.objectAt(i) === model) return true
        }
        return false
    }

    function queueConflict(model) {
        if (!win.conflictOwnerLive(model) || win.conflictModel === model
            || win.conflictQueue.indexOf(model) >= 0) return
        win.conflictQueue = win.conflictQueue.concat([model])
        win.drainConflicts()
    }

    function drainConflicts() {
        // cancelled() precedes close(): never present a queued owner while
        // the previous dialog is still visible or owns its answer route.
        if (win.conflictModel || conflictDialog.visible) return
        while (win.conflictQueue.length > 0) {
            const m = win.conflictQueue[0]
            win.conflictQueue = win.conflictQueue.slice(1)
            if (!win.conflictOwnerLive(m) || m.conflict_json() === "") continue
            win.conflictModel = m
            win.showNextConflict()
            return
        }
    }

    function forgetConflictOwner(model) {
        win.conflictQueue = win.conflictQueue.filter(m => m !== model)
        if (win.conflictModel === model) {
            win.conflictModel = null
            conflictDialog.close()
        }
    }

    function showNextConflict() {
        const raw = win.conflictOwnerLive(win.conflictModel) ? win.conflictModel.conflict_json() : ""
        if (raw === "") { win.conflictModel = null; conflictDialog.close(); return }
        conflictDialog.show(JSON.parse(raw))
        win.qaDump("conflict")
    }

    function doDuplicate() {
        const err = dirModel.duplicate_row(dirModel.cursor)
        if (err !== "") { win.alertText = err; win.qaDump("dup-refused"); return }
        hints.message = qsTr("Duplicating…")
        jobTimer.start()
        win.qaDump("dup-started")
    }

    // The confirmation's body before the total arrives.
    property string deleteBody: ""
    Connections {
        target: dirModel
        function onDelete_totalChanged() {
            if (!confirm.visible || (confirm.purpose !== "trash") || dirModel.delete_total === "") return
            if (win.deleteBody === "") win.deleteBody = confirm.body.split("\n\n")[0]
            confirm.body = win.deleteBody + "\n\n" + dirModel.delete_total
            // Dump after the dialog has laid out the new text, so the
            // button positions in the state match the screen.
            Qt.callLater(() => win.qaDump("delete-total"))
        }
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
            : "\"" + dirModel.target_name() + "\""

        // What is really inside, measured on a worker; the dialog's body
        // gains the total when it arrives (see onDelete_totalChanged).
        win.deleteBody = ""
        dirModel.request_delete_total()
        // The body carries a placeholder line that the total later fills,
        // so the buttons do not jump under the pointer when it arrives.
        confirm.open(
            hard ? "Delete permanently?" : "Move to Trash?",
            (hard ? subject + " — no Trash here. This CANNOT be undone." : subject)
              + "\n\n" + qsTr("Counting…"),
            dirModel.delete_action_label(),
            hard)
    }

    // --- layout ------------------------------------------------------------
    // The mouse's side buttons are Back and Forward everywhere in the
    // window. A handler, not a MouseArea: it takes only those two buttons
    // and never competes with a click, drag or menu.
    TapHandler {
        acceptedButtons: Qt.BackButton | Qt.ForwardButton
        gesturePolicy: TapHandler.WithinBounds
        onTapped: (point, button) => {
            if (button === Qt.BackButton) dirModel.go_back()
            else dirModel.go_forward()
        }
    }

    Column {
        anchors.fill: parent

        // The administrator window says so, on every screen of it.
        Rectangle {
            id: adminBanner
            width: parent.width
            height: win.adminMode ? Tokens.controlHeight : 0
            visible: win.adminMode
            color: Tokens.withAlpha(Tokens.danger, 0.16)
            Rectangle {
                width: Math.max(2, Tokens.borderWidth * 2)
                height: parent.height
                color: Tokens.danger
            }
            Text {
                textFormat: Text.PlainText
                anchors.verticalCenter: parent.verticalCenter
                x: Tokens.rowPaddingX
                width: parent.width - Tokens.rowPaddingX * 2
                text: qsTr("Administrator — changes affect the whole system")
                color: Tokens.foregroundBright
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
                font.bold: true
                elide: Text.ElideRight
            }
            Accessible.role: Accessible.StaticText
            Accessible.name: qsTr("Administrator window")
        }

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
                    id: tabRepeater
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
                            textFormat: Text.PlainText
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
                            id: tabMouse
                            anchors.fill: parent
                            acceptedButtons: Qt.LeftButton | Qt.MiddleButton
                            // A few pixels before a click becomes a drag, so
                            // a slightly imprecise click is still a click —
                            // the same threshold the file list uses.
                            property point pressAt: Qt.point(0, 0)
                            property bool dragging: false

                            onPressed: (mouse) => {
                                tabMouse.pressAt = Qt.point(mouse.x, mouse.y)
                                tabMouse.dragging = false
                            }
                            onPositionChanged: (mouse) => {
                                if (!(mouse.buttons & Qt.LeftButton)) return
                                const dx = mouse.x - tabMouse.pressAt.x
                                const dy = mouse.y - tabMouse.pressAt.y
                                if (!tabMouse.dragging
                                    && Math.abs(dx) + Math.abs(dy) < 8) return
                                tabMouse.dragging = true
                                const p = tabMouse.mapToItem(null, mouse.x, mouse.y)
                                win.tabDragMoved(parent.index, p.x, p.y)
                            }
                            onReleased: (mouse) => {
                                if (!tabMouse.dragging) return
                                tabMouse.dragging = false
                                const p = tabMouse.mapToItem(null, mouse.x, mouse.y)
                                win.tabDragDropped(parent.index, p.x, p.y)
                            }
                            onCanceled: {
                                tabMouse.dragging = false
                                win.tabDragEnded()
                            }
                            onClicked: (mouse) => {
                                if (tabMouse.dragging) return
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
                        textFormat: Text.PlainText
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
                        property bool enabledAct: modelData.act === "back" ? win.canBack
                                                : modelData.act === "forward" ? win.canForward
                                                : true
                        color: navHover.hovered && enabledAct
                               ? Tokens.withAlpha(Tokens.foreground, 0.1) : "transparent"
                        HoverHandler { id: navHover }
                        Text {
                            textFormat: Text.PlainText
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
                    onDragHover: (t) => { hints.message = t; win.qaDump("drag-hover") }
                    onDragLanded: (m, e) => { hints.message = ""; win.transferStarted(m, e, "drop") }
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

                // Sort: what the list is ordered by, and a menu to change it,
                // where Nautilus keeps the same control. Every view, not
                // only Rows, whose column headers also sort.
                Rectangle {
                    id: sortButton
                    objectName: "sortButton"
                    width: sortText.implicitWidth + Tokens.spaceLg * 2
                    height: Tokens.controlHeight
                    color: sortHover.hovered ? Tokens.withAlpha(Tokens.foreground, 0.1) : "transparent"
                    radius: Tokens.radius
                    readonly property string word: {
                        const f = win.dirModel ? win.dirModel.sort_field : "name"
                        return f === "kind" ? qsTr("Kind")
                             : f === "size" ? qsTr("Size")
                             : f === "modified" ? qsTr("Modified")
                             : qsTr("Name")
                    }
                    HoverHandler { id: sortHover }
                    Text {
                        textFormat: Text.PlainText
                        id: sortText
                        anchors.centerIn: parent
                        text: sortButton.word
                              + ((win.dirModel && !win.dirModel.sort_ascending) ? " ▼" : " ▲")
                        color: Tokens.foreground
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSmall
                    }
                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: win.openSortMenu()
                    }
                    Accessible.role: Accessible.Button
                    Accessible.name: qsTr("Sort by %1").arg(sortButton.word)
                }

                Item { width: Tokens.spaceSm; height: 1 }

                Repeater {
                    model: [
                        { m: "rows",    glyph: "▤", nm: "Rows view" },
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
                            textFormat: Text.PlainText
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

                Item { width: Tokens.spaceSm; height: 1 }

                // Settings: a visible control, never only a shortcut.
                Rectangle {
                    objectName: "settingsButton"
                    width: Tokens.controlHeight
                    height: Tokens.controlHeight
                    color: setHover.hovered ? Tokens.withAlpha(Tokens.foreground, 0.1) : "transparent"
                    radius: Tokens.radius
                    HoverHandler { id: setHover }
                    Text {
                        textFormat: Text.PlainText
                        anchors.centerIn: parent
                        text: "⛭"
                        color: Tokens.foreground
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                    }
                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: win.openSettings()
                    }
                    Accessible.role: Accessible.Button
                    Accessible.name: qsTr("Settings")
                }
            }
        }

        // Body: sidebar + view.
        Item {
            id: body
            width: parent.width
            height: parent.height - tabStrip.height - headerBar.height - filterBar.height - adminBanner.height
                    - chooserBar.height - interruptedNotice.height - jobDrawer.height - hints.height

            Sidebar {
                id: sidebar
                width: win.sidebarVisible ? Math.round(180 * Tokens.spacingScale) : 0
                height: parent.height
                visible: win.sidebarVisible
                model: dirModel
                activePanel: trashPanel.open ? "trash" : recentPanel.open ? "recent" : ""
                onPlaceChosen: (p) => { dirModel.navigate(p); win.focusList() }
                onDragHover: (t) => { hints.message = t; win.qaDump("drag-hover") }
                onDragLanded: (m, e) => { hints.message = ""; win.transferStarted(m, e, "drop") }
                remotePlaces: win.connectedPlaces
                onConnectChosen: win.runAction("connect")
                onRemoteChosen: (p) => { dirModel.navigate(p); win.focusList() }
                onRemoteMenu: (kind, name, x, y) => {
                    win.remoteMenuPlace = { kind: kind, name: name }
                    contextMenu.popup(x, y, [
                        { label: qsTr("Open in new tab"), hint: "", action: "rp-open-tab" },
                        { separator: true },
                        { label: qsTr("Disconnect"), hint: "", action: "rp-off" }
                    ])
                    win.qaDump("remote-menu")
                }
                devices: win.deviceList
                bookmarks: win.bookmarkList
                onDeviceChosen: (path, mounted) => win.openDevice(path, mounted)
                onDeviceMenu: (path, mounted, encrypted, locked, x, y) => {
                    win.deviceMenuPath = path
                    const items = [{ label: locked ? qsTr("Unlock…") : qsTr("Open"), hint: "", action: "dv-open" }]
                    // A locked drive has nothing mounted and nothing to eject.
                    if (!locked && (mounted || encrypted)) {
                        items.push({ separator: true })
                        items.push({ label: encrypted ? qsTr("Lock and eject") : qsTr("Eject"),
                                     hint: "", action: "dv-eject" })
                    }
                    contextMenu.popup(x, y, items)
                    win.qaDump("device-menu")
                }
                onPhoneChosen: (id, stateKey) => win.openPhone(id, stateKey)
                onPhoneMenu: (id, apps, open, x, y) => {
                    win.phoneMenuId = id
                    const items = [{ label: qsTr("Open photos and files"), hint: "", action: "ph-open" }]
                    if (apps) items.push({ label: qsTr("Open app files"), hint: "", action: "ph-apps" })
                    items.push({ separator: true })
                    items.push({ label: qsTr("Eject"), hint: "", action: "ph-eject" })
                    contextMenu.popup(x, y, items)
                    win.qaDump("phone-menu")
                }
                onTrashChosen: win.runAction("trash-view")
                onRecentChosen: win.runAction("recent")
                onBookmarkMenu: (path, x, y) => {
                    win.bookmarkMenuPath = path
                    contextMenu.popup(x, y, [
                        { label: qsTr("Open"), hint: "", action: "bm-open" },
                        { label: qsTr("Open in new tab"), hint: "", action: "bm-open-tab" },
                        { separator: true },
                        { label: qsTr("Remove from Bookmarks"), hint: "", action: "bm-remove" }
                    ])
                }
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
                    onOpenInNewTab: (r) => win.openRowInBackgroundTab(win.paneAModel, r)
                    // A click in the list gives the list the keyboard, even
                    // when this pane was already active (e.g. from a text
                    // field such as the Save name).
                    onSelectionTouched: { win.focusPane(0); win.focusList(); win.qaDump("select") }
                    onDragBegan: { win.focusPane(0); win.qaDump("drag-begin") }
                    onDropPreview: (t) => { hints.message = t }
                    onDropFailed: (owner, message) => { win.transferStarted(owner, message, "drop") }
                    onDropMenu: (r, x, y) => { win.focusPane(0); win.openDropMenu(win.paneAModel, r, x, y) }
                    onDropDone: (owner) => { win.transferStarted(owner, "", "drop") }
                    onContextRequested: (r, x, y) => { win.focusPane(0); win.openContextMenu(r, x, y) }
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
                    onOpenInNewTab: (r) => win.openRowInBackgroundTab(win.paneBModel, r)
                        onSelectionTouched: { win.focusPane(1); win.focusList(); win.qaDump("select") }
                        onDragBegan: { win.focusPane(1); win.qaDump("drag-begin") }
                        onDropPreview: (t) => { hints.message = t }
                        onDropFailed: (owner, message) => { win.transferStarted(owner, message, "drop") }
                        onDropMenu: (r, x, y) => { win.focusPane(1); win.openDropMenu(win.paneBModel, r, x, y) }
                        onDropDone: (owner) => { win.transferStarted(owner, "", "drop") }
                        onContextRequested: (r, x, y) => { win.focusPane(1); win.openContextMenu(r, x, y) }
                    }
                }

                // Where a dragged tab would land. Shown only while dragging
                // over a real target: a highlight with no drop behind it
                // would promise something that does not happen.
                Rectangle {
                    z: 20
                    visible: win.tabDropZone === "left"
                             || win.tabDropZone === "right"
                    x: win.tabDropZone === "right"
                       ? Math.round(paneArea.width * 0.67) : 0
                    y: 0
                    width: Math.round(paneArea.width * 0.33)
                    height: paneArea.height
                    color: Tokens.withAlpha(Tokens.accent, 0.12)
                    border.width: Math.max(1, Tokens.borderWidth)
                    border.color: Tokens.accent

                    Text {
                        textFormat: Text.PlainText
                        anchors.centerIn: parent
                        text: win.tabDropZone === "left" ? qsTr("Left pane")
                                                         : qsTr("Right pane")
                        color: Tokens.foregroundBright
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
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
            height: (filterActive || searchActive) ? Tokens.controlHeight : 0
            visible: filterActive || searchActive
            color: Tokens.backgroundDark
            property bool filterActive: false
            // Search as you type (Ctrl+F): the same bar, asking a different
            // question. The filter narrows this folder; search walks this
            // folder and below, on a worker, ranked (DESIGN §6.3).
            property bool searchActive: false
            onFilterActiveChanged: win.qaDump("filterbar")
            onSearchActiveChanged: win.qaDump("searchbar")

            Row {
                anchors.fill: parent
                anchors.leftMargin: Tokens.rowPaddingX
                spacing: Tokens.spaceLg

                Text {
                    id: searchLabel
                    textFormat: Text.PlainText
                    anchors.verticalCenter: parent.verticalCenter
                    text: filterBar.searchActive ? qsTr("search") : "/"
                    color: Tokens.accent
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                }
                TextInput {
                    id: filterInput
                    objectName: "filterInput"
                    visible: !filterBar.searchActive
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
                TextInput {
                    id: searchInput
                    objectName: "searchInput"
                    visible: filterBar.searchActive
                    anchors.verticalCenter: parent.verticalCenter
                    width: parent.width - searchHint.implicitWidth - searchLabel.implicitWidth
                           - Tokens.spaceLg * 3 - Tokens.rowPaddingX * 2
                    color: Tokens.foregroundBright
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                    selectByMouse: true
                    clip: true
                    // A walk per keystroke would be wasted work while a word
                    // is still being typed; a short pause starts it.
                    onTextChanged: if (filterBar.searchActive) searchDebounce.restart()
                    Keys.onEscapePressed: win.closeSearch(false)
                    Keys.onReturnPressed: win.closeSearch(true)
                    Keys.onEnterPressed: win.closeSearch(true)
                    // Down moves into the results without losing them.
                    Keys.onDownPressed: win.closeSearch(true)
                    Accessible.role: Accessible.EditableText
                    Accessible.name: "Search this folder and below"
                }
                Text {
                    id: searchHint
                    visible: filterBar.searchActive
                    textFormat: Text.PlainText
                    anchors.verticalCenter: parent.verticalCenter
                    text: dirModel.finding
                          ? qsTr("Enter keeps results · Esc goes back")
                          : qsTr("type to search here and below")
                    color: Tokens.foregroundDim
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontSmall
                }
            }
            Timer {
                id: searchDebounce
                interval: 120
                repeat: false
                onTriggered: {
                    const t = searchInput.text.trim()
                    if (t === "") dirModel.find_clear()
                    else dirModel.find_start(t)
                    win.qaDump("search-typed")
                }
            }
        }

        // Another app's Open/Save dialog: what it asked, and the answer.
        ChooserBar {
            id: chooserBar
            width: parent.width
            onAccepted: win.chooserAccept()
            onCancelled: Qt.quit()
        }

        InterruptedNotice {
            id: interruptedNotice
            width: parent.width
            onOpenDestination: (path) => {
                dirModel.navigate(path)
                win.qaDump("interrupted-open")
            }
            onDismiss: {
                dirModel.acknowledge_interrupted()
                interruptedNotice.items = []
                win.qaDump("interrupted-dismissed")
            }
        }

        JobDrawer {
            id: jobDrawer
            width: parent.width
            model: dirModel
            job: win.job
            onRequestPause: {
                dirModel.pause_job()
                hints.message = qsTr("Paused · Resume carries on, Cancel stops")
                const raw = dirModel.poll_job()
                if (raw !== "") { try { win.job = JSON.parse(raw) } catch (e) {} }
                win.qaDump("job-pause")
            }
            onRequestResume: {
                dirModel.resume_job()
                hints.message = qsTr("Resumed")
                const raw = dirModel.poll_job()
                if (raw !== "") { try { win.job = JSON.parse(raw) } catch (e) {} }
                win.qaDump("job-resume")
            }
            onRequestCancel: {
                dirModel.cancel_job()
                // Show "Stopping" now, not on the next poll.
                const raw = dirModel.poll_job()
                if (raw !== "") { try { win.job = JSON.parse(raw) } catch (e) {} }
                win.qaDump("job-cancel")
            }
        }

        HintStrip {
            id: hints
            width: parent.width
            preset: win.keyPreset
            // Bound for good: transient messages go to `message`, which
            // gives way when the folder, tab or pane changes. Writing
            // to `status` here destroyed this binding after the first action
            // ("12 items" and "reading…" never reached the strip again).
            status: dirModel.status
            // Bound, not assigned: a selection is live state and must not go
            // stale behind a message some earlier keystroke left here.
            selection: win.selectionText
            alert: win.alertText !== "" ? win.alertText : dirModel.notice
            dialog: confirm.visible || connectionRecovery.visible
            onStatusChanged: win.qaDump("hint")
            onMessageChanged: win.qaDump("hint")
            // A message belongs to the folder it was said in: leaving it, or
            // switching tab or pane, brings the folder's own status back.
            Connections {
                target: dirModel
                function onLocationChanged() { hints.message = "" }
            }
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

            // h in Quick Look computes the file's SHA-256 (on a worker;
            // Escape or closing stops it). Checked before the key table so
            // Vim's h does not go up a folder under an open preview.
            if (quickLook.open && event.key === Qt.Key_H && event.modifiers === Qt.NoModifier) {
                dirModel.hash_row(quickLook.row)
                win.qaDump("hash")
                event.accepted = true; return
            }
            // A preview owns arrow keys so they cannot navigate the list
            // underneath it. PDF pages keep Left/Right; audio seeks with Shift.
            if (quickLook.open && event.modifiers === Qt.ShiftModifier
                && (event.key === Qt.Key_Left || event.key === Qt.Key_Right)) {
                quickLook.seekBy(event.key === Qt.Key_Left ? -5 : 5)
                win.qaDump("quicklook-key")
                event.accepted = true; return
            }
            if (quickLook.open && event.modifiers === Qt.NoModifier) {
                let used = false
                if (event.key === Qt.Key_P || event.key === Qt.Key_K) used = quickLook.togglePlay()
                else if (event.key === Qt.Key_X && quickLook.programSeen) {
                    // x flips the run permission of the program shown.
                    quickLook.setRunnable(quickLook.program !== "run")
                    used = true
                }
                else if (event.key === Qt.Key_Left) used = quickLook.turnPage(-1) || quickLook.browse(-1)
                else if (event.key === Qt.Key_Right) used = quickLook.turnPage(1) || quickLook.browse(1)
                else if (event.key === Qt.Key_Up) used = quickLook.zoomBy(1)
                else if (event.key === Qt.Key_Down) used = quickLook.zoomBy(-1)
                else if (event.key === Qt.Key_PageUp) used = quickLook.turnPage(-1)
                else if (event.key === Qt.Key_PageDown) used = quickLook.turnPage(1)
                else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                    // Enter opens the file in its own application; the
                    // preview (and any sound it is playing) gets out of the way.
                    quickLook.hide()
                    win.activateRow(dirModel.cursor)
                    win.qaDump("quicklook")
                    event.accepted = true; return
                }
                if (used) { win.qaDump("quicklook-key"); event.accepted = true; return }
            }

            // Preset-aware chords from the shared key table (gust-core
            // keymap.rs), which also renders the ? key map: a shortcut and
            // its documentation cannot drift apart.
            const mapped = dirModel.key_action(win.keyPreset, event.key, event.modifiers)
            if (mapped !== "" && win.runKeyAction(mapped)) {
                event.accepted = true; return
            }

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
            if (event.key === Qt.Key_Menu
                || (event.key === Qt.Key_F10 && (event.modifiers & Qt.ShiftModifier))) {
                win.openContextMenuFromKeyboard()
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

            // Alt+Left / Alt+Right: Back and Forward, as in every browser
            // and file manager. Without them the history existed but only
            // the toolbar arrows could reach it.
            if ((event.modifiers & Qt.AltModifier)
                    && (event.key === Qt.Key_Left || event.key === Qt.Key_Right)) {
                if (event.key === Qt.Key_Left) dirModel.go_back()
                else dirModel.go_forward()
                event.accepted = true; return
            }

            // Alt+Enter: Properties, the convention on every desktop.
            if ((event.key === Qt.Key_Return || event.key === Qt.Key_Enter)
                    && (event.modifiers & Qt.AltModifier)) {
                win.toggleQuickLook()
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
                // Settings opened by mouse may not hold the keyboard yet;
                // Escape still closes it rather than acting underneath.
                if (settingsPanel.open) {
                    settingsPanel.hide()
                    win.qaDump("settings-closed")
                    event.accepted = true; return
                }
                if (dirModel.job_running()) {
                    dirModel.cancel_job()
                    hints.message = qsTr("Cancelling…")
                    win.qaDump("job-cancel")
                    event.accepted = true; return
                }
                if (dirModel.clipboard_busy()) {
                    dirModel.cancel_clip_paste()
                    hints.message = qsTr("Paste cancelled")
                    win.qaDump("clipboard-cancelled")
                    event.accepted = true; return
                }
                if (chooserBar.active && dirModel.sel_count() === 0 && dirModel.filter === "") {
                    // Escape in an Open/Save dialog is Cancel, as everywhere.
                    Qt.quit()
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
                    hints.message = ""
                    win.qaDump("filter-cleared")
                    event.accepted = true; return
                }
                // Last rung: abandon a pending copy or cut. Without this a
                // clipboard could only ever be replaced, never given up —
                // and clip_clear() had no caller anywhere in the UI.
                if (dirModel.clip_summary() !== "") {
                    dirModel.clip_clear()
                    hints.message = qsTr("Clipboard cleared")
                    win.qaDump("clip-cleared")
                    event.accepted = true; return
                }
                break
            case Qt.Key_F1:
                win.showKeyMap()
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
                if (event.key === Qt.Key_L) { palette.openJump(); event.accepted = true; return }
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
                if (event.key === Qt.Key_R) { dirModel.reload_all(); event.accepted = true; return }
                if (event.key === Qt.Key_Comma) { win.openSettings(); event.accepted = true; return }
                if (event.key === Qt.Key_B) {
                    // The folder under the cursor, else the one being viewed.
                    const onDir = dirModel.count > 0 && dirModel.is_dir(dirModel.cursor)
                    const p = onDir ? dirModel.entry_path(dirModel.cursor) : dirModel.location
                    const pinned = win.bookmarkList.some(b => b.path === p)
                    if (onDir) win.runAction(pinned ? "unpin-row" : "pin-row")
                    else win.runAction(pinned ? "unpin-here" : "pin-here")
                    event.accepted = true; return
                }
                if (event.key === Qt.Key_Z) {
                    const label = dirModel.undo_label()
                    const err = dirModel.undo_last()
                    if (err === "") {
                        win.alertText = ""
                        hints.message = label === "Undo: remove the link" ? qsTr("Removed the link")
                                     : label === "Undo: remove the links" ? qsTr("Removed the links")
                                     : label === "Undo: put owners back" ? qsTr("Owners put back")
                                     : label === "Undo: remove the pasted image" ? qsTr("Removed the pasted image")
                                     : label === "Undo: put permissions back" ? qsTr("Permissions put back")
                                     : label === "Undo: restore the original name" ? qsTr("Restored the original name")
                                     : /^Undo: put \d+ names? back$/.test(label) ? qsTr("Names put back")
                                     : label === "Undo: put the tags back" ? qsTr("Tags put back")
                                     : label.startsWith("Undo: remove") ? "Copies removed"
                                     : label.startsWith("Undo: move") ? "Moved back"
                                     : "Restored from Trash"
                    } else {
                        win.alertText = "Cannot undo: " + err
                    }
                    win.qaDump("undo")
                    event.accepted = true; return
                }
                if (event.key === Qt.Key_C && (event.modifiers & Qt.ShiftModifier)) {
                    hints.message = dirModel.copy_path(dirModel.cursor)
                    win.qaDump("copy-path"); event.accepted = true; return
                }
                if (event.key === Qt.Key_C) {
                    hints.message = dirModel.clip_copy(dirModel.cursor)
                    win.qaDump("clip"); event.accepted = true; return
                }
                if (event.key === Qt.Key_X) {
                    hints.message = dirModel.clip_cut(dirModel.cursor)
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
                case Qt.Key_Y: hints.message = dirModel.clip_copy(dirModel.cursor)
                               win.qaDump("clip"); event.accepted = true; return
                case Qt.Key_D: hints.message = dirModel.clip_cut(dirModel.cursor)
                               win.qaDump("clip"); event.accepted = true; return
                case Qt.Key_P: win.doPaste(); event.accepted = true; return
                }
                return
            }

            // Standard preset: printable keys select a matching row in THIS
            // folder. They never start a search (Nautilus #1157).
            if (event.text.length === 1 && event.text >= " ") {
                const row = dirModel.type_ahead(event.text)
                hints.message = row >= 0
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
        property int ticks: 0
        interval: 100
        repeat: true
        running: false
        onTriggered: {
            // Every tab's job advances and finishes even when its tab is not
            // on screen: a job is finalised (reload, undo record) only when
            // its own model is polled, and a background copy used to hang
            // there as "running" until its tab was shown again.
            for (let i = 0; i < tabList.count; i++) {
                const o = tabs.objectAt(i)
                if (o && o !== dirModel && o.job_running()) o.poll_job()
            }
            const raw = dirModel.poll_job()
            if (raw === "") {
                // This tab has no job (finished, or never had one). Keep
                // ticking while any other tab still does.
                win.job = null
                if (win.runningJobModels().length === 0) {
                    jobTimer.stop()
                    win.qaDump("job-done")
                }
                return
            }
            try {
                win.job = JSON.parse(raw)
            } catch (e) {
                win.job = null
            }
            // Twice a second while running, so QA can watch speed and ETA.
            if (win.job && win.job.state === "Running" && (++jobTimer.ticks % 5) === 0)
                win.qaDump("job-progress")
            if (win.job && win.job.state !== "Running") {
                if (win.runningJobModels().length === 0) jobTimer.stop()
                hints.message = win.job.message
                // A failed transfer is often a place that went away: say so
                // in the sidebar at once rather than still showing it
                // "connected".
                if (win.job.state === "Failed") win.refreshPlaces()
                win.qaDump("job-finished")
            }
        }
    }

    TextPrompt {
        id: prompt
        anchors.fill: parent
        property string mode: ""
        onVisibleChanged: win.qaDump("prompt")
        onClosed: { if (connectPanel.open) connectPanel.refocus(); else win.focusList() }
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
            if (prompt.mode === "tailnet-user") {
                win.tailnetUserEntered(text)
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
            if (prompt.mode === "unlock") {
                prompt.close()
                win.unlockEntered(text)
                return
            }

            let err = ""
            if (prompt.mode === "rename")
                err = dirModel.rename_row(prompt.targetRow, text)
            else if (prompt.mode === "newfolder")
                err = dirModel.new_folder(text)
            else if (prompt.mode === "newfile")
                err = dirModel.new_file(text)
            else if (prompt.mode === "owner") {
                err = dirModel.set_owner(text)
                if (err === "") { prompt.close(); win.qaDump("prompt-ok"); return }
            }

            if (err === "") {
                hints.message = prompt.mode === "rename"
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
        onJumpFirstChanged: win.qaDump("palette-results")
        onClosed: win.focusList()
        onNavigateRequested: (p) => { dirModel.navigate(p); win.focusList() }
        onActionRequested: (a) => win.runAction(a)
    }

    RecentPanel {
        id: recentPanel
        onOpenRequested: (path) => {
            const err = dirModel.open_path(path)
            recentPanel.error = err
            if (err === "") { hints.message = qsTr("Opened ") + path; recentPanel.hide() }
            win.qaDump("recent-open")
        }
        onRevealRequested: (path) => { recentPanel.hide(); dirModel.reveal(path); win.qaDump("recent-reveal") }
        onClosed: { win.focusList(); win.qaDump("recent-closed") }
    }

    ConnectPanel {
        id: connectPanel
        places: win.placesInfo
        onConnectRequested: (kind, name, target, user) => {
            win.connectPlace(kind, name, target, user)
        }
        onRefreshRequested: win.refreshPlaces()
        onOpenRequested: (path) => { connectPanel.hide(); dirModel.navigate(path) }
        onDisconnectRequested: (kind, name) => win.disconnectPlace(kind, name)
        onActionRequested: (a) => {
            if (a.startsWith("setup-install:")) win.installSupport(a.substring(14))
            else win.runAction(a)
        }
        onClosed: { win.focusList(); win.qaDump("connect-closed") }
        onVisibleChanged: win.qaDump("connect")
        onSelectionKeyChanged: win.qaDump("connect-select")
    }

    DeviceSharingPanel {
        id: deviceSharing
        onOpenRequested: (path) => { if (path) dirModel.navigate(path) }
        onClosed: { win.focusList(); win.qaDump("sharing-closed") }
        onOpenChanged: win.qaDump("sharing")
        onMessageChanged: win.qaDump("sharing-message")
        onBusyChanged: {
            if (!deviceSharing.busy && hints.message === qsTr("Transfer continues") && win.busyModels().length === 0)
                hints.message = ""
            win.qaDump("sharing-busy")
        }
        onCheckingChanged: win.qaDump("sharing-checking")
        onStateChanged: win.qaDump("sharing-state")
        onFocusedButtonChanged: win.qaDump("sharing-focus")
        onFolderConfirmationOpenChanged: win.qaDump("sharing-confirm")
        onHelpOpenChanged: win.qaDump("sharing-help")
    }

    TrashPanel {
        id: trashPanel
        onRestoreRequested: (name) => {
            const err = dirModel.trash_restore(name)
            trashPanel.error = err
            if (err === "") {
                hints.message = qsTr("Restored ") + name
                trashPanel.items = JSON.parse(dirModel.trash_list())
                trashPanel.current = Math.min(trashPanel.current, trashPanel.items.length - 1)
            }
            win.qaDump("trash-restore")
        }
        onShowFolder: (path) => { trashPanel.hide(); dirModel.navigate(path) }
        onClosed: { win.focusList(); win.qaDump("trash-closed") }
    }

    KeyMapPanel {
        id: keyMapPanel
        onClosed: { win.focusList(); win.qaDump("keymap-closed") }
    }

    SettingsPanel {
        id: settingsPanel
        onChanged: (key, value) => {
            const err = dirModel.settings_set(key, value)
            if (err !== "") win.alertText = err
            // New tabs honour hidden-files; the current one follows too,
            // since the user just asked for it.
            if (key === "show_hidden" && (value === "true") !== dirModel.show_hidden)
                dirModel.toggle_hidden()
            win.applySettings()
            settingsPanel.s = JSON.parse(dirModel.settings_json())
            win.qaDump("settings-changed")
        }
        onDialogsRequested: (on) => {
            const err = dirModel.set_file_dialogs(on)
            if (err !== "") win.alertText = err
            settingsPanel.s = JSON.parse(dirModel.settings_json())
            win.qaDump("settings-dialogs")
        }
        onKeysRequested: (on) => {
            const err = dirModel.set_omarchy_keys(on)
            if (err !== "") win.alertText = err
            settingsPanel.s = JSON.parse(dirModel.settings_json())
            win.qaDump("settings-keys")
        }
        onDefaultRequested: (on) => {
            const err = dirModel.set_default_file_manager(on)
            if (err !== "") win.alertText = err
            settingsPanel.s = JSON.parse(dirModel.settings_json())
            win.qaDump("settings-default")
        }
        onClosed: { win.focusList(); win.qaDump("settings-closed") }
    }

    QuickLook {
        id: quickLook
        model: dirModel
        onVisualChanged: win.qaDump("quicklook-visual")
        // Polled player state; dumped so qa can follow playback.
        onAudioChanged: win.qaDump("quicklook-audio")
        onHashChanged: win.qaDump("hash")
        onProgramChanged: win.qaDump("quicklook-program")
        onRunRequested: (row) => {
            quickLook.hide()
            if (row >= 0) win.askRun(row)
            else win.alertText = qsTr("That file is gone")
        }
        onClosed: win.focusList()
    }

    ContextMenu {
        id: contextMenu
        onChosen: (a) => win.runAction(a)
        onHighlightMoved: win.qaDump("context-menu")
        onClosed: { win.focusList(); win.qaDump("context-menu") }
    }

    PermissionsDialog {
        id: permissionsDialog
        objectName: "permissionsDialog"
        onOpenChanged: win.qaDump("permissions")
        onLoadingChanged: win.qaDump("permissions")
        onDraftChanged: win.qaDump("permissions")
        onErrorTextChanged: win.qaDump("permissions")
        onExplainingChanged: win.qaDump("permissions")
        // What changed and that Ctrl+Z puts it back, as other actions say
        // it, over the count of the very items it changed; items the system
        // refused must be read, so they alert.
        onFinished: (message, partial) => {
            if (partial) win.alertText = message
            else { hints.message = message; hints.messageAboutSelection = true }
            win.qaDump("permissions-done")
        }
        onClosed: win.focusList()
    }

    RenameCard {
        id: renameCard
        objectName: "renameCard"
        onVisibleChanged: win.qaDump("rename-card")
        onCheckChanged: win.qaDump("rename-check")
        onClosed: win.focusList()
        onAccepted: (names) => {
            const m = renameCard.model
            const r = renameCard.row
            renameCard.close()
            if (!m) return
            const err = m.bulk_apply_names(r, names)
            if (err !== "") win.alertText = err
            win.qaDump("bulk-done")
        }
    }

    ConflictDialog {
        id: conflictDialog
        objectName: "conflictDialog"
        onAnswered: (choice, all) => {
            const m = win.conflictModel
            if (!win.conflictOwnerLive(m) || m.conflict_json() === "") {
                win.conflictModel = null
                conflictDialog.close()
                return
            }
            const err = m.conflict_answer(choice, all)
            if (err === "?conflict") { win.showNextConflict(); return }
            win.conflictModel = null
            conflictDialog.close()
            if (err !== "") { win.alertText = err; win.qaDump("conflict-refused"); return }
            hints.message = qsTr("Started")
            jobTimer.start()
            win.qaDump("conflict-resolved")
        }
        onCancelled: {
            if (win.conflictOwnerLive(win.conflictModel)) win.conflictModel.conflict_cancel()
            win.conflictModel = null
            hints.message = qsTr("Cancelled — nothing was changed")
            win.qaDump("conflict-cancelled")
        }
        onClosed: {
            win.conflictModel = null
            win.focusList()
            // Run after close()/cancel() and their current handlers unwind.
            Qt.callLater(() => win.drainConflicts())
        }
    }

    ConfirmDialog {
        id: connectionRecovery
        preferCancel: true
        property string routeKey: ""
        onAccepted: {
            const key = routeKey
            Qt.callLater(() => connectPanel.showRoutes(key))
        }
        onClosed: win.focusList()
    }

    ConfirmDialog {
        id: confirm
        objectName: "confirmDialog"
        // What this confirmation is FOR. Two different questions must never
        // share one handler by accident.
        property string purpose: "trash"
        onVisibleChanged: win.qaDump("confirm")
        onClosed: { if (connectPanel.open) connectPanel.refocus(); else win.focusList() }
        onAccepted: {
            if (confirm.purpose === "run" || confirm.purpose === "allow-run") {
                const model = win.pendingRunModel
                const token = win.pendingRunToken
                win.pendingRunModel = null
                win.pendingRunToken = 0
                if (!model || token <= 0) return
                const err = model.run_confirm(token, confirm.purpose === "allow-run")
                // Said as a message: the reload after a permission change
                // puts "N items" back in the status before it can be read.
                if (err !== "") win.alertText = err
                else hints.message = model.status
                win.qaDump("run")
                return
            }
            if (confirm.purpose === "bulk") {
                win.bulkApply()
                return
            }
            if (confirm.purpose === "tailnetkey") {
                const t = win.pendingTailnet
                win.startTailnetAttempt(t, win.pendingTailnetUser, true)
                win.qaDump("tailnet-trusted")
                return
            }
            if (confirm.purpose === "clouddirty") {
                // The user was told exactly how many files are unfinished
                // and chose to go anyway. Only now is force allowed.
                const cn = win.pendingCloudOff
                win.pendingCloudOff = ""
                if (cn === "") return
                // A transfer may have started while the question was open.
                if (win.refuseBusyDisconnect("cloud", cn)) return
                const e = dirModel.cloud_disconnect(cn, true)
                if (e !== "") win.alertText = e
                return
            }
            if (confirm.purpose === "closetab") {
                const i = win.pendingCloseTab
                win.pendingCloseTab = -1
                if (i < 0) return
                const m = win.tabModelAt(i)
                if (!m) return
                // Archive work has no cancel; it is only waited for.
                if (m.job_running() || m.sharing_busy) m.cancel_job()
                // The tab stays, showing "Stopping", until its worker has
                // actually stopped — the same wait the window close makes.
                closeTabAfterCancel.wait(m)
                win.qaDump("tab-close-confirmed")
                return
            }
            if (confirm.purpose === "close") {
                // The user chose to abandon the transfers. Cancel every one
                // and close once each worker has actually stopped; whatever
                // cleanup it manages is its own business, not promised here.
                win.runningJobModels().forEach(m => m.cancel_job())
                win.closing = true
                win.qaDump("close-confirmed")
                closeAfterCancel.start()
                return
            }
            if (confirm.purpose === "closewait") {
                // The user chose not to wait for a stop that has not
                // finished. Their decision, made with the consequence shown.
                confirm.purpose = "trash"
                closeAfterCancel.stop()
                win.closeNow = true
                win.qaDump("close-without-waiting")
                Qt.quit()
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
            hints.message = err === ""
                ? dirModel.undo_label() + "  (Ctrl+Z)"
                : "Could not move to Trash: " + err
            win.qaDump("trashed")
        }
        onRejected: {
            if (confirm.purpose === "run" || confirm.purpose === "allow-run") {
                if (win.pendingRunModel) win.pendingRunModel.run_discard(win.pendingRunToken)
                win.pendingRunModel = null
                win.pendingRunToken = 0
                win.qaDump("run-cancelled")
                return
            }
            // Cancel means CANCEL. This handler used to carry a copy of the
            // accept branches, so cancelling "disconnect anyway?" force-
            // disconnected and cancelling "close this tab?" closed it —
            // exactly the opposite of what the button says.
            if (confirm.purpose === "clouddirty") {
                win.pendingCloudOff = ""
                win.qaDump("cloud-disconnect-cancelled")
                return
            }
            if (confirm.purpose === "tailnetkey") {
                // Cancel: nothing recorded, nothing connected.
                win.pendingTailnet = null
                connectPanel.showFeedback(qsTr("Not connected"), qsTr("Nothing was trusted."), false)
                hints.message = qsTr("Not connected")
                win.qaDump("tailnet-key-cancelled")
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
                hints.message = qsTr("Transfer continues")
                win.qaDump("tab-close-cancelled")
                return
            }
            if (confirm.purpose === "close") {
                confirm.purpose = "trash"
                hints.message = qsTr("Transfer continues")
                win.qaDump("close-cancelled")
                win.focusList()
                return
            }
            if (confirm.purpose === "closewait") {
                confirm.purpose = "trash"
                hints.message = qsTr("Still stopping…")
                win.qaDump("close-keep-waiting")
                win.focusList()
                return
            }
            hints.message = "Cancelled — nothing was changed"
            win.pendingTrashRow = -1
            win.focusList()
        }
    }

    // QA state probe. Enabled ONLY when GUST_QA_STATE names a file, so an
    // ordinary run writes nothing and has no extra surface. The harness reads
    // this instead of guessing from OCR.
    property string qaStatePath: ""
    property int qaTick: 0

    // Geometry in the probe (cursorPoint) is read when the dump is written;
    // a cursor move scrolls the list AFTER that. One settle dump shortly
    // after the last one reports where the row really ended up. QA only.
    Timer { id: qaSettle; interval: 300; onTriggered: win.qaDump("settle") }

    function qaDump(tag) {
        if (qaStatePath === "") return
        if (tag !== "settle") qaSettle.restart()
        const payload = {
            tag: tag,
            view: win.viewMode,
            // A file is being dragged inside the window (qa).
            dragging: (function () {
                const it = win.activePaneItem()
                const v = it && it.currentView ? it.currentView() : null
                return v ? !!v.dragActive : false
            })(),
            // The row ringed for a drag from another window, -1 if none (qa).
            dropRing: (function () {
                const it = win.activePaneItem()
                const v = it && it.currentView ? it.currentView() : null
                return v && v.platformDrop ? v.dropRow : -1
            })(),
            // The name riding under the pointer during a drag, "" if none (qa).
            dragGhost: (function () {
                const it = win.activePaneItem()
                return it && it.ghostName ? it.ghostName() : ""
            })(),
            preset: win.keyPreset,
            filterBarActive: filterBar.filterActive,
            searchBarActive: filterBar.searchActive,
            searchText: searchInput.text,
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
            activePaneModelMatches: (function () {
                const it = win.activePaneItem()
                return !!it && it.model === win.dirModel
            })(),
            presentedPaneLocations: (function () {
                const out = []
                for (let p = 0; p < win.paneCount; p++) {
                    const it = p === 0 ? paneA : paneBLoader.item
                    out.push(it && it.model ? it.model.location : "")
                }
                return out
            })(),
            paneModelBindingsMatch: (function () {
                const out = []
                for (let p = 0; p < win.paneCount; p++) {
                    const it = p === 0 ? paneA : paneBLoader.item
                    out.push(!!it && it.model === win.paneModel(p))
                }
                return out
            })(),
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
            quickLookProgram: quickLook.open ? quickLook.program : "",
            quickLookHash: quickLook.hash,
            keyMapOpen: keyMapPanel.open,
            breadcrumbText: crumbs.shownText(),
            rowHeight: Tokens.rowHeight,
            // Centre of the cursor row in window coordinates, so a mouse
            // case can click a row without guessing pixel offsets.
            cursorPoint: (function () {
                const it = win.activePaneItem()
                if (!it || !it.cursorAnchor) return null
                const p = it.cursorAnchor()
                return [Math.round(p.x + Tokens.rowPaddingX * 4), Math.round(p.y - Tokens.rowHeight / 2)]
            })(),
            nameHeaderPoint: (function () {
                const it = win.activePaneItem()
                if (!it || !it.nameHeaderPoint) return null
                const p = it.nameHeaderPoint()
                return p ? [Math.round(p.x), Math.round(p.y)] : null
            })(),
            headerTexts: (function () {
                const it = win.activePaneItem()
                return it && it.headerTexts ? it.headerTexts() : []
            })(),
            keyMapRows: keyMapPanel.open ? keyMapPanel.rows.length : 0,
            zoom: Tokens.zoom,
            adminMode: win.adminMode,
            hintsCut: hints.cutHints(),
            quickLookCloseInside: quickLook.closeInside(),
            // How far the hint strip's bottom edge sits past the window's
            // (0 when it is fully inside; > 0 means it was pushed off).
            hintsOverflow: Math.round(hints.mapToItem(null, 0, hints.height).y - win.height),
            jobPaused: win.job ? !!win.job.paused : false,
            jobPausePoint: jobDrawer.pausePoint(),
            menuPoints: contextMenu.open ? contextMenu.itemPoints() : ({}),
            placePoints: sidebar.placePoints(),
            crumbPoints: crumbs.crumbPoints(),
            rowTags: (function () {
                const out = {}
                if (!dirModel) return out
                for (let i = 0; i < Math.min(dirModel.count, 60); i++) {
                    const t = dirModel.entry_tags(i)
                    if (t !== "") out[dirModel.entry_name(i)] = t
                }
                return out
            })(),
            rowPoints: (function () {
                const it = win.activePaneItem()
                return it && it.rowPoints ? it.rowPoints() : ({})
            })(),
            activeTabPath: win.dirModel ? win.dirModel.location : "",
            quickLookName: quickLook.info ? quickLook.info.name : "",
            quickLookKind: quickLook.info ? quickLook.info.kindWords : "",
            quickLookLines: quickLook.info && quickLook.info.lines
                            ? quickLook.info.lines.length : 0,
            quickLookNoPreview: quickLook.info ? quickLook.info.noPreview : "",
            quickLookFacts: quickLook.info && quickLook.info.facts ? quickLook.info.facts : [],
            quickLookImage: quickLook.open ? quickLook.imageInfo() : null,
            quickLookVisual: quickLook.visual ? (quickLook.visual.image ? "image" : quickLook.visual.archive ? "archive:" + quickLook.visual.total
                             : quickLook.visual.markdown !== undefined ? "markdown"
                             : quickLook.visual.audio !== undefined ? "audio:" + quickLook.visual.audio : "why:" + quickLook.visual.why) : "",
            quickLookPage: [quickLook.page, quickLook.pages],
            quickLookVideo: quickLook.visual !== null && quickLook.visual.video === true,
            quickLookMarkdown: quickLook.visual && quickLook.visual.markdown !== undefined ? quickLook.visual.markdown : "",
            quickLookAudio: quickLook.open ? dirModel.audio_status() : "",
            audioBarPoint: quickLook.open ? quickLook.audioBarPoint() : null,
            tabDropZone: win.tabDropZone,
            paletteOpen: palette.visible,
            paletteResults: palette.visible ? palette.resultLabels() : [],
            paletteText: palette.visible ? palette.inputText() : "",
            promptOpen: prompt.visible,
            promptMode: prompt.mode,
            promptError: prompt.errorText,
            promptHint: prompt.hint,
            promptTitle: prompt.title,
            clip: dirModel.clip_summary(),
            selCount: dirModel.sel_count(),
            selSummary: dirModel.sel_summary(),
            hintShown: hints.displayText,
            hintKeys: hints.shownKeys,
            alertText: win.alertText,
            jobRunning: dirModel.job_running(),
            clipboardReading: dirModel.clipboard_busy(),
            jobsRunningAllTabs: win.runningJobModels().length,
            jobState: win.job ? win.job.state : "",
            jobStopping: win.job ? !!win.job.stopping : false,
            jobMessage: win.job ? win.job.message : "",
            jobRefused: win.job ? win.job.refused : 0,
            menuOpen: contextMenu.open,
            menuCurrent: contextMenu.current >= 0 && contextMenu.items[contextMenu.current]
                         ? contextMenu.items[contextMenu.current].label : "",
            menuItems: contextMenu.open
                       ? contextMenu.items.map(it => it.separator ? "-" : (it.enabled === false ? "(" + it.label + ")" : it.label))
                       : [],
            confirmOpen: confirm.visible,
            conflictOpen: conflictDialog.visible,
            live: dirModel.is_live(),
            emptyReason: dirModel.empty_reason,
            theme: Tokens.themeName,
            renderer: win.GraphicsInfo.api === GraphicsInfo.Software ? "software" : "hardware",
            themeBackground: "" + Tokens.background,
            themeRevision: Tokens.tokens.revision,
            interrupted: interruptedNotice.items.map(i => i.text),
            liveHold: win.liveHold,
            canBack: win.canBack,
            paneView: win.activePane === 1 ? win.paneBView : win.paneAView,
            canForward: win.canForward,
            chooserMode: chooserBar.mode,
            chooserError: chooserBar.error,
            settingsOpen: settingsPanel.open,
            trashOpen: trashPanel.open,
            recentOpen: recentPanel.open,
            deleteTotal: dirModel.delete_total,
            recentItems: recentPanel.items.slice(0, 5).map(i => i.name + "|" + i.folder),
            trashItems: trashPanel.items.map(i => i.shown + "|" + i.from),
            trashError: trashPanel.error,
            settings: settingsPanel.s,
            keyPreset: win.keyPreset,
            sidebarVisible: win.sidebarVisible,
            density: Tokens.density,
            bookmarks: win.bookmarkList.map(b => b.label + "=" + b.path),
            jobState: win.job ? win.job.state : "",
            jobBytes: win.job ? (win.job.bytes_text || "") : "",
            jobSpeed: win.job ? (win.job.speed || "") : "",
            jobEta: win.job ? (win.job.eta || "") : "",
            conflictName: conflictDialog.clash ? conflictDialog.clash.name : "",
            permissionsOpen: permissionsDialog.open,
            permissions: permissionsDialog.open ? permissionsDialog.qaState() : null,
            permissionsPoints: permissionsDialog.open && permissionsDialog.ready ? permissionsDialog.points() : null,
            renameCardOpen: renameCard.open,
            renameCardBad: renameCard.open ? renameCard.badLines() : 0,
            renameCardOk: renameCard.open ? renameCard.check.ok : false,
            renameCardSummary: renameCard.open ? renameCard.check.summary : "",
            renameCardPoints: renameCard.open ? renameCard.buttonPoints() : ({}),
            conflictCanMerge: !!(conflictDialog.clash && conflictDialog.clash.canMerge),
            conflictFocus: conflictDialog.focusedChoice(),
            conflictPoints: conflictDialog.visible ? conflictDialog.buttonPoints() : ({}),
            devices: win.deviceList.map((x) => x.label + "|" + x.state),
            connectedPlaces: win.connectedPlaces.map(p => p.name),
            lostPlaces: win.lostPlaces,
            connectionRecoveryOpen: connectionRecovery.visible,
            connectionRecoveryButtons: connectionRecovery.buttonCenters(),
            connectRouteKey: connectPanel.routeKey,
            connectRows: connectPanel.rows,
            connectSelection: connectPanel.selectionKey,
            connectOpen: connectPanel.open,
            connectFeedbackTitle: connectPanel.statusTitle,
            connectFeedbackBody: connectPanel.statusBody,
            connectBusy: connectPanel.busy,
            connectPoints: connectPanel.open ? connectPanel.buttonPoints() : ({}),
            sharingOpen: deviceSharing.open,
            sharingBusy: deviceSharing.busy,
            sharingChecking: deviceSharing.checking,
            sharingMessage: deviceSharing.message,
            sharingFailed: deviceSharing.failed,
            sharingState: deviceSharing.state,
            sharingPoints: deviceSharing.open ? deviceSharing.buttonPoints() : ({}),
            sharingConfirmOpen: deviceSharing.open && deviceSharing.folderConfirmationOpen,
            sharingConfirmTitle: deviceSharing.folderConfirmationTitle,
            sharingConfirmBody: deviceSharing.folderConfirmationBody,
            sharingFocus: deviceSharing.focusedButton,
            sharingHelpOpen: deviceSharing.helpOpen,
            sharingOnly: deviceSharing.only,
            sharingTitle: deviceSharing.preferredDevice,
            sharingLastAction: deviceSharing.lastAction,
            sharingInboxButton: deviceSharing.open && deviceSharing.inboxTouched,
            windowWidth: win.width,
            windowHeight: win.height,
            places: win.placesInfo,
            tabCenters: win.tabCenters(),
            confirmTitle: confirm.title,
            confirmBody: confirm.body,
            confirmButtons: confirm.visible ? confirm.buttonCenters() : null,
            confirmIsDangerous: confirm.dangerous,
            hintStatus: hints.message !== "" ? hints.message : hints.status,
            hintAlert: hints.alert,
            notice: dirModel.notice,
            undoLabel: dirModel.undo_label(),
            // Which item holds the keyboard: "listFocus" is the window's key
            // handler; anything else means arrow keys go somewhere else.
            focusOwner: win.activeFocusItem
                        ? (win.activeFocusItem.objectName !== "" ? win.activeFocusItem.objectName
                                                                 : String(win.activeFocusItem))
                        : "",
            listScroll: (function () {
                const it = win.activePaneItem()
                return it && it.scrollTop ? it.scrollTop() : -1
            })(),
            nerdGlyphs: Tokens.nerdGlyphs,
            icons: (function () {
                const it = win.activePaneItem()
                return it && it.iconInfo ? it.iconInfo() : []
            })(),
            sortButtonPoint: (function () {
                const p = sortButton.mapToItem(null, sortButton.width / 2, sortButton.height / 2)
                return [Math.round(p.x), Math.round(p.y)]
            })(),
            sortButtonText: sortText.text,
            model: JSON.parse(dirModel.state_json())
        }
        qaWriter.write(qaStatePath, JSON.stringify(payload, null, 2))
    }

    // A cancelled job must stop and clean up its staging file before its
    // tab disappears: the tab's model owns the job, so removing it early
    // would drop the work while a slow drive or remote call still holds
    // files open. No deadline — the stop takes as long as the call does.
    // Tracked by model, not index, because tabs can open and close while
    // this waits.
    Timer {
        id: closeTabAfterCancel
        interval: 150
        repeat: true
        property var models: []
        function wait(m) {
            if (models.indexOf(m) < 0) models = models.concat([m])
            start()
        }
        onTriggered: {
            // Avoid per-tick closures here: Qt 6.4's ARM64 JIT can leave
            // invalid GC roots while creating their call contexts.
            const done = []
            const pending = []
            for (let i = 0; i < models.length; i++) {
                const m = models[i]
                if (!m.job_running() && !m.archive_running()
                        && !m.permissions_running() && !m.sharing_busy)
                    done.push(m)
                else
                    pending.push(m)
            }
            models = pending
            if (models.length === 0) stop()
            // Off the waiting list first, so closeTab removes it for real.
            for (const m of done) {
                for (let i = 0; i < tabList.count; i++) {
                    if (win.tabModelAt(i) === m) { win.closeTab(i); break }
                }
            }
        }
    }

    QaWriter { id: qaWriter }

    // Anything that can change what the user sees re-dumps the state.
    Connections {
        target: dirModel
        function onLocationChanged() {
            if (hints.message.startsWith("Selected ") || hints.message.startsWith("No file starting"))
                hints.message = ""
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
        function onMeta_revChanged() { win.qaDump("row-details") }
        function onShow_hiddenChanged() { win.qaDump("hidden") }
    }

    // A theme switch repaints everything by itself; record it for QA.
    Connections {
        target: Tokens.tokens
        function onRevisionChanged() { win.qaDump("theme") }
    }

    Component.onCompleted: {
        // The model does not list anything until it is told to: construction
        // must not do I/O. This is the one explicit kick at startup — or the
        // first restored tab, when Settings asks for last time's tabs.
        if (!win.restoreSession()) dirModel.reload()
        // After a crash, say what was interrupted — read once, here.
        try { interruptedNotice.items = JSON.parse(dirModel.interrupted_jobs()) } catch (e) {}
        // The folder Gust opens with gets its remembered view too.
        applyRememberedView()
        // A local read of the kernel's own view: no daemon, no network.
        dirModel.request_devices()
        dirModel.watch_devices()
        win.refreshPlaces()
        win.refreshBookmarks()
        win.applySettings()
        chooserBar.mode = dirModel.chooser_mode()
        if (chooserBar.active) {
            chooserBar.title = dirModel.chooser_title()
            chooserBar.name = dirModel.chooser_name()
            win.title = Qt.binding(() => (chooserBar.title || qsTr("Choose")) + " — Gust")
            if (chooserBar.mode === "save") Qt.callLater(chooserBar.focusName)
        }
        if (win.defaultShowHidden && !dirModel.show_hidden) dirModel.toggle_hidden()
        qaStatePath = qaWriter.env_path("GUST_QA_STATE")
        // Started by a tab detach: take the view it had, then tell the
        // window it came from that this one is on screen.
        const hv = dirModel.handoff_view()
        if (hv !== "") win.setView(hv)
        Qt.callLater(() => dirModel.signal_ready())
        console.log("GUST_READY location=" + dirModel.location
                    + " count=" + dirModel.count
                    + " theme=" + Tokens.themeName)
        win.focusList()
        qaDump("ready")
    }
}
