import QtQuick
import com.gust.app

// Permissions… (DESIGN §6.26): who may read, change and run the target, as
// nine checkboxes in plain words. Nothing is typed and no mode number is
// shown. The facts are read on a worker; until they arrive the card says so.
//
// Each checkbox shows what the items really have: on, off, or mixed when only
// some of a selection have it. Apply asks only for the bits the user changed,
// so everything else, special flags included, stays as each item has it.
FocusScope {
    id: root
    anchors.fill: parent
    visible: open
    z: 100

    property bool open: false
    /// The model whose targets were read, and the read's token.
    property var model: null
    property int token: 0
    /// The parsed `permissions_ready` facts, null while reading.
    property var facts: null
    property bool loading: false
    property bool busy: false
    property string errorText: ""
    property bool explaining: false
    /// Per bit, owner-read first: 0 none of the items has it, 1 some do,
    /// 2 all do. `draft` is what Apply will ask for; 1 only where `orig` is 1.
    property var orig: []
    property var draft: []

    /// The change went through: `message` says what changed and how to
    /// undo it; `partial` when some items were refused.
    signal finished(string message, bool partial)
    signal closed()

    readonly property bool ready: !!facts && facts.ok === true
    readonly property int total: ready ? facts.total : 0
    readonly property string kind: !ready ? ""
                                   : facts.dirs === 0 ? "file"
                                   : facts.files === 0 ? "folder" : "mixed"
    readonly property bool editable: ready && facts.owned > 0 && !busy
    readonly property int addMask: {
        let m = 0
        for (let i = 0; i < draft.length; i++)
            if (draft[i] === 2 && orig[i] !== 2) m |= (256 >> i)
        return m
    }
    readonly property int removeMask: {
        let m = 0
        for (let i = 0; i < draft.length; i++)
            if (draft[i] === 0 && orig[i] !== 0) m |= (256 >> i)
        return m
    }
    readonly property int changes: {
        let n = 0
        for (let i = 0; i < draft.length; i++)
            if (draft[i] !== orig[i]) n++
        return n
    }
    readonly property bool changed: addMask !== 0 || removeMask !== 0
    readonly property bool anyMixed: orig.indexOf(1) >= 0

    readonly property var permLabels: kind === "folder"
        ? [qsTr("List files"), qsTr("Add or remove"), qsTr("Access")]
        : kind === "mixed" ? [qsTr("Read"), qsTr("Change"), qsTr("Run / Access")]
        : [qsTr("Read"), qsTr("Edit"), qsTr("Run")]

    readonly property var audiences: {
        if (!ready) return []
        const o = facts.owner, g = facts.group
        const ownerSub = o.you ? (o.name ? qsTr("Owner · %1").arg(o.name) : qsTr("Owner"))
                       : !o.same ? qsTr("Each item's own owner")
                       : (o.name || qsTr("An account not named on this computer"))
        const groupSub = !g.same ? qsTr("Each item's own group")
                       : (g.name || qsTr("A group not named on this computer"))
                         + (g.member ? qsTr(" · includes you") : "")
        return [
            { title: o.you ? qsTr("You") : qsTr("Owner"), sub: ownerSub },
            { title: qsTr("Group"), sub: groupSub },
            { title: qsTr("Other users"), sub: qsTr("Everyone else on this computer") }
        ]
    }

    readonly property string subject: kind === "file" && total === 1 ? qsTr("this file")
                                    : kind === "folder" && total === 1 ? qsTr("this folder")
                                    : qsTr("these items")

    readonly property string countText: {
        if (!ready) return ""
        const f = facts.files, d = facts.dirs
        const files = f === 1 ? qsTr("1 file") : qsTr("%1 files").arg(f)
        const dirs = d === 1 ? qsTr("1 folder") : qsTr("%1 folders").arg(d)
        return d === 0 ? files : f === 0 ? dirs : qsTr("%1 and %2").arg(files).arg(dirs)
    }

    readonly property string context: {
        if (!ready) return ""
        const parts = [facts.name ? facts.name : countText]
        if (facts.place) parts.push(facts.place)
        return parts.join("  ·  ")
    }

    readonly property var definitions: {
        const file = [
            [qsTr("Read"), qsTr("Open and view the file.")],
            [qsTr("Edit"), qsTr("Change what is in the file. Renaming or deleting it is decided by its folder.")],
            [qsTr("Run"), qsTr("Start the file as a program or script. Allow it only for programs you trust.")]
        ]
        const folder = [
            [qsTr("List files"), qsTr("See the names of the items in the folder.")],
            [qsTr("Add or remove"), qsTr("Create, rename and delete items in the folder, even ones the person cannot edit. Works only together with Access.")],
            [qsTr("Access"), qsTr("Enter the folder and reach the items inside, as far as their own permissions allow.")]
        ]
        if (kind === "file") return file
        if (kind === "folder") return folder
        return [[qsTr("Files"), ""]].concat(file).concat([[qsTr("Folders"), ""]]).concat(folder)
    }

    /// What the card needs to say now, most important first.
    /// tone: "danger", "warn" or "" (plain).
    readonly property var notices: {
        const n = []
        if (errorText !== "") n.push({ tone: "danger", text: errorText })
        if (!ready) return n
        if (facts.owned === 0)
            n.push({ tone: "warn", text: qsTr("Only the owner or an administrator can change these permissions.") })
        else if (facts.owned < total)
            n.push({ tone: "warn", text: total - facts.owned === 1
                        ? qsTr("1 of these %1 items belongs to someone else; the system will refuse it.").arg(total)
                        : qsTr("%1 of these %2 items belong to someone else; the system will refuse those.")
                              .arg(total - facts.owned).arg(total) })
        if (draft[7] === 2 && orig[7] !== 2)
            n.push({ tone: "warn", text: kind === "folder"
                        ? qsTr("Other users will be able to add, rename and delete items in %1.").arg(subject)
                        : qsTr("Other users will be able to change %1.").arg(subject) })
        if (facts.owner.you && facts.dirs > 0 && ((draft[0] === 0 && orig[0] !== 0) || (draft[2] === 0 && orig[2] !== 0)))
            n.push({ tone: "warn", text: qsTr("You will not be able to open the folder until you allow it again.") })
        if (facts.dirs > 0) {
            for (let a = 0; a < 3; a++) {
                if (draft[a * 3 + 1] === 2 && draft[a * 3 + 2] === 0) {
                    n.push({ tone: "", text: qsTr("For a folder, Add or remove works only together with Access.") })
                    break
                }
            }
            n.push({ tone: "", text: kind === "mixed"
                        ? qsTr("Folders change only themselves. What is inside keeps its own permissions.")
                        : total === 1 ? qsTr("This folder only. The files and folders inside keep their own permissions.")
                        : qsTr("These folders only. The files and folders inside keep their own permissions.") })
        }
        if (facts.special > 0)
            n.push({ tone: "", text: total === 1 ? qsTr("This item has special system flags. They stay as they are.")
                                     : facts.special === 1 ? qsTr("One of the items has special system flags. They stay as they are.")
                                     : qsTr("%1 of the items have special system flags. They stay as they are.").arg(facts.special) })
        if (facts.links > 0)
            n.push({ tone: "", text: facts.links === 1 ? qsTr("1 link is left out: a link has no permissions of its own.")
                                     : qsTr("%1 links are left out: a link has no permissions of its own.").arg(facts.links) })
        if (facts.missing.length > 0)
            n.push({ tone: "warn", text: qsTr("Left out, could not be read: %1").arg(facts.missing.join("; ")) })
        return n
    }

    readonly property string statusText: loading ? qsTr("Reading permissions…")
        : busy ? qsTr("Applying…")
        : !ready ? ""
        : facts.owned === 0 ? qsTr("View only")
        : changes === 0 ? qsTr("No changes")
        : changes === 1 ? qsTr("1 change") : qsTr("%1 changes").arg(changes)

    /// The cells, by bit index, for keyboard moves between them.
    property var cellItems: []

    function openFor(m) {
        root.model = m
        root.facts = null
        root.orig = []
        root.draft = []
        root.errorText = ""
        root.explaining = false
        root.busy = false
        root.token = 0
        root.loading = true
        root.open = true
        flick.contentY = 0
        cancelButton.forceActiveFocus()
        const why = m ? m.permissions_load() : qsTr("Nothing to change here")
        if (why !== "") {
            root.loading = false
            root.facts = { ok: false, error: why }
            root.errorText = why
        }
    }

    function receive(tok, json) {
        if (!root.open || !root.loading) return
        let f
        try { f = JSON.parse(json) } catch (e) { f = { ok: false, error: qsTr("The permissions could not be read") } }
        root.token = tok
        root.loading = false
        root.facts = f
        if (!f.ok) {
            root.errorText = f.error || qsTr("The permissions could not be read")
            cancelButton.forceActiveFocus()
            return
        }
        const o = f.bits.map(c => c === 0 ? 0 : c === f.total ? 2 : 1)
        root.orig = o
        root.draft = o.slice()
        if (root.editable) Qt.callLater(() => root.focusCell(0))
    }

    function focusCell(i) {
        const c = root.cellItems[i]
        if (c) c.forceActiveFocus()
    }

    /// Whatever has the keyboard is scrolled into view: in a small or
    /// zoomed window the boxes do not all fit at once.
    /// Positioners lay out after focus lands, so this runs again whenever
    /// the scrolled area changes size.
    readonly property Item focusedItem: Window.activeFocusItem
    onFocusedItemChanged: root.revealFocus()
    function revealFocus() {
        const it = root.focusedItem
        if (!root.open || !it || !flick.contentItem) return
        let inside = false
        for (let p = it.parent; p; p = p.parent)
            if (p === flick.contentItem) { inside = true; break }
        if (!inside) return
        const y = it.mapToItem(flick.contentItem, 0, 0).y
        const most = Math.max(0, flick.contentHeight - flick.height)
        if (y < flick.contentY) flick.contentY = Math.max(0, y)
        else if (y + it.height > flick.contentY + flick.height)
            flick.contentY = Math.min(most, y + it.height - flick.height)
    }

    /// On ↔ off; a mixed bit also returns to mixed (each item as it is).
    function toggle(i) {
        if (!root.editable || i < 0 || i >= root.draft.length) return
        const d = root.draft.slice()
        if (root.orig[i] === 1) d[i] = d[i] === 1 ? 2 : d[i] === 2 ? 0 : 1
        else d[i] = d[i] === 2 ? 0 : 2
        root.draft = d
        root.errorText = ""
    }

    function apply() {
        if (!root.editable || !root.changed || !root.model) return
        const err = root.model.permissions_apply(root.token, root.addMask, root.removeMask)
        if (err !== "") { root.errorText = err; return }
        root.errorText = ""
        root.busy = true
    }

    function cancel() {
        // The change is already with the system; it reports when done.
        if (root.busy) return
        root.close()
    }

    function close() {
        root.open = false
        root.loading = false
        root.busy = false
        root.model = null
        root.closed()
    }

    /// For QA: what the card shows and would ask for.
    function qaState() {
        return {
            loading: root.loading, busy: root.busy, ready: root.ready, kind: root.kind,
            editable: root.editable, orig: root.orig, draft: root.draft,
            add: root.addMask, remove: root.removeMask, changed: root.changed,
            error: root.errorText, status: root.statusText, context: root.context,
            labels: root.permLabels, audiences: root.audiences.map(a => a.title + " | " + a.sub),
            notices: root.notices.map(n => n.text), explaining: root.explaining
        }
    }
    /// For QA: centres, in window coordinates, of each cell and button.
    function points() {
        const at = (it) => {
            const p = it.mapToItem(null, it.width / 2, it.height / 2)
            return [Math.round(p.x), Math.round(p.y)]
        }
        return { cells: root.cellItems.map(c => c ? at(c) : null),
                 cancel: at(cancelButton), apply: at(applyButton), explain: at(explainToggle) }
    }

    Connections {
        target: root.model
        function onPermissions_ready(tok, json) { root.receive(tok, json) }
        function onPermissions_done(tok, error, message, partial) {
            if (!root.open || tok !== root.token) return
            root.busy = false
            if (error !== "") { root.errorText = error; return }
            root.finished(message, partial)
            root.close()
        }
    }

    /// Tab and Shift+Tab stay inside the card, in reading order.
    function moveFocus(step) {
        const ring = (root.ready ? root.cellItems.slice(0, 9) : [])
            .concat(root.ready ? [explainToggle] : [], [cancelButton], applyButton.enabled ? [applyButton] : [])
            .filter(it => !!it)
        if (ring.length === 0) return
        const at = ring.findIndex(it => it.activeFocus)
        ring[(at + step + ring.length) % ring.length].forceActiveFocus()
    }

    Keys.onPressed: (event) => {
        if (event.key === Qt.Key_Tab || event.key === Qt.Key_Backtab) {
            root.moveFocus(event.key === Qt.Key_Backtab || (event.modifiers & Qt.ShiftModifier) ? -1 : 1)
            event.accepted = true
        } else if (event.key === Qt.Key_Escape) {
            root.cancel()
            event.accepted = true
        } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
            root.apply()
            event.accepted = true
        }
    }

    // Scrim: a click outside is Cancel, as in the other cards.
    Rectangle {
        anchors.fill: parent
        color: Tokens.withAlpha(Tokens.background, 0.72)
        MouseArea {
            anchors.fill: parent
            onClicked: root.cancel()
        }
    }

    FontMetrics {
        id: fm
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontBody
    }
    TextMetrics {
        id: widestLabel
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontSmall
        text: qsTr("Add or remove")
    }
    TextMetrics {
        id: widestAudience
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontBody
        font.bold: true
        text: qsTr("Other users")
    }

    Rectangle {
        id: card
        objectName: "permCard"
        anchors.centerIn: parent
        width: Math.min(parent.width - Tokens.spaceXl * 2,
                        Math.round(560 * Tokens.spacingScale * Tokens.fontBase / 12))
        height: Math.min(parent.height - Tokens.spaceXl * 2,
                         content.implicitHeight + Tokens.spaceXl + footer.implicitHeight + Tokens.spaceXxl * 2)
        color: Tokens.menuBackground
        border.width: Math.max(1, Tokens.borderWidth)
        border.color: root.errorText !== "" ? Tokens.danger : Tokens.border
        radius: Tokens.radius

        MouseArea { anchors.fill: parent } // swallow clicks

        // The choices scroll when the window is small; the buttons below
        // them never do.
        Flickable {
            id: flick
            objectName: "permFlick"
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.bottom: footer.top
            anchors.margins: Tokens.spaceXxl
            anchors.bottomMargin: Tokens.spaceXl
            contentWidth: width
            contentHeight: content.implicitHeight
            boundsBehavior: Flickable.StopAtBounds
            interactive: contentHeight > height
            onContentHeightChanged: root.revealFocus()
            onHeightChanged: root.revealFocus()
            clip: true

            Column {
                id: content
                objectName: "permContent"
                width: flick.width
                spacing: Tokens.spaceXl

                // Columns as wide as their longest label, until that would
                // crowd out who the row is for; then they narrow and their
                // labels wrap, never below the box itself.
                readonly property real colIdeal: Math.max(widestLabel.advanceWidth + Tokens.spaceLg * 2,
                                                          Tokens.controlHeight * 2)
                readonly property real whoMin: Math.max(widestAudience.advanceWidth + Tokens.spaceMd,
                                                        Math.round(width * 0.3))
                readonly property real colWidth: Math.max(Math.round(Tokens.fontBody * 1.35) + Tokens.spaceSm * 2,
                                                          Math.min(colIdeal, Math.floor((width - whoMin) / 3)))
                readonly property real whoWidth: Math.max(0, width - colWidth * 3)

                // Title, and what it is about.
                Row {
                    width: parent.width
                    spacing: Tokens.spaceLg

                    FileIcon {
                        id: icon
                        size: Math.round(Tokens.controlHeight * 1.3)
                        look: Tokens.iconLook(root.kind === "folder" ? "d:" + (root.ready && root.facts.name ? root.facts.name : "")
                                              : "f:" + (root.ready && root.facts.name ? root.facts.name : ""))
                        anchors.verticalCenter: titleColumn.verticalCenter
                    }
                    Column {
                        id: titleColumn
                        width: parent.width - icon.width - parent.spacing
                        spacing: Tokens.spaceXs
                        Text {
                            textFormat: Text.PlainText
                            width: parent.width
                            text: qsTr("Permissions")
                            color: Tokens.foregroundBright
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSubtitle
                            font.bold: true
                            objectName: "permissionsTitle"
                        }
                        Text {
                            textFormat: Text.PlainText
                            width: parent.width
                            text: root.loading ? qsTr("Reading permissions…") : root.context
                            visible: text !== ""
                            color: Tokens.foregroundDim
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                            elide: Text.ElideMiddle
                            objectName: "permissionsContext"
                        }
                    }
                }

                Text {
                    textFormat: Text.PlainText
                    width: parent.width
                    visible: root.ready
                    text: qsTr("Choose who can use %1, and what they can do.").arg(root.subject)
                    color: Tokens.foreground
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                    wrapMode: Text.Wrap
                }

                // Who × what.
                Column {
                    id: grid
                    width: parent.width
                    visible: root.ready
                    spacing: 0

                    Row {
                        Text {
                            textFormat: Text.PlainText
                            width: content.whoWidth
                            height: Math.max(Tokens.controlHeight, implicitHeight)
                            anchors.verticalCenter: parent.verticalCenter
                            verticalAlignment: Text.AlignVCenter
                            text: qsTr("Who has access")
                            color: Tokens.foregroundDim
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                            wrapMode: Text.Wrap
                            maximumLineCount: 2
                            elide: Text.ElideRight
                        }
                        Repeater {
                            model: root.permLabels
                            delegate: Text {
                                required property string modelData
                                textFormat: Text.PlainText
                                width: content.colWidth
                                height: Math.max(Tokens.controlHeight, implicitHeight)
                                anchors.verticalCenter: parent.verticalCenter
                                horizontalAlignment: Text.AlignHCenter
                                verticalAlignment: Text.AlignVCenter
                                text: modelData
                                color: Tokens.foregroundDim
                                font.family: Tokens.fontFamily
                                font.pixelSize: Tokens.fontSmall
                                wrapMode: Text.Wrap
                                maximumLineCount: 2
                                elide: Text.ElideRight
                                objectName: "permHeader"
                            }
                        }
                    }

                    Repeater {
                        model: root.audiences
                        delegate: Column {
                            id: audienceRow
                            required property var modelData
                            required property int index
                            width: grid.width

                            Rectangle {
                                width: parent.width
                                height: Math.max(1, Tokens.borderWidth)
                                color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                            }
                            Row {
                                height: Math.max(Tokens.controlHeight * 1.5,
                                                 who.implicitHeight + Tokens.spaceLg * 2)
                                Column {
                                    id: who
                                    width: content.whoWidth
                                    anchors.verticalCenter: parent.verticalCenter
                                    spacing: Tokens.spaceXs
                                    Text {
                                        textFormat: Text.PlainText
                                        width: parent.width - Tokens.spaceMd
                                        text: audienceRow.modelData.title
                                        objectName: "permAudience"
                                        color: Tokens.foregroundBright
                                        font.family: Tokens.fontFamily
                                        font.pixelSize: Tokens.fontBody
                                        font.bold: true
                                        elide: Text.ElideRight
                                    }
                                    Text {
                                        textFormat: Text.PlainText
                                        width: parent.width - Tokens.spaceMd
                                        text: audienceRow.modelData.sub
                                        color: Tokens.foregroundDim
                                        font.family: Tokens.fontFamily
                                        font.pixelSize: Tokens.fontSmall
                                        wrapMode: Text.Wrap
                                        maximumLineCount: 2
                                        elide: Text.ElideRight
                                    }
                                }
                                Repeater {
                                    model: 3
                                    delegate: Item {
                                        id: cell
                                        required property int index
                                        readonly property int bit: audienceRow.index * 3 + index
                                        readonly property int value: root.draft.length > bit ? root.draft[bit] : 0
                                        readonly property bool moved: root.orig.length > bit && root.draft[bit] !== root.orig[bit]
                                        width: content.colWidth
                                        height: parent.height
                                        activeFocusOnTab: root.ready
                                        objectName: "permCell" + bit
                                        Component.onCompleted: root.cellItems[bit] = cell

                                        Rectangle {
                                            anchors.fill: parent
                                            anchors.margins: Tokens.spaceXs
                                            color: cell.activeFocus
                                                   ? Tokens.withAlpha(Tokens.selectedBackground, Math.max(Tokens.selectedAlpha, 0.14))
                                                   : cellMouse.containsMouse && root.editable
                                                     ? Tokens.withAlpha(Tokens.foreground, 0.06) : "transparent"
                                            border.width: cell.activeFocus ? Math.max(1, Tokens.borderWidth) : 0
                                            border.color: Tokens.accent
                                            radius: Tokens.radius
                                        }

                                        Rectangle {
                                            id: box
                                            anchors.centerIn: parent
                                            width: Math.round(Tokens.fontBody * 1.35)
                                            height: width
                                            radius: Tokens.radius
                                            opacity: root.editable || root.busy ? 1.0 : 0.5
                                            color: cell.value === 2 ? Tokens.accent : Tokens.backgroundDark
                                            border.width: Math.max(1, Tokens.borderWidth)
                                            border.color: cell.value === 0 ? Tokens.foregroundDim : Tokens.accent

                                            Text {
                                                textFormat: Text.PlainText
                                                anchors.centerIn: parent
                                                visible: cell.value === 2
                                                text: "✓"
                                                color: Tokens.background
                                                font.family: Tokens.fontFamily
                                                font.pixelSize: Tokens.fontBody
                                                font.bold: true
                                            }
                                            // Mixed: some items have it, some do not.
                                            Rectangle {
                                                anchors.centerIn: parent
                                                visible: cell.value === 1
                                                width: Math.round(parent.width * 0.5)
                                                height: Math.max(2, Math.round(parent.height * 0.14))
                                                color: Tokens.accent
                                            }
                                        }
                                        // A changed cell is marked under its box.
                                        Rectangle {
                                            anchors.horizontalCenter: parent.horizontalCenter
                                            anchors.top: box.bottom
                                            anchors.topMargin: Tokens.spaceXs
                                            visible: cell.moved
                                            width: box.width
                                            height: Math.max(2, Tokens.borderWidth * 2)
                                            color: Tokens.accent
                                        }

                                        MouseArea {
                                            id: cellMouse
                                            anchors.fill: parent
                                            hoverEnabled: true
                                            cursorShape: root.editable ? Qt.PointingHandCursor : Qt.ArrowCursor
                                            onClicked: { cell.forceActiveFocus(); root.toggle(cell.bit) }
                                        }
                                        Keys.onPressed: (event) => {
                                            const r = audienceRow.index, c = cell.index
                                            let to = -1
                                            if (event.key === Qt.Key_Space) { root.toggle(cell.bit); event.accepted = true; return }
                                            if (event.key === Qt.Key_Left && c > 0) to = cell.bit - 1
                                            else if (event.key === Qt.Key_Right && c < 2) to = cell.bit + 1
                                            else if (event.key === Qt.Key_Up && r > 0) to = cell.bit - 3
                                            else if (event.key === Qt.Key_Down && r < 2) to = cell.bit + 3
                                            if (to >= 0) { root.focusCell(to); event.accepted = true }
                                        }

                                        Accessible.role: Accessible.CheckBox
                                        Accessible.name: audienceRow.modelData.title + ": " + (root.permLabels[cell.index] || "")
                                        Accessible.checkable: root.editable
                                        Accessible.checked: cell.value === 2
                                        Accessible.checkStateMixed: cell.value === 1
                                        Accessible.description: cell.value === 1 ? qsTr("Some of the items have this") : ""
                                    }
                                }
                            }
                        }
                    }
                    Rectangle {
                        width: parent.width
                        height: Math.max(1, Tokens.borderWidth)
                        color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                    }
                }

                // What a mixed box means, only when one is on screen.
                Row {
                    visible: root.ready && root.anyMixed
                    spacing: Tokens.spaceMd
                    Rectangle {
                        anchors.verticalCenter: parent.verticalCenter
                        width: Math.round(Tokens.fontSmall * 1.2)
                        height: width
                        radius: Tokens.radius
                        color: Tokens.backgroundDark
                        border.width: Math.max(1, Tokens.borderWidth)
                        border.color: Tokens.accent
                        Rectangle {
                            anchors.centerIn: parent
                            width: Math.round(parent.width * 0.5)
                            height: Math.max(2, Math.round(parent.height * 0.14))
                            color: Tokens.accent
                        }
                    }
                    Text {
                        textFormat: Text.PlainText
                        width: content.width - Tokens.fontSmall * 1.2 - Tokens.spaceMd
                        text: qsTr("Some of the items have this. Left as it is, each item keeps its own.")
                        color: Tokens.foregroundDim
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSmall
                        wrapMode: Text.Wrap
                    }
                }

                // Explanations, on demand.
                Column {
                    width: parent.width
                    visible: root.ready
                    spacing: Tokens.spaceMd

                    Item {
                        id: explainToggle
                        width: explainLabel.implicitWidth + Tokens.spaceMd * 2
                        height: Tokens.controlHeight
                        activeFocusOnTab: true
                        objectName: "permExplain"
                        Rectangle {
                            anchors.fill: parent
                            color: explainToggle.activeFocus
                                   ? Tokens.withAlpha(Tokens.selectedBackground, Math.max(Tokens.selectedAlpha, 0.14))
                                   : explainMouse.containsMouse ? Tokens.withAlpha(Tokens.foreground, 0.06) : "transparent"
                            border.width: explainToggle.activeFocus ? Math.max(1, Tokens.borderWidth) : 0
                            border.color: Tokens.accent
                            radius: Tokens.radius
                        }
                        Text {
                            id: explainLabel
                            textFormat: Text.PlainText
                            anchors.centerIn: parent
                            text: (root.explaining ? "▾ " : "▸ ") + qsTr("What do these mean?")
                            color: Tokens.foreground
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                        }
                        MouseArea {
                            id: explainMouse
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: root.explaining = !root.explaining
                        }
                        Keys.onPressed: (event) => {
                            if (event.key === Qt.Key_Space || event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                                root.explaining = !root.explaining
                                event.accepted = true
                            }
                        }
                        Accessible.role: Accessible.Button
                        Accessible.name: qsTr("What do these mean?")
                        Accessible.checkable: true
                        Accessible.checked: root.explaining
                    }

                    Column {
                        width: parent.width
                        visible: root.explaining
                        spacing: Tokens.spaceSm
                        TextMetrics {
                            id: termMetrics
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                            font.bold: true
                            text: qsTr("Add or remove")
                        }
                        Repeater {
                            model: root.explaining ? root.definitions : []
                            delegate: Row {
                                required property var modelData
                                spacing: Tokens.spaceLg
                                Text {
                                    textFormat: Text.PlainText
                                    width: termMetrics.advanceWidth + Tokens.spaceSm
                                    text: modelData[0]
                                    color: modelData[1] === "" ? Tokens.foregroundDim : Tokens.foregroundBright
                                    font.family: Tokens.fontFamily
                                    font.pixelSize: Tokens.fontSmall
                                    font.bold: modelData[1] !== ""
                                }
                                Text {
                                    textFormat: Text.PlainText
                                    width: content.width - termMetrics.advanceWidth - Tokens.spaceSm - Tokens.spaceLg
                                    text: modelData[1]
                                    color: Tokens.foreground
                                    font.family: Tokens.fontFamily
                                    font.pixelSize: Tokens.fontSmall
                                    wrapMode: Text.Wrap
                                }
                            }
                        }
                        Text {
                            textFormat: Text.PlainText
                            width: parent.width
                            text: qsTr("These are the basic permissions on this computer. The folders above it and other access rules can also limit who gets in.")
                            color: Tokens.foregroundDim
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                            wrapMode: Text.Wrap
                        }
                    }
                }

                // What the card needs to say: refusals, warnings, scope.
                Rectangle {
                    width: parent.width
                    visible: root.notices.length > 0
                    height: noticeColumn.implicitHeight + Tokens.spaceLg * 2
                    color: Tokens.backgroundDark
                    radius: Tokens.radius
                    border.width: Math.max(1, Tokens.borderWidth)
                    border.color: root.errorText !== "" ? Tokens.danger : Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                    Column {
                        id: noticeColumn
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.margins: Tokens.spaceLg
                        spacing: Tokens.spaceSm
                        Repeater {
                            model: root.notices
                            // A warning is marked in its tone; its words stay
                            // in the readable foreground on every theme.
                            delegate: Row {
                                required property var modelData
                                spacing: Tokens.spaceMd
                                Rectangle {
                                    width: Math.max(2, Tokens.borderWidth * 2)
                                    height: noticeText.height
                                    color: modelData.tone === "danger" ? Tokens.danger
                                           : modelData.tone === "warn" ? Tokens.warn : "transparent"
                                }
                                Text {
                                    id: noticeText
                                    textFormat: Text.PlainText
                                    width: noticeColumn.width - Math.max(2, Tokens.borderWidth * 2) - Tokens.spaceMd
                                    text: modelData.text
                                    color: modelData.tone === "danger" ? Tokens.danger : Tokens.foreground
                                    font.family: Tokens.fontFamily
                                    font.pixelSize: Tokens.fontSmall
                                    wrapMode: Text.Wrap
                                    objectName: "permNotice"
                                }
                            }
                        }
                    }
                }
            }
        }

        // In the card's margin, clear of the Run column.
        ScrollIndicator {
            target: flick
            x: flick.x + flick.width + Math.round((Tokens.spaceXxl - thickness) / 2)
        }

        Column {
            id: footer
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            anchors.margins: Tokens.spaceXxl
            spacing: Tokens.spaceXl

            Rectangle {
                width: parent.width
                height: Math.max(1, Tokens.borderWidth)
                color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
            }

            // Status, then Cancel and Apply changes.
            Item {
                width: parent.width
                height: Tokens.controlHeight

                Text {
                    textFormat: Text.PlainText
                    anchors.left: parent.left
                    anchors.right: buttons.left
                    anchors.rightMargin: Tokens.spaceLg
                    anchors.verticalCenter: parent.verticalCenter
                    text: root.statusText
                    color: Tokens.foregroundDim
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontSmall
                    elide: Text.ElideRight
                    objectName: "permStatus"
                    Accessible.role: Accessible.StaticText
                    Accessible.name: text
                }

                Row {
                    id: buttons
                    anchors.right: parent.right
                    spacing: Tokens.spaceLg

                    Rectangle {
                        id: cancelButton
                        width: cancelLabel.implicitWidth + Tokens.spaceXxl
                        height: Tokens.controlHeight
                        color: activeFocus ? Tokens.withAlpha(Tokens.foreground, 0.14)
                               : cancelMouse.containsMouse ? Tokens.backgroundLighter : "transparent"
                        border.width: Math.max(1, Tokens.borderWidth)
                        border.color: activeFocus ? Tokens.accent : Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                        radius: Tokens.radius
                        activeFocusOnTab: true
                        objectName: "permCancel"
                        opacity: root.busy ? 0.4 : 1.0
                        Text {
                            id: cancelLabel
                            textFormat: Text.PlainText
                            anchors.centerIn: parent
                            // Nothing to cancel when nothing can change.
                            text: root.loading || (root.ready && root.facts.owned > 0) ? qsTr("Cancel") : qsTr("Close")
                            color: Tokens.foreground
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontBody
                        }
                        MouseArea {
                            id: cancelMouse
                            anchors.fill: parent
                            hoverEnabled: true
                            onClicked: root.cancel()
                        }
                        Keys.onPressed: (event) => {
                            if (event.key === Qt.Key_Space || event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                                root.cancel()
                                event.accepted = true
                            }
                        }
                        Accessible.role: Accessible.Button
                        Accessible.name: cancelLabel.text
                    }

                    Rectangle {
                        id: applyButton
                        visible: root.ready && root.facts.owned > 0
                        enabled: root.editable && root.changed
                        width: applyLabel.implicitWidth + Tokens.spaceXxl
                        height: Tokens.controlHeight
                        color: activeFocus ? Tokens.withAlpha(Tokens.accent, 0.2)
                               : applyMouse.containsMouse ? Tokens.withAlpha(Tokens.accent, 0.22)
                               : "transparent"
                        border.width: Math.max(1, Tokens.borderWidth)
                        border.color: activeFocus ? Tokens.foregroundBright : Tokens.accent
                        radius: Tokens.radius
                        opacity: enabled ? 1.0 : 0.4
                        activeFocusOnTab: true
                        objectName: "permApply"
                        Text {
                            id: applyLabel
                            textFormat: Text.PlainText
                            anchors.centerIn: parent
                            text: qsTr("Apply changes")
                            color: Tokens.foregroundBright
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontBody
                            font.bold: true
                        }
                        MouseArea {
                            id: applyMouse
                            anchors.fill: parent
                            hoverEnabled: true
                            onClicked: root.apply()
                        }
                        Keys.onPressed: (event) => {
                            if (event.key === Qt.Key_Space || event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                                root.apply()
                                event.accepted = true
                            }
                        }
                        Accessible.role: Accessible.Button
                        Accessible.name: qsTr("Apply changes")
                    }
                }
            }

            Text {
                textFormat: Text.PlainText
                width: parent.width
                horizontalAlignment: Text.AlignRight
                // A short window keeps its room for the choices themselves.
                visible: root.editable && root.height >= Tokens.controlHeight * 10
                elide: Text.ElideLeft
                text: qsTr("Space switch · arrows move · Enter apply · Esc cancel")
                color: Tokens.foregroundDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontCaption
            }
        }
    }

    Accessible.role: Accessible.Dialog
    Accessible.name: qsTr("Permissions")
    Accessible.description: root.context
}
