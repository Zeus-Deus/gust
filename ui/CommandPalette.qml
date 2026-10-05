import QtQuick
import com.gust.app

// The `:` palette. Filters its actions correctly and shows the scope of a
// search, so a find never quietly widens beyond the current folder.
Rectangle {
    id: root
    property var model
    signal navigateRequested(string path)
    signal actionRequested(string action)
    /// Emitted whenever the palette stops being visible, so the window can put
    /// keyboard focus back on the file list. Without this the focus stays in
    /// the hidden TextInput and every later key press is swallowed.
    signal closed()

    anchors.fill: parent
    color: Tokens.withAlpha(Tokens.background, 0.6)
    visible: false
    z: 90

    /// True when opened with Ctrl+L: folders come before actions.
    property bool jumpFirst: false

    function open() {
        jumpFirst = false
        visible = true
        input.text = ""
        input.forceActiveFocus()
        if (root.model) root.model.jump_prepare()
        rebuildAndAnnounce("")
    }
    /// Ctrl+L: jump to a folder by a few letters, or type a path (Tab
    /// completes it).
    function openJump() {
        open()
        jumpFirst = true
        rebuildAndAnnounce("")
    }

    Connections {
        target: root.model
        function onJump_revChanged() { if (root.visible) root.rebuildAndAnnounce(input.text) }
        function onJump_completionChanged() {
            const s = root.model.jump_completion
            const nl = s.indexOf("\n")
            if (nl < 0) return
            // Only if the box still holds what was asked about.
            if (s.substring(0, nl) === input.text && s.length > nl + 1) {
                input.text = s.substring(nl + 1)
                input.cursorPosition = input.text.length
            }
        }
    }

    /// Folders matching `t`, best first, as palette rows.
    function jumpRows(t, limit) {
        if (!root.model || t.length === 0) return []
        let hits = []
        try { hits = JSON.parse(root.model.jump_query(t, limit)) } catch (e) { hits = [] }
        return hits.map(h => ({ kind: "path", label: "Go to " + h.shown, payload: h.path,
                                hint: h.source }))
    }
    /// Open with a query already typed, for a key that means one section of
    /// the palette — "o" for open-with, say.
    function openWith(query) {
        visible = true
        input.text = query
        input.forceActiveFocus()
        input.cursorPosition = query.length
        rebuildAndAnnounce(query)
    }
    function close() {
        visible = false
        input.text = ""
        // Give focus back before anyone can type again.
        closed()
    }

    readonly property var allActions: [
        { id: "bulk",         label: "Bulk rename (selection)", hint: "" },
        { id: "compress",     label: "Compress to .tar.zst",  hint: "" },
        { id: "extract",      label: "Extract here",          hint: "" },
        { id: "split",        label: "Split: two panes",      hint: "" },
        { id: "unsplit",      label: "Split: one pane",       hint: "" },
        { id: "send-copy",    label: "Copy to other pane",    hint: "F5" },
        { id: "send-move",    label: "Move to other pane",    hint: "F6" },
        { id: "view-rows",    label: "View: Rows",            hint: "" },
        { id: "sort-name",    label: "Sort by name",          hint: "" },
        { id: "sort-kind",    label: "Sort by kind",          hint: "" },
        { id: "sort-size",    label: "Sort by size",          hint: "" },
        { id: "sort-modified",label: "Sort by date",          hint: "" },
        { id: "view-grid",    label: "View: Grid",            hint: "" },
        { id: "view-columns", label: "View: Columns",         hint: "" },
        { id: "toggle-hidden",label: "Toggle hidden files",   hint: "Ctrl+H" },
        { id: "new-folder",   label: "New folder",            hint: "Ctrl+N" },
        { id: "new-file",     label: "New file",              hint: "Ctrl+Shift+N" },
        { id: "make-link",    label: "Make link",             hint: "" },
        { id: "run",          label: "Run program…",          hint: "" },
        { id: "permissions",  label: "Permissions…",          hint: "" },
        { id: "owner",        label: "Owner…",                hint: "" },
        { id: "paste-image",  label: "Paste image as file",   hint: "" },
        { id: "open-admin",   label: "Open as administrator", hint: "" },
        { id: "invert-selection", label: "Invert selection",  hint: "Ctrl+I" },
        { id: "deselect-all", label: "Select nothing",        hint: "Ctrl+Shift+A" },
        { id: "rename",       label: "Rename",                hint: "F2" },
        { id: "duplicate",    label: "Duplicate",             hint: "Ctrl+D" },
        { id: "copy",         label: "Copy",                  hint: "Ctrl+C" },
        { id: "cut",          label: "Cut",                   hint: "Ctrl+X" },
        { id: "paste",        label: "Paste here",            hint: "Ctrl+V" },
        { id: "terminal",     label: "Open terminal here",    hint: "" },
        { id: "trash",        label: "Move to Trash",         hint: "Delete" },
        { id: "reload",       label: "Reload this folder",    hint: "Ctrl+R" },
        { id: "up",           label: "Go up one folder",      hint: "" },
        { id: "keys",         label: "Show all keys",         hint: "?" },
        { id: "settings",     label: "Settings",              hint: "Ctrl+," },
        { id: "trash-view",   label: "Open the Trash",        hint: "" },
        { id: "recent",       label: "Recent files",          hint: "" },
        { id: "connect",      label: "Connect to a computer or cloud…", hint: "" },
        { id: "pin-here",     label: "Add this folder to Bookmarks", hint: "Ctrl+B" }
    ]

    ListModel { id: results }

    /// Emitted whenever the offered list changes, so the window can dump QA
    /// state at the moment the user would be reading it.
    signal resultsChanged()

    /// What the palette is offering, for tests and for the QA state dump.
    /// Reading the app's own answer beats reading pixels: the row below is
    /// low-contrast by design and OCR misses it.
    function inputText() { return input.text }
    function resultLabels() {
        const out = []
        for (let i = 0; i < results.count; i++) out.push(results.get(i).label)
        return out
    }

    /// Rebuild the offered list, then announce it. `rebuild` has an early
    /// return per branch, so the signal is emitted here rather than at the
    /// end of a function that usually does not reach its end.
    function rebuildAndAnnounce(text) {
        rebuild(text)
        root.resultsChanged()
    }

    function rebuild(text) {
        results.clear()
        const t = text.trim()

        // Remote places the user's config mentions. Listing them opens no
        // connection — "connect" is always a separate, deliberate choice.
        if (t.toLowerCase().startsWith("connect") || t.toLowerCase().startsWith("remote")) {
            let hosts = []
            try { hosts = JSON.parse(root.model.remote_suggestions()) } catch (e) { hosts = [] }
            for (let i = 0; i < hosts.length; i++) {
                const h = hosts[i]
                results.append({ kind: "action",
                                 label: (h.connected ? "Disconnect from " : "Connect to ")
                                        + h.alias + "  (" + h.host + ")"
                                        + (h.proxy ? " · via a jump host" : "")
                                        + (h.lost ? " · connection lost" : ""),
                                 payload: (h.connected ? "remote-off:" : "remote-on:") + h.alias })
            }
            // The full list: Tailscale devices, computers and cloud storage.
            results.append({ kind: "action",
                             label: "Connect… (computers, Tailscale devices, cloud storage)",
                             payload: "connect" })
            return
        }

        // Cloud remotes the INSTALLED rclone has configured. Reading its
        // config opens no socket, exactly as SSH discovery does not.
        if (t.toLowerCase().startsWith("cloud")) {
            let remotes = []
            try { remotes = JSON.parse(root.model.cloud_suggestions()) } catch (e) { remotes = [] }
            // rclone missing or locked: say why, and do not pretend the
            // user simply has no cloud storage.
            if (remotes.length === 1 && remotes[0].unavailable) {
                results.append({ kind: "action", label: remotes[0].why, payload: "noop" })
                return
            }
            if (remotes.length === 0) {
                results.append({ kind: "action",
                                 label: "No cloud storage configured — set one up with rclone",
                                 payload: "cloud-setup" })
                return
            }
            for (let i = 0; i < remotes.length; i++) {
                const c = remotes[i]
                results.append({ kind: "action",
                                 label: (c.connected ? "Disconnect from " : "Connect to ")
                                        + c.name + "  (" + c.backend + ")",
                                 payload: (c.connected ? "cloud-off:" : "cloud-on:") + c.name })
            }
            results.append({ kind: "action",
                             label: "Add cloud storage (opens rclone config)",
                             payload: "cloud-setup" })
            return
        }

        // "Open with" lists what the SYSTEM says can open this file. Not a
        // command line: Gust never runs something the user typed.
        if (t.toLowerCase().startsWith("open with") || t.toLowerCase() === "open") {
            let apps = []
            try { apps = JSON.parse(root.model.open_with_list(root.model.cursor)) }
            catch (e) { apps = [] }
            if (apps.length === 0) {
                results.append({ kind: "action",
                                 label: "No application is registered for this file",
                                 payload: "noop" })
                return
            }
            for (let i = 0; i < apps.length; i++) {
                const a = apps[i]
                results.append({ kind: "action",
                                 label: "Open with " + a.name
                                        + (a.default ? "  (default)" : ""),
                                 payload: "openwith:" + a.id })
            }
            return
        }

        // A shared folder is asked for, never guessed: discovery can be
        // unavailable or incomplete (plan §3).
        if (t.toLowerCase().startsWith("share") || t.toLowerCase().startsWith("nas")
            || t.toLowerCase().startsWith("smb")) {
            results.append({ kind: "action",
                             label: "Add a shared folder (SMB)",
                             payload: "share-add" })
            return
        }

        // Devices the kernel reports. Listing them mounts nothing.
        if (t.toLowerCase().startsWith("device") || t.toLowerCase().startsWith("eject")) {
            let devs = []
            try { devs = JSON.parse(root.model.devices_json()) } catch (e) { devs = [] }
            if (devs.length === 0) {
                results.append({ kind: "action",
                                 label: "No removable devices attached",
                                 payload: "noop" })
                return
            }
            for (let i = 0; i < devs.length; i++) {
                const dv = devs[i]
                if (dv.empty) {
                    results.append({ kind: "action",
                                     label: dv.label + " — " + dv.state,
                                     payload: "noop" })
                } else if (dv.mounted) {
                    results.append({ kind: "action",
                                     label: "Eject " + dv.label,
                                     payload: "device-eject:" + dv.path })
                } else {
                    results.append({ kind: "action",
                                     label: "Mount " + dv.label,
                                     payload: "device-mount:" + dv.path })
                }
            }
            return
        }

        // "find NAME" searches this folder AND BELOW — a different, explicit
        // scope from the filter, which only ever touches the current folder.
        if (t.toLowerCase().startsWith("find ")) {
            const needle = t.substring(5).trim()
            if (needle.length > 0) {
                results.append({ kind: "action",
                                 label: "Find \"" + needle + "\" in this folder and below",
                                 payload: "find:" + needle })
                return
            }
        }

        // "grep NEEDLE" searches what is INSIDE files, not their names.
        // Explicitly scoped and cancellable, like find (DESIGN §6.10).
        if (t.toLowerCase().startsWith("grep ")) {
            const inside = t.substring(5).trim()
            if (inside.length > 0) {
                results.append({ kind: "action",
                                 label: "Search inside files for \"" + inside
                                        + "\" in this folder and below",
                                 payload: "grep:" + inside })
                return
            }
        }

        // A leading / or ~ is a path, not an action.
        if (t.startsWith("/") || t.startsWith("~")) {
            results.append({ kind: "path", label: "Go to " + t, payload: t,
                             hint: "Enter" })
            return
        }
        const lower = t.toLowerCase()
        const folders = jumpRows(t, root.jumpFirst ? 6 : 3)
        if (root.jumpFirst) for (const f of folders) results.append(f)
        for (var i = 0; i < allActions.length; i++) {
            const a = allActions[i]
            if (lower === "" || a.label.toLowerCase().indexOf(lower) !== -1)
                results.append({ kind: "action", label: a.label, payload: a.id,
                                 hint: a.hint })
        }
        // The user's own actions that fit the selection.
        let custom = []
        try { custom = JSON.parse(root.model.custom_actions_json()).actions } catch (e) { custom = [] }
        for (const c of custom)
            if (lower === "" || c.name.toLowerCase().indexOf(lower) !== -1)
                results.append({ kind: "action", label: c.name, payload: "custom:" + c.id, hint: "action" })
        if (!root.jumpFirst) for (const f of folders) results.append(f)
        // Scope is stated explicitly — never an implicit recursive crawl.
        if (t.length > 0) {
            results.append({
                kind: "action",
                label: "Filter this folder for \"" + t + "\"",
                payload: "filter:" + t,
                hint: "current folder only"
            })
        }
    }

    MouseArea {
        anchors.fill: parent
        onClicked: root.close()
    }

    Rectangle {
        anchors.horizontalCenter: parent.horizontalCenter
        y: Math.round(parent.height * 0.16)
        width: Math.min(parent.width * 0.7, Math.round(520 * Tokens.spacingScale))
        height: box.implicitHeight + Tokens.spaceXl * 2
        color: Tokens.menuBackground
        border.width: Math.max(1, Tokens.borderWidth)
        border.color: Tokens.border
        radius: Tokens.radius

        MouseArea { anchors.fill: parent }

        Column {
            id: box
            anchors.centerIn: parent
            width: parent.width - Tokens.spaceXl * 2
            spacing: Tokens.spaceLg

            Row {
                spacing: Tokens.spaceLg
                width: parent.width
                Text {
                    text: ":"
                    color: Tokens.accent
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontSubtitle
                }
                TextInput {
                    id: input
                    objectName: "paletteInput"
                    width: parent.width - Tokens.spaceXxl
                    color: Tokens.foregroundBright
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontSubtitle
                    selectionColor: Tokens.withAlpha(Tokens.accent, 0.4)
                    selectByMouse: true
                    // A text field owns its keys: file shortcuts must not fire
                    // while typing here (plan §8).
                    onTextChanged: root.rebuildAndAnnounce(text)
                    Keys.onEscapePressed: root.close()
                    // Tab: complete a typed path like a shell; on a folder
                    // row, put its path in the box to keep going deeper.
                    Keys.onTabPressed: (event) => {
                        event.accepted = true
                        const t = input.text
                        if (t.startsWith("/") || t.startsWith("~")) {
                            root.model.jump_complete(t)
                            return
                        }
                        if (results.count > 0) {
                            const it = results.get(Math.max(0, list.currentIndex))
                            if (it.kind === "path") {
                                input.text = it.label.substring(6) + "/"
                                input.cursorPosition = input.text.length
                            }
                        }
                    }
                    Keys.onReturnPressed: root.accept()
                    Keys.onDownPressed: list.currentIndex = Math.min(list.currentIndex + 1, results.count - 1)
                    Keys.onUpPressed: list.currentIndex = Math.max(list.currentIndex - 1, 0)

                    Accessible.role: Accessible.EditableText
                    Accessible.name: "Command and path input"
                }
            }

            Rectangle {
                width: parent.width
                height: Tokens.borderWidth
                color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
            }

            ListView {
                id: list
                WheelScroll { view: list }
                width: parent.width
                height: Math.min(results.count, 8) * Tokens.rowHeight
                model: results
                clip: true
                currentIndex: 0
                highlightMoveDuration: 0

                delegate: Rectangle {
                    width: list.width
                    height: Tokens.rowHeight
                    color: index === list.currentIndex
                           ? Tokens.withAlpha(Tokens.selectedBackground, Math.max(Tokens.selectedAlpha, 0.16))
                           : "transparent"

                    Row {
                        anchors.fill: parent
                        anchors.leftMargin: Tokens.spaceLg
                        anchors.rightMargin: Tokens.spaceLg

                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            width: parent.width - hintText.implicitWidth - Tokens.spaceLg
                            text: model.label
                            color: index === list.currentIndex ? Tokens.selectedText : Tokens.foreground
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontBody
                            elide: Text.ElideRight
                        }
                        Text {
                            id: hintText
                            anchors.verticalCenter: parent.verticalCenter
                            text: model.hint
                            color: Tokens.foregroundDim
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontCaption
                        }
                    }

                    MouseArea {
                        anchors.fill: parent
                        onClicked: { list.currentIndex = index; root.accept() }
                    }

                    Accessible.role: Accessible.ListItem
                    Accessible.name: model.label + " " + model.hint
                }
            }
        }
    }

    function accept() {
        if (results.count === 0) return
        const item = results.get(Math.max(0, list.currentIndex))
        if (item.kind === "path") root.navigateRequested(item.payload)
        else root.actionRequested(item.payload)
        root.close()
    }

    Accessible.role: Accessible.Dialog
    Accessible.name: "Command palette"
}
