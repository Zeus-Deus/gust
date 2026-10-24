import QtQuick
import com.gust.app

// One sheet for sharing with phones, tablets and other devices (DESIGN §6.4.1).
// Each row names a direction from this computer's point of view: Send a copy
// (here → one device), Get files (any device → Gust Inbox), and Let devices
// open a folder (ongoing, policy-wide). Entry points on a file or folder show
// only their own row, so the same words mean the same thing everywhere.
Item {
    id: root
    anchors.fill: parent
    visible: open
    z: 85
    property bool open: false
    property var owner: null
    /// "" shows every row; "send" or "folder" shows only that job.
    property string only: ""
    property string selection: ""
    property string folderPath: ""
    property string folderToken: ""
    property bool folderProvided: false
    property string preferredDevice: ""
    property string message: ""
    property bool failed: false
    property bool stopping: false
    /// The row that started the current or last action owns its feedback.
    property string lastAction: ""
    property string lastTarget: ""
    property string receivedPath: ""
    property bool inboxTouched: false
    property bool helpOpen: false
    property string focusedButton: "closeDeviceSharing"
    readonly property bool folderConfirmationOpen: shareConfirm.visible
    readonly property string folderConfirmationTitle: shareConfirm.title
    readonly property string folderConfirmationBody: shareConfirm.body
    readonly property bool busy: !!owner && owner.sharing_busy
    readonly property bool checking: !!owner && owner.sharing_checking
    readonly property var state: {
        const raw = owner ? owner.sharing_json : "{}"
        try { return JSON.parse(raw) } catch (e) { return {} }
    }
    readonly property string sharedPath: state.shared_path || ""
    /// A folder someone chose, unless a different one is already open to
    /// devices: only one folder at a time, and the one that is on is shown.
    readonly property bool otherFolderOn: root.folderProvided && root.sharedPath !== ""
        && !!state.shared_token && state.shared_token !== root.folderToken
    readonly property string chosenFolder: state.shared_label || (root.folderProvided ? folderPath : state.default_label || "")
    readonly property string chosenToken: state.shared_token || (root.folderProvided ? folderToken : state.default_token || "")
    readonly property string computerName: state.computer_name || qsTr("this computer")
    readonly property var receivers: {
        // The device this sheet was opened for comes first; the others stay
        // available because Send eligibility is the account's, not the row's.
        const list = (state.targets || []).slice()
        return list.sort((a, b) => (b.name === root.preferredDevice) - (a.name === root.preferredDevice))
    }
    readonly property var preferredTarget: (state.targets || []).find(t => t.name === root.preferredDevice) || null
    signal closed()
    signal openRequested(string path)

    Keys.onTabPressed: (event) => { if (!shareConfirm.visible) root.cycleFocus(1); else event.accepted = false }
    Keys.onBacktabPressed: (event) => { if (!shareConfirm.visible) root.cycleFocus(-1); else event.accepted = false }
    Keys.onEscapePressed: if (!shareConfirm.visible) root.hide()

    function show(model, selected, folder, device, focusOnly) {
        if (root.busy) { root.open = true; root.refocus(); return }
        root.owner = model
        root.only = focusOnly || ""
        root.selection = selected || ""
        root.folderProvided = !!folder && typeof folder === "object"
        root.folderPath = root.folderProvided ? folder.path || "" : ""
        root.folderToken = root.folderProvided ? folder.token || "" : ""
        root.preferredDevice = device || ""
        root.message = ""; root.failed = false; root.stopping = false
        root.lastAction = ""; root.lastTarget = ""
        // A later visit starts clean: "Show files" without its result line
        // would not say which files.
        root.inboxTouched = false; root.receivedPath = ""
        root.helpOpen = false
        viewport.contentY = 0
        root.open = true
        root.owner.request_sharing()
        root.refocus()
    }
    function hide() { shareConfirm.close(); root.open = false; root.closed() }
    function refocus() { Qt.callLater(() => { if (root.open) { closeButton.forceActiveFocus(); root.focusedButton = closeButton.objectName } }) }
    function cycleFocus(direction) {
        const buttons = []
        function visit(item) {
            if (item.visible && item.enabled && item.activeFocusOnTab) buttons.push(item)
            // QML child lists are indexable, but not iterable on system Qt 6.4.
            const children = item.children || []
            for (let i = 0; i < children.length; i++) visit(children[i])
        }
        visit(card)
        if (buttons.length === 0) return
        const current = buttons.findIndex(b => b.activeFocus)
        const next = buttons[(current + direction + buttons.length) % buttons.length]
        next.forceActiveFocus()
        root.focusedButton = next.objectName
        if (!["closeDeviceSharing", "refreshDeviceSharing", "cancelDeviceSharing", "deviceSharingHelp"].includes(next.objectName)) {
            const p = next.mapToItem(contents, 0, 0)
            if (p.y < viewport.contentY) viewport.contentY = p.y
            else if (p.y + next.height > viewport.contentY + viewport.height)
                viewport.contentY = Math.max(0, p.y + next.height - viewport.height)
        }
    }
    function buttonPoints() {
        const points = {}
        function visit(item) {
            if (item.visible && item.objectName && (item.objectName.startsWith("sendTo:")
                || ["receiveDeviceFiles", "openDeviceInbox", "publishDeviceFolder", "closeDeviceSharing", "refreshDeviceSharing", "cancelDeviceSharing"].includes(item.objectName))) {
                const p = item.mapToItem(null, item.width / 2, item.height / 2)
                points[item.objectName] = [Math.round(p.x), Math.round(p.y)]
            }
            const children = item.children || []
            for (let i = 0; i < children.length; i++) visit(children[i])
        }
        visit(card)
        return points
    }
    function rowFor(action) { return action === "publish" || action === "unpublish" ? "folder" : action }
    function start(action, value) {
        if (!root.owner || root.busy || root.checking) return
        root.failed = false; root.stopping = false
        root.lastAction = action
        if (action === "receive") root.inboxTouched = true
        if (action === "send") root.lastTarget = (root.receivers.find(t => t.address === value) || {}).name || ""
        root.message = action === "send" ? qsTr("Sending to %1…").arg(root.lastTarget || qsTr("the device"))
            : action === "receive" ? qsTr("Checking for files…")
            : action === "publish" ? qsTr("Turning on…") : qsTr("Turning off…")
        const error = root.owner.device_share(action, value || "")
        if (error !== "") { root.failed = true; root.message = error }
    }
    function receivedSummary(message) {
        const count = parseInt(message, 10)
        return count === 1 ? qsTr("1 file saved in Gust Inbox.")
            : count > 1 ? qsTr("%1 files saved in Gust Inbox.").arg(count) : qsTr("Files saved in Gust Inbox.")
    }
    function askFolder() {
        if (root.busy || root.checking || root.chosenToken === "") return
        shareConfirm.action = root.sharedPath ? "unpublish" : "publish"
        shareConfirm.path = root.chosenToken
        shareConfirm.preferCancel = true
        shareConfirm.open(root.sharedPath ? qsTr("Turn off folder access?") : qsTr("Let devices open this folder?"),
            root.sharedPath ? qsTr("“%1”\n\nDevices can no longer open it. All files stay on this computer.").arg(root.chosenFolder)
                : qsTr("“%1”\n\nDevices your Tailscale settings allow can open, change and delete files in it. That can include other people on your network. Files stay on this computer.").arg(root.chosenFolder),
            root.sharedPath ? qsTr("Turn Off") : qsTr("Turn On"), false)
    }
    Connections {
        target: root.owner
        function onSharing_done(action, success, message, path) {
            root.failed = !success
            root.stopping = false
            root.lastAction = action
            const none = action === "receive" && message.startsWith("No files")
            root.message = !success ? message
                : action === "send" ? qsTr("Sent. Open Tailscale on %1 to save it.").arg(root.lastTarget || qsTr("the device"))
                : action === "receive" ? (none ? qsTr("Nothing waiting yet.") : root.receivedSummary(message))
                : action === "publish" ? qsTr("On. Access follows your Tailscale settings.") : qsTr("Off. All files stay here.")
            if (action === "receive") root.receivedPath = success && !none ? path : ""
        }
    }

    // A row: direction glyph, plain title, one line of what it does, then
    // its actions. The status line belongs to the row that acted.
    component SheetRow: Column {
        id: sheetRow
        property string glyph: ""
        property string title: ""
        property string badge: ""
        property bool badgeOn: false
        property string detail: ""
        /// Paths stay on one line and keep both ends visible.
        property bool detailIsPath: false
        property string hint: ""
        property string action: ""
        property bool hasActions: true
        default property alias actions: actionFlow.data
        width: parent ? parent.width : 0
        spacing: Tokens.spaceSm
        Item {
            width: parent.width
            height: Math.max(titleText.implicitHeight, glyphText.implicitHeight)
            Text {
                id: glyphText
                width: Tokens.spaceXxl * 2
                text: sheetRow.glyph
                textFormat: Text.PlainText
                color: Tokens.accent
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSubtitle
            }
            Text {
                id: titleText
                anchors.left: glyphText.right
                anchors.right: badgeText.left
                anchors.rightMargin: Tokens.spaceLg
                text: sheetRow.title
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                color: Tokens.foregroundBright
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSubtitle
            }
            Text {
                id: badgeText
                anchors.right: parent.right
                anchors.baseline: titleText.baseline
                text: sheetRow.badge
                textFormat: Text.PlainText
                color: sheetRow.badgeOn ? Tokens.accent : Tokens.foregroundDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
            }
        }
        Column {
            x: glyphText.width
            width: parent.width - x
            spacing: Tokens.spaceSm
            Text {
                width: parent.width
                visible: text !== ""
                text: sheetRow.detail
                textFormat: Text.PlainText
                wrapMode: sheetRow.detailIsPath ? Text.NoWrap : Text.Wrap
                elide: sheetRow.detailIsPath ? Text.ElideMiddle : Text.ElideNone
                color: Tokens.foreground
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
            }
            Text {
                width: parent.width
                visible: text !== ""
                text: sheetRow.hint
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                color: Tokens.foregroundDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
            }
            Text {
                objectName: "sharingStatus:" + sheetRow.action
                width: parent.width
                visible: root.message !== "" && root.rowFor(root.lastAction) === sheetRow.action
                text: root.message
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                color: root.failed ? Tokens.danger : Tokens.accent
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
            }
            Flow {
                id: actionFlow
                visible: sheetRow.hasActions
                width: parent.width
                spacing: Tokens.spaceSm
                topPadding: Tokens.spaceXs
            }
        }
    }
    component Divider: Rectangle {
        width: parent ? parent.width : 0
        height: Tokens.borderWidth
        color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
    }

    Rectangle {
        anchors.fill: parent
        color: Tokens.withAlpha(Tokens.background, 0.82)
        MouseArea { anchors.fill: parent; onClicked: root.hide() }
    }
    Rectangle {
        id: card
        objectName: "deviceSharingPanel"
        anchors.centerIn: parent
        width: Math.min(parent.width - Tokens.spaceLg * 2, Tokens.rowHeight * 22)
        height: Math.min(parent.height - Tokens.spaceLg * 2, contents.implicitHeight + footer.height + Tokens.spaceXl * 3)
        color: Tokens.backgroundDark
        border.width: Tokens.borderWidth
        border.color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
        MouseArea { anchors.fill: parent }
        Keys.onEscapePressed: root.hide()

        Flickable {
            id: viewport
            anchors.top: parent.top
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: footer.top
            anchors.margins: Tokens.spaceXl
            clip: true
            contentHeight: contents.implicitHeight
            boundsBehavior: Flickable.StopAtBounds
            WheelScroll { view: viewport }
            Column {
                id: contents
                width: viewport.width - Tokens.spaceSm
                spacing: Tokens.spaceLg
                Column {
                    width: parent.width
                    spacing: Tokens.spaceXs
                    Text {
                        objectName: "sharingTitle"
                        width: parent.width
                        text: root.preferredDevice || qsTr("Your devices")
                        textFormat: Text.PlainText
                        elide: Text.ElideRight
                        color: Tokens.foregroundBright
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontTitle
                    }
                    Text {
                        objectName: "sharingFeedback"
                        width: parent.width
                        text: root.checking ? qsTr("Checking devices…")
                            : root.preferredDevice && root.only === "" && root.preferredTarget && !root.preferredTarget.online
                              ? qsTr("Offline · open Tailscale on it, then check again")
                            : qsTr("Through Tailscale · this computer is %1").arg(root.computerName)
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                        color: Tokens.foregroundDim
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSmall
                    }
                }
                Divider {}
                SheetRow {
                    objectName: "sharingSendChoices"
                    action: "send"
                    visible: root.only === "" || root.only === "send"
                    glyph: "→"
                    title: qsTr("Send a copy")
                    detail: root.selection ? qsTr("%1 · the original stays here").arg(root.selection) : ""
                    detailIsPath: true
                    hasActions: root.selection !== ""
                    hint: !root.selection ? qsTr("Select files in Gust, then right-click → Send a copy to a device.")
                        : root.checking ? ""
                        : root.state.send_error ? root.state.send_error
                        : root.receivers.length === 0 ? qsTr("No devices can receive files right now. Open Tailscale on the device, then check again.")
                        : root.receivers.length > 1 ? qsTr("Choose where it goes.") : ""
                    Repeater {
                        model: root.selection ? root.receivers : []
                        SharingButton {
                            required property var modelData
                            onFocusStep: (direction) => root.cycleFocus(direction)
                            objectName: "sendTo:" + modelData.address
                            text: qsTr("Send to %1").arg(modelData.name) + (modelData.online ? "" : qsTr(" · offline"))
                            enabled: modelData.online && !root.busy && !root.checking
                            onClicked: root.start("send", modelData.address)
                        }
                    }
                }
                Divider { visible: root.only === "" }
                SheetRow {
                    objectName: "sharingReceiveActions"
                    action: "receive"
                    visible: root.only === ""
                    glyph: "←"
                    title: qsTr("Get files from your devices")
                    detail: qsTr("On the phone or tablet: Share → Tailscale → %1").arg(root.computerName)
                    hint: qsTr("Then check here. Files are saved in Gust Inbox.")
                    SharingButton {
                        objectName: "receiveDeviceFiles"
                        onFocusStep: (direction) => root.cycleFocus(direction)
                        text: qsTr("Check for files")
                        enabled: !root.busy && !root.checking
                        onClicked: root.start("receive", "")
                    }
                    SharingButton {
                        objectName: "openDeviceInbox"
                        onFocusStep: (direction) => root.cycleFocus(direction)
                        text: root.receivedPath ? qsTr("Show files") : qsTr("Open Gust Inbox")
                        visible: root.inboxTouched && !(root.lastAction === "receive" && !root.failed && !root.receivedPath)
                        enabled: !root.busy
                        onClicked: { root.openRequested(root.receivedPath || root.state.inbox_path); root.hide() }
                    }
                }
                Divider { visible: root.only === "" }
                SheetRow {
                    objectName: "sharingFolderActions"
                    action: "folder"
                    visible: root.only === "" || root.only === "folder"
                    glyph: "↔"
                    title: qsTr("Let devices open a folder")
                    badge: root.state.drive_error ? qsTr("Needs setup") : root.sharedPath ? qsTr("On") : root.checking ? "" : qsTr("Off")
                    badgeOn: !root.state.drive_error && root.sharedPath !== ""
                    detail: root.state.drive_error ? qsTr("Tailscale folder access (Taildrive) is not set up yet.") : root.chosenFolder
                    detailIsPath: !root.state.drive_error
                    hint: root.state.drive_error ? ""
                        : root.otherFolderOn ? qsTr("Only one folder at a time. Turn this one off to choose %1.").arg(root.folderPath.split("/").pop())
                        : root.sharedPath ? qsTr("In the Files app: Browse → Tailscale → your network → %1 → gust").arg(root.computerName)
                        : qsTr("Open and edit it from the Files app on your phone or tablet.")
                    SharingButton {
                        id: folderButton
                        objectName: "publishDeviceFolder"
                        onFocusStep: (direction) => root.cycleFocus(direction)
                        text: root.state.drive_error ? qsTr("Set Up…") : root.sharedPath ? qsTr("Turn Off…") : qsTr("Turn On…")
                        enabled: !root.busy && !root.checking && (!!root.state.drive_error || root.chosenToken !== "")
                        onClicked: {
                            if (root.state.drive_error) root.helpOpen = true
                            else root.askFolder()
                        }
                    }
                }
                Column {
                    objectName: "sharingHelp"
                    visible: root.helpOpen
                    width: parent.width
                    spacing: Tokens.spaceSm
                    Divider {}
                    Text {
                        width: parent.width
                        text: qsTr("Sending and getting files use Taildrop. Folder access uses Taildrive, a Tailscale preview that whoever runs your Tailscale network may need to turn on. Send folders as a compressed file.")
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                        color: Tokens.foregroundDim
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSmall
                    }
                    Text {
                        visible: text !== ""
                        width: parent.width
                        text: root.state.drive_error || ""
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                        color: Tokens.foregroundDim
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSmall
                    }
                    SharingButton {
                        objectName: "deviceTaildriveSetup"
                        onFocusStep: (direction) => root.cycleFocus(direction)
                        text: qsTr("Tailscale setup guide ↗")
                        onClicked: Qt.openUrlExternally("https://tailscale.com/docs/features/taildrive?tab=ios")
                    }
                }
            }
        }
        ScrollIndicator { target: viewport }
        Flow {
            id: footer
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            anchors.margins: Tokens.spaceXl
            spacing: Tokens.spaceSm
            SharingButton {
                id: closeButton
                onFocusStep: (direction) => root.cycleFocus(direction)
                objectName: "closeDeviceSharing"
                text: root.busy ? qsTr("Hide") : qsTr("Done")
                onClicked: root.hide()
            }
            SharingButton {
                objectName: "refreshDeviceSharing"
                onFocusStep: (direction) => root.cycleFocus(direction)
                text: qsTr("Check again")
                visible: !root.busy
                enabled: !root.checking
                onClicked: root.owner.request_sharing()
            }
            SharingButton {
                objectName: "deviceSharingHelp"
                onFocusStep: (direction) => root.cycleFocus(direction)
                text: root.helpOpen ? qsTr("Hide help") : qsTr("Help")
                onClicked: root.helpOpen = !root.helpOpen
            }
            SharingButton {
                objectName: "cancelDeviceSharing"
                onFocusStep: (direction) => root.cycleFocus(direction)
                text: root.stopping ? qsTr("Stopping…") : qsTr("Stop")
                visible: root.busy
                enabled: !root.stopping
                onClicked: {
                    root.stopping = true
                    root.message = qsTr("Stopping… Changes may already have been accepted.")
                    root.owner.cancel_device_share()
                }
            }
        }
    }
    ConfirmDialog {
        id: shareConfirm
        objectName: "confirmDeviceFolder"
        property string action: ""
        property string path: ""
        onAccepted: root.start(action, path)
        onClosed: root.refocus()
    }
}
