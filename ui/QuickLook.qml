// Quick Look — what a file IS, without running it (DESIGN §6.9).
//
// Everything shown here came from bytes and metadata the bridge already read
// on a worker. This component decodes nothing, runs nothing, and follows no
// link; text arrives pre-sanitised so a file full of escape sequences is
// shown rather than obeyed.

import QtQuick
import com.gust.app

Item {
    id: root
    anchors.fill: parent
    visible: open
    z: 50

    /// Is the panel showing?
    property bool open: false
    /// The parsed peek, as the bridge reported it.
    property var info: null
    /// SHA-256 state from the model (peek_hash).
    readonly property string hash: root.model && root.open ? root.model.peek_hash : ""
    /// Folders have no SHA-256. The kind is "Folder" locally, "folder" remotely.
    readonly property bool hashable: root.info !== null && root.info.kind.toLowerCase() !== "folder"

    signal closed()
    /// Run… was chosen for the file shown; the window asks before anything
    /// starts. -1 when the file is no longer at the cursor.
    signal runRequested(int row)
    function requestRun() {
        root.runRequested(root.model ? root.model.preview_row() : -1)
    }
    /// The row shown, so h can hash it.
    property int row: -1
    property real imageZoom: 1

    /// The model, so the panel can ask for and receive its visual preview.
    property var model: null
    // The panel belongs to the folder it was opened in. Another tab means
    // another model whose rows are different files: close, never act on them.
    onModelChanged: if (root.open) root.hide()
    // The model tracks the file shown byte-exact (preview_track). When its
    // rows are rebuilt — a chmod from this panel reloads the folder — the
    // row follows that file, so h, pages and browsing act on what is shown.
    Connections {
        target: root.model
        function onModelReset() { root.followFile() }
        function onRowsInserted() { root.followFile() }
        function onRowsRemoved() { root.followFile() }
    }
    function followFile() {
        if (!root.open || !root.model) return
        const r = root.model.preview_row()
        if (r < 0) { root.hide(); return }
        root.row = r
        // The cursor follows the preview, as when browsing: Enter opens
        // what is shown.
        if (root.model.cursor !== r) root.model.focus_row(r)
    }
    /// Image path or archive listing, rendered on a worker (sandboxed).
    readonly property var visual: {
        const s = root.model && root.open ? root.model.peek_visual : ""
        if (!s) return null
        try { return JSON.parse(s) } catch (e) { return null }
    }

    // --- running as a program -------------------------------------------
    /// "run", "allow" or "" — DirModel.program_state for the row shown.
    property string program: ""
    /// Stays true once the row was a program, so turning the permission
    /// off does not make the switch that turns it back on disappear.
    property bool programSeen: false
    property string programError: ""
    function refreshProgram() {
        root.program = root.open && root.model ? root.model.program_state_preview() : ""
        if (root.program !== "") root.programSeen = true
    }
    /// The Properties switch: allow (u+x) or stop (a-x) this one file.
    function setRunnable(on) {
        if (!root.open || !root.model) return
        root.programError = root.model.set_runnable(on)
        // The mode line and the Permissions fact change with it.
        try {
            const fresh = JSON.parse(root.model.peek_preview())
            if (fresh.ok) root.info = fresh
        } catch (e) {}
        root.refreshProgram()
    }

    function show(json, row) {
        if (root.open && root.model) {
            audioPoll.stop()
            root.model.audio_stop()
            root.model.hash_cancel()
        }
        try {
            root.info = JSON.parse(json)
        } catch (e) {
            root.info = null
        }
        root.open = root.info !== null && root.info.ok
        root.row = row === undefined ? -1 : row
        if (root.model) root.model.preview_track(root.open ? root.row : -1)
        root.audio = null
        root.imageZoom = 1
        root.programSeen = false
        root.programError = ""
        root.refreshProgram()
        if (root.open && root.model && row !== undefined) root.model.request_peek_visual(row)
    }

    function browse(delta) {
        if (!root.open || !root.model) return false
        const next = root.model.preview_neighbor(root.row, delta)
        if (next !== root.row) {
            root.model.focus_row(next)
            root.show(root.model.peek_row(next), next)
        }
        // At either end, keep the key out of the file list underneath.
        return true
    }

    function zoomBy(delta) {
        if (!root.open) return false
        if (root.visual && root.visual.image !== undefined)
            root.imageZoom = Math.max(0.125, Math.min(8, root.imageZoom * Math.pow(2, delta / 10)))
        // Audio/text have no image to zoom; keep their cursor in place too.
        return true
    }

    // --- PDF pages --------------------------------------------------------
    readonly property int page: root.visual && root.visual.page ? root.visual.page : 0
    readonly property int pages: root.visual && root.visual.pages ? root.visual.pages : 0
    /// Turn the page by `d` (Left/Right, PageUp/PageDown). A PDF owns these
    /// keys even while loading, on failure or with only one page; other files
    /// fall through to browsing. The peek name has no MIME field, and display
    /// sanitisation preserves its .pdf suffix.
    function turnPage(d) {
        if (!root.open || !root.model) return false
        if (root.pages < 2) return root.info !== null && /\.pdf$/i.test(root.info.name)
        const want = Math.max(1, Math.min(root.pages, root.page + d))
        if (want !== root.page) root.model.request_peek_page(root.row, want)
        return true
    }

    // --- audio ------------------------------------------------------------
    /// Is there sound to play here? Starts it as soon as it is known.
    readonly property bool isAudio: root.visual !== null && root.visual.audio === true
    onIsAudioChanged: if (isAudio && root.model) { root.model.audio_play(root.row); audioPoll.start() }
    /// The player's state, polled while it plays.
    property var audio: null
    Timer {
        id: audioPoll
        interval: 200
        repeat: true
        running: false
        onTriggered: {
            if (!root.open || !root.model) { stop(); return }
            const s = root.model.audio_status()
            try { root.audio = s === "" ? null : JSON.parse(s) } catch (e) { root.audio = null }
        }
    }
    /// p / k: play or pause. False when nothing is playing here.
    function togglePlay() {
        if (!root.isAudio || !root.model) return false
        root.model.audio_toggle()
        return true
    }
    /// Shift+Left / Right: five seconds back or forward.
    function seekBy(d) {
        if (!root.isAudio || !root.model || !root.audio) return false
        root.model.audio_seek(Math.max(0, root.audio.pos + d))
        return true
    }
    /// A point near the end of the position bar, in window coordinates (qa).
    function audioBarPoint() {
        if (!audioPanel.visible || track.width <= 0) return null
        const p = track.mapToItem(null, track.width * 0.92, track.height / 2)
        return [Math.round(p.x), Math.round(p.y)]
    }
    function clock(t) {
        if (t === null || t === undefined || !isFinite(t)) return "–:––"
        t = Math.max(0, Math.floor(t))
        const h = Math.floor(t / 3600), m = Math.floor(t / 60) % 60, sec = t % 60
        return (h > 0 ? h + ":" + (m < 10 ? "0" : "") : "") + m + ":" + (sec < 10 ? "0" : "") + sec
    }
    /// Is the close hint inside the panel? (qa)
    /// The preview image: rendered size and drawn size, in pixels (qa).
    /// Explicit user zoom can enlarge it; fit mode never upscales the render.
    function imageInfo() {
        if (!visualImage.visible || visualImage.status !== Image.Ready) return null
        return { rendered: [visualImage.implicitWidth, visualImage.implicitHeight],
                 drawn: [Math.round(visualImage.paintedWidth * root.imageZoom),
                         Math.round(visualImage.paintedHeight * root.imageZoom)],
                 zoom: root.imageZoom }
    }

    function closeInside() {
        if (!root.open) return true
        const b = qlClose.mapToItem(panel, 0, qlClose.height)
        return b.y <= panel.height
    }
    function hide() {
        audioPoll.stop()
        if (root.model) { root.model.audio_stop(); root.model.hash_cancel() }
        root.audio = null
        root.open = false
        root.info = null
        if (root.model) root.model.preview_track(-1)
        root.program = ""
        root.programSeen = false
        root.closed()
    }

    // A dim behind the panel, so the file underneath is clearly not the
    // thing being interacted with.
    Rectangle {
        anchors.fill: parent
        color: Tokens.withAlpha(Tokens.background, 0.82)
        MouseArea {
            anchors.fill: parent
            onClicked: root.hide()
        }
    }

    Rectangle {
        id: panel
        anchors.centerIn: parent
        // A file with nothing to preview gets a compact card that fits its
        // facts; only text, images, pages, archives and sound get the big
        // panel. Decided from the name up front, so it never jumps.
        readonly property bool hasPreview: root.info !== null
            && ((root.info.lines && root.info.lines.length > 0)
                || root.info.visualExpected === true || root.visual !== null)
        width: Math.min(parent.width - Tokens.spaceXl * 4,
                        Math.round(Tokens.rowHeight * (hasPreview ? 26 : 20)))
        height: hasPreview
                ? Math.min(parent.height - Tokens.spaceXl * 4, Math.round(Tokens.rowHeight * 24))
                : Math.min(parent.height - Tokens.spaceXl * 4, qlColumn.implicitHeight + Tokens.spaceXl * 2)
        color: Tokens.backgroundDark
        border.width: Math.max(1, Tokens.borderWidth)
        border.color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
        radius: Tokens.radius

        Column {
            id: qlColumn
            anchors.fill: parent
            anchors.margins: Tokens.spaceXl
            spacing: Tokens.spaceLg

            Text {
                textFormat: Text.PlainText
                id: qlName
                width: parent.width
                text: root.info ? root.info.name : ""
                color: Tokens.foregroundBright
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSubtitle
                elide: Text.ElideMiddle
            }

            // Kind, size and mode on one honest line. The kind is words
            // ("PNG image"), never a MIME type.
            Text {
                textFormat: Text.PlainText
                id: qlKind
                width: parent.width
                text: root.info
                      ? (root.info.kindWords
                         + (root.info.size > 0 ? "  ·  " + root.info.sizeText : "")
                         + (root.info.mode !== "" ? "  ·  " + root.info.mode : ""))
                      : ""
                color: Tokens.foregroundDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
                wrapMode: Text.WordWrap
            }

            // Properties: where it is, when it changed, who owns it — the
            // facts a Properties window would hold, without a second window.
            Grid {
                id: factsGrid
                width: parent.width
                columns: 2
                columnSpacing: Tokens.spaceXl
                rowSpacing: Tokens.spaceXs
                visible: root.info && root.info.facts && root.info.facts.length > 0
                Repeater {
                    model: root.info && root.info.facts ? root.info.facts.length * 2 : 0
                    delegate: Text {
                        required property int index
                        readonly property var pair: root.info.facts[Math.floor(index / 2)]
                        readonly property bool isLabel: index % 2 === 0
                        width: isLabel ? Math.round(Tokens.rowHeight * 4)
                                       : factsGrid.width - Math.round(Tokens.rowHeight * 4) - Tokens.spaceXl
                        text: isLabel ? pair[0] : pair[1]
                        color: isLabel ? Tokens.foregroundDim : Tokens.foreground
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSmall
                        textFormat: Text.PlainText
                        elide: Text.ElideMiddle
                    }
                }
            }

            // SHA-256 on request (h): reading a big file is not free, so it
            // is never started just because Quick Look opened.
            Text {
                id: qlHash
                objectName: "quickLookHash"
                width: parent.width
                // Offered in the key hints; the line appears once asked for.
                visible: root.hashable && root.hash !== ""
                text: root.hash === "…" ? qsTr("SHA-256  reading…")
                    : qsTr("SHA-256  ") + root.hash
                color: root.hash.length === 64 ? Tokens.foreground : Tokens.foregroundDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
                wrapMode: Text.WrapAnywhere
                textFormat: Text.PlainText
            }

            // Run permission, where it matters. A downloaded program arrives
            // without it; this is the switch a Properties window would hold.
            Column {
                id: qlProgram
                objectName: "quickLookProgram"
                width: parent.width
                spacing: Tokens.spaceXs
                visible: root.programSeen

                Item {
                    width: parent.width
                    height: Tokens.controlHeight

                    Row {
                        id: allowSwitch
                        objectName: "quickLookAllowRun"
                        readonly property bool checked: root.program === "run"
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: Tokens.spaceLg

                        Rectangle {
                            anchors.verticalCenter: parent.verticalCenter
                            width: Math.round(Tokens.fontBody * 1.2)
                            height: width
                            color: allowSwitch.checked ? Tokens.withAlpha(Tokens.accent, 0.2) : "transparent"
                            border.width: Math.max(1, Tokens.borderWidth)
                            border.color: allowSwitch.checked || allowArea.containsMouse
                                          ? Tokens.accent : Tokens.foregroundDim
                            radius: Tokens.radius
                            Text {
                                textFormat: Text.PlainText
                                anchors.centerIn: parent
                                visible: allowSwitch.checked
                                text: "✓"
                                color: Tokens.accent
                                font.family: Tokens.fontFamily
                                font.pixelSize: Tokens.fontSmall
                                font.bold: true
                            }
                        }
                        Text {
                            textFormat: Text.PlainText
                            anchors.verticalCenter: parent.verticalCenter
                            text: qsTr("Allow running")
                            color: Tokens.foregroundBright
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                        }

                        Accessible.role: Accessible.CheckBox
                        Accessible.name: qsTr("Allow running")
                        Accessible.checkable: true
                        Accessible.checked: allowSwitch.checked
                    }
                    MouseArea {
                        id: allowArea
                        anchors.fill: allowSwitch
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: root.setRunnable(!allowSwitch.checked)
                    }

                    Rectangle {
                        id: runButton
                        objectName: "quickLookRun"
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        width: runLabel.implicitWidth + Tokens.spaceXxl
                        height: Tokens.controlHeight
                        visible: root.program !== ""
                        color: runArea.containsMouse ? Tokens.withAlpha(Tokens.accent, 0.2) : "transparent"
                        border.width: Math.max(1, Tokens.borderWidth)
                        border.color: Tokens.accent
                        radius: Tokens.radius
                        Text {
                            id: runLabel
                            textFormat: Text.PlainText
                            anchors.centerIn: parent
                            text: qsTr("Run…")
                            color: Tokens.foregroundBright
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                        }
                        MouseArea {
                            id: runArea
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: root.requestRun()
                        }
                        Accessible.role: Accessible.Button
                        Accessible.name: runLabel.text
                    }
                }

                // Only a failure gets words; the switch speaks for itself.
                Text {
                    textFormat: Text.PlainText
                    width: parent.width
                    visible: root.programError !== ""
                    text: root.programError
                    color: Tokens.danger
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontCaption
                    wrapMode: Text.WordWrap
                }
            }

            Rectangle {
                visible: panel.hasPreview
                width: parent.width
                height: Math.max(1, Tokens.borderWidth)
                color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
            }

            // The compact card keeps a reason only when it tells something
            // (a link, a vanished file); "no preview" is what it already looks like.
            Text {
                textFormat: Text.PlainText
                width: parent.width
                visible: !panel.hasPreview && root.info !== null
                         && root.info.noPreview !== "" && root.info.kind !== "File"
                text: root.info ? root.info.noPreview : ""
                color: Tokens.foregroundDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
                wrapMode: Text.WordWrap
            }

            // Either a bounded text peek, or an honest sentence about why
            // there is nothing to show. "No preview" beats a preview that
            // required running the file.
            Item {
                visible: panel.hasPreview
                width: parent.width
                clip: visualImage.visible
                // Everything else in the column, plus one spacing per gap.
                height: panel.height - Tokens.spaceXl * 2
                        - qlName.height - qlKind.height - qlClose.height
                        - (factsGrid.visible ? factsGrid.height + parent.spacing : 0)
                        - (qlHash.visible ? qlHash.height + parent.spacing : 0)
                        - (qlProgram.visible ? qlProgram.height + parent.spacing : 0)
                        - (qlTrunc.visible ? qlTrunc.height + parent.spacing : 0)
                        - Math.max(1, Tokens.borderWidth) - parent.spacing * 4

                // Image, PDF first page or video frame: a PNG the sandboxed
                // helper made. Only this PNG is decoded here, never the file.
                //
                // Fit without upscaling by default. Only explicit user zoom
                // enlarges the cached preview, clipped inside this area.
                Image {
                    id: visualImage
                    objectName: "quickLookImage"
                    anchors.horizontalCenter: parent.horizontalCenter
                    anchors.verticalCenter: parent.verticalCenter
                    anchors.verticalCenterOffset: videoHint.visible ? -(videoHint.height + Tokens.spaceLg) / 2 : 0
                    width: Math.min(parent.width, implicitWidth)
                    height: Math.min(parent.height - (videoHint.visible ? videoHint.height + Tokens.spaceLg : 0),
                                     implicitHeight)
                    visible: root.visual && root.visual.image !== undefined
                    source: visible ? "file://" + root.visual.image : ""
                    fillMode: Image.PreserveAspectFit
                    asynchronous: true
                    cache: false
                    smooth: true
                    mipmap: true
                    scale: root.imageZoom
                }

                // A PDF with more than one page: where we are, and how to move.
                Rectangle {
                    objectName: "quickLookPages"
                    visible: root.pages > 1 && visualImage.visible
                    anchors.bottom: visualImage.bottom
                    anchors.horizontalCenter: parent.horizontalCenter
                    anchors.bottomMargin: Tokens.spaceLg
                    width: pageText.implicitWidth + Tokens.spaceXl * 2
                    height: pageText.implicitHeight + Tokens.spaceSm * 2
                    radius: Tokens.radius
                    color: Tokens.withAlpha(Tokens.backgroundDark, 0.9)
                    border.width: Math.max(1, Tokens.borderWidth)
                    border.color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
                    Text {
                        textFormat: Text.PlainText
                        id: pageText
                        anchors.centerIn: parent
                        text: qsTr("‹  page %1 of %2  ›").arg(root.page).arg(root.pages)
                        color: Tokens.foreground
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSmall
                    }
                    MouseArea {
                        anchors.fill: parent
                        onClicked: (m) => root.turnPage(m.x < width / 2 ? -1 : 1)
                    }
                }

                // A video: the frame above; playing it is the real player's
                // job (Enter), with its own window, controls and codecs.
                Text {
                    textFormat: Text.PlainText
                    id: videoHint
                    objectName: "quickLookVideoHint"
                    visible: root.visual !== null && root.visual.video === true
                    anchors.bottom: parent.bottom
                    anchors.horizontalCenter: parent.horizontalCenter
                    text: qsTr("▶  Enter plays it in your video player")
                    color: Tokens.foreground
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontSmall
                }

                // Markdown, rendered: headings, lists, emphasis and code.
                // The bridge escaped every character of the file first; the
                // only markup here is what it added. Images are not loaded
                // and links lead nowhere.
                Flickable {
                    id: mdFlick
                    objectName: "quickLookMarkdown"
                    WheelScroll { view: mdFlick }
                    anchors.fill: parent
                    visible: root.visual !== null && root.visual.markdown !== undefined
                    clip: true
                    contentHeight: mdText.implicitHeight
                    contentWidth: width
                    boundsBehavior: Flickable.StopAtBounds
                    Text {
                        id: mdText
                        width: parent.width
                        text: mdFlick.visible
                              ? root.visual.markdown.split("\u0001").join(Tokens.accent.toString()) : ""
                        textFormat: Text.StyledText
                        color: Tokens.foreground
                        linkColor: Tokens.accent
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSmall
                        wrapMode: Text.Wrap
                    }
                }

                // Audio: decoded in the sandbox, played through the
                // desktop's own player. p pauses, Shift+Left / Right seek.
                Column {
                    id: audioPanel
                    objectName: "quickLookAudio"
                    visible: root.visual !== null && root.visual.audio !== undefined
                    anchors.centerIn: parent
                    width: parent.width
                    spacing: Tokens.spaceLg
                    readonly property real dur: root.audio && root.audio.duration ? root.audio.duration : 0
                    readonly property real pos: root.audio ? root.audio.pos : 0
                    /// Nothing to play: the sandbox is missing, or the file
                    /// is not audio after all. Controls would only mislead.
                    readonly property bool failed: (root.visual && root.visual.audio === false)
                                                   || (root.audio !== null && root.audio.state === "failed")

                    Text {
                        textFormat: Text.PlainText
                        width: parent.width
                        horizontalAlignment: Text.AlignHCenter
                        text: String.fromCodePoint(0xF0387)
                        visible: Tokens.nerdGlyphs
                        color: Tokens.accent
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontHeading * 3
                    }
                    Text {
                        width: parent.width
                        horizontalAlignment: Text.AlignHCenter
                        text: root.audio && root.audio.title !== "" ? root.audio.title
                              : (root.info ? root.info.name : "")
                        color: Tokens.foregroundBright
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSubtitle
                        textFormat: Text.PlainText
                        elide: Text.ElideMiddle
                    }
                    Text {
                        width: parent.width
                        horizontalAlignment: Text.AlignHCenter
                        visible: text !== ""
                        text: root.audio ? [root.audio.artist, root.audio.album].filter(x => x !== "").join("  ·  ") : ""
                        color: Tokens.foregroundDim
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSmall
                        textFormat: Text.PlainText
                        elide: Text.ElideRight
                    }
                    // Position: click or drag anywhere on the bar to jump.
                    Item {
                        width: parent.width
                        height: Tokens.rowHeight
                        visible: !audioPanel.failed
                        Rectangle {
                            id: track
                            anchors.verticalCenter: parent.verticalCenter
                            width: parent.width
                            height: Math.max(2, Tokens.borderWidth * 3)
                            radius: Tokens.radius
                            color: Tokens.withAlpha(Tokens.foreground, 0.15)
                            Rectangle {
                                objectName: "quickLookAudioFill"
                                height: parent.height
                                radius: Tokens.radius
                                width: audioPanel.dur > 0 ? parent.width * Math.min(1, audioPanel.pos / audioPanel.dur) : 0
                                color: Tokens.accent
                            }
                        }
                        MouseArea {
                            anchors.fill: parent
                            enabled: audioPanel.dur > 0
                            cursorShape: Qt.PointingHandCursor
                            onPressed: (m) => root.model.audio_seek(audioPanel.dur * Math.max(0, Math.min(1, m.x / width)))
                            onPositionChanged: (m) => { if (pressed) root.model.audio_seek(audioPanel.dur * Math.max(0, Math.min(1, m.x / width))) }
                        }
                    }
                    Item {
                        width: parent.width
                        height: playBtn.height
                        visible: !audioPanel.failed
                        Text {
                            textFormat: Text.PlainText
                            anchors.left: parent.left
                            anchors.verticalCenter: parent.verticalCenter
                            text: root.clock(audioPanel.pos)
                            color: Tokens.foregroundDim
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                        }
                        Rectangle {
                            id: playBtn
                            objectName: "quickLookPlay"
                            anchors.horizontalCenter: parent.horizontalCenter
                            width: Tokens.controlHeight * 1.4
                            height: Tokens.controlHeight
                            radius: Tokens.radius
                            color: playMouse.containsMouse ? Tokens.withAlpha(Tokens.accent, 0.22) : "transparent"
                            border.width: Math.max(1, Tokens.borderWidth)
                            border.color: Tokens.accent
                            Text {
                                textFormat: Text.PlainText
                                anchors.centerIn: parent
                                text: !root.audio || root.audio.state === "loading" ? "…"
                                      : root.audio.state === "playing" ? "❚❚"
                                      : root.audio.state === "ended" ? "↺" : "▶"
                                color: Tokens.accent
                                font.family: Tokens.fontFamily
                                font.pixelSize: Tokens.fontBody
                            }
                            MouseArea {
                                id: playMouse
                                anchors.fill: parent
                                hoverEnabled: true
                                cursorShape: Qt.PointingHandCursor
                                onClicked: root.togglePlay()
                            }
                        }
                        Text {
                            textFormat: Text.PlainText
                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            text: root.clock(audioPanel.dur > 0 ? audioPanel.dur : null)
                            color: Tokens.foregroundDim
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                        }
                    }
                    Text {
                        width: parent.width
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.WordWrap
                        text: root.visual && root.visual.audio === false ? root.visual.why
                              : root.audio && root.audio.state === "failed" ? qsTr("Cannot play: %1").arg(root.audio.why)
                              : qsTr("p play  ·  ← → track  ·  Shift+← → seek  ·  Enter open")
                        color: audioPanel.failed ? Tokens.danger : Tokens.foregroundDim
                        font.bold: audioPanel.failed
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontCaption
                        textFormat: Text.PlainText
                    }
                }

                // Archive: what is inside, listed without extracting.
                ListView {
                    id: archiveList
                    WheelScroll { view: archiveList }
                    anchors.fill: parent
                    visible: root.visual && root.visual.archive !== undefined
                    clip: true
                    boundsBehavior: Flickable.StopAtBounds
                    model: visible ? root.visual.archive : []
                    header: Text {
                        text: root.visual && root.visual.total !== undefined
                              ? qsTr("%1 entries").arg(root.visual.total) : ""
                        color: Tokens.foregroundDim
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontCaption
                        bottomPadding: Tokens.spaceSm
                    }
                    delegate: Text {
                        required property var modelData
                        width: ListView.view.width
                        text: modelData
                        color: Tokens.foreground
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSmall
                        textFormat: Text.PlainText
                        elide: Text.ElideMiddle
                    }
                }

                // The sandboxed helper could not render it (a damaged or
                // disguised file): say so rather than show an empty frame.
                Text {
                    textFormat: Text.PlainText
                    anchors.fill: parent
                    visible: root.visual && root.visual.why !== undefined && root.visual.audio === undefined
                    text: visible ? qsTr("Can't preview: %1").arg(root.visual.why) : ""
                    color: Tokens.foregroundDim
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontSmall
                    wrapMode: Text.WordWrap
                }

                Text {
                    textFormat: Text.PlainText
                    anchors.fill: parent
                    visible: root.info && root.info.noPreview !== "" && !root.visual
                    text: root.info ? root.info.noPreview : ""
                    color: Tokens.foregroundDim
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontSmall
                    wrapMode: Text.WordWrap
                }

                Flickable {
                    id: textFlick
                    WheelScroll { view: textFlick }
                    anchors.fill: parent
                    visible: root.info && root.info.lines
                             && root.info.lines.length > 0 && !root.visual
                    clip: true
                    contentHeight: peekText.implicitHeight
                    contentWidth: width
                    boundsBehavior: Flickable.StopAtBounds

                    Text {
                        id: peekText
                        width: parent.width
                        // Already sanitised in Rust: control characters were
                        // replaced before this string existed.
                        text: root.info && root.info.lines
                              ? root.info.lines.join("\n") : ""
                        color: Tokens.foreground
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSmall
                        textFormat: Text.PlainText   // never rich text
                        wrapMode: Text.NoWrap
                    }
                }
            }

            Text {
                textFormat: Text.PlainText
                id: qlTrunc
                visible: text !== ""
                width: parent.width
                text: root.info && root.info.truncated
                      ? qsTr("Showing the beginning of the file")
                      : ""
                color: Tokens.foregroundDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontCaption
            }

            Text {
                textFormat: Text.PlainText
                id: qlClose
                width: parent.width
                horizontalAlignment: Text.AlignRight
                text: [root.hashable && root.hash === "" ? qsTr("h sha-256") : "",
                       visualImage.visible ? qsTr("↑ ↓ zoom") : "",
                       root.programSeen ? qsTr("x allow") : "",
                       root.program !== "" ? qsTr("Enter run") : "",
                       qsTr("Esc close")].filter(t => t !== "").join("  ·  ")
                color: Tokens.foregroundDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontCaption
            }
        }
    }

    Accessible.role: Accessible.Dialog
    Accessible.name: root.info ? qsTr("Quick Look: ") + root.info.name : ""
}
