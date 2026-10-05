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

    signal closed()
    /// The row shown, so h can hash it.
    property int row: -1

    /// The model, so the panel can ask for and receive its visual preview.
    property var model: null
    /// Image path or archive listing, rendered on a worker (sandboxed).
    readonly property var visual: {
        const s = root.model && root.open ? root.model.peek_visual : ""
        if (!s) return null
        try { return JSON.parse(s) } catch (e) { return null }
    }

    function show(json, row) {
        try {
            root.info = JSON.parse(json)
        } catch (e) {
            root.info = null
        }
        root.open = root.info !== null && root.info.ok
        root.row = row === undefined ? -1 : row
        root.audio = null
        if (root.open && root.model && row !== undefined) root.model.request_peek_visual(row)
    }

    // --- PDF pages --------------------------------------------------------
    readonly property int page: root.visual && root.visual.page ? root.visual.page : 0
    readonly property int pages: root.visual && root.visual.pages ? root.visual.pages : 0
    /// Turn the page by `d` (Left/Right, PageUp/PageDown). False when this
    /// is not a document with pages, so the key does what it always does.
    function turnPage(d) {
        if (!root.open || root.pages < 2 || !root.model) return false
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
    /// Left / Right: five seconds back or forward.
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
    /// Drawn larger than rendered means it was stretched, i.e. blurry.
    function imageInfo() {
        if (!visualImage.visible || visualImage.status !== Image.Ready) return null
        return { rendered: [visualImage.implicitWidth, visualImage.implicitHeight],
                 drawn: [Math.round(visualImage.paintedWidth), Math.round(visualImage.paintedHeight)] }
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
        width: Math.min(parent.width - Tokens.spaceXl * 4,
                        Math.round(Tokens.rowHeight * 26))
        // As tall as it needs: a folder or a binary has no preview, and a
        // mostly-empty panel reads as "something failed to load".
        readonly property bool hasPreview: root.info !== null
            && ((root.info.lines && root.info.lines.length > 0) || root.info.noPreview !== ""
                || root.visual !== null)
        height: hasPreview
                ? Math.min(parent.height - Tokens.spaceXl * 4, Math.round(Tokens.rowHeight * 24))
                : Math.min(parent.height - Tokens.spaceXl * 4,
                           Tokens.spaceXl * 2 + Tokens.rowHeight * 3 + Tokens.spaceLg * 4
                           + (factsGrid.visible ? factsGrid.height : 0))
        color: Tokens.backgroundDark
        border.width: Math.max(1, Tokens.borderWidth)
        border.color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
        radius: Tokens.radius

        Column {
            anchors.fill: parent
            anchors.margins: Tokens.spaceXl
            spacing: Tokens.spaceLg

            Text {
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
                visible: root.info && root.info.kind !== "folder"
                text: root.hash === "" ? qsTr("SHA-256  ·  press h to compute")
                    : root.hash === "…" ? qsTr("SHA-256  ·  reading…  (Esc stops)")
                    : qsTr("SHA-256  ") + root.hash
                color: root.hash.length === 64 ? Tokens.foreground : Tokens.foregroundDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
                wrapMode: Text.WrapAnywhere
                textFormat: Text.PlainText
            }

            Rectangle {
                width: parent.width
                height: Math.max(1, Tokens.borderWidth)
                color: Tokens.withAlpha(Tokens.border, Tokens.borderAlpha)
            }

            // Either a bounded text peek, or an honest sentence about why
            // there is nothing to show. "No preview" beats a preview that
            // required running the file.
            Item {
                visible: panel.hasPreview
                width: parent.width
                // Everything else in the column, plus one spacing per gap.
                height: panel.height - Tokens.spaceXl * 2
                        - qlName.height - qlKind.height - qlClose.height
                        - (factsGrid.visible ? factsGrid.height + parent.spacing : 0)
                        - (qlHash.visible ? qlHash.height + parent.spacing : 0)
                        - (qlTrunc.visible ? qlTrunc.height + parent.spacing : 0)
                        - Math.max(1, Tokens.borderWidth) - parent.spacing * 4

                // Image, PDF first page or video frame: a PNG the sandboxed
                // helper made. Only this PNG is decoded here, never the file.
                //
                // Never larger than the render itself: stretching a small
                // image past its own pixels is what made previews blurry.
                // Larger renders are scaled DOWN to fit, with mipmaps.
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
                // desktop's own player. p pauses, Left / Right skip.
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
                              : qsTr("p play / pause  ·  ← → 5 seconds  ·  Enter opens your music player")
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
                    anchors.fill: parent
                    visible: root.visual && root.visual.why !== undefined && root.visual.audio === undefined
                    text: visible ? qsTr("No preview: the file could not be read as what its name says (%1)").arg(root.visual.why) : ""
                    color: Tokens.foregroundDim
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontSmall
                    wrapMode: Text.WordWrap
                }

                Text {
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
                id: qlClose
                width: parent.width
                horizontalAlignment: Text.AlignRight
                text: qsTr("Space or Escape to close")
                color: Tokens.foregroundDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontCaption
            }
        }
    }

    Accessible.role: Accessible.Dialog
    Accessible.name: root.info ? qsTr("Quick Look: ") + root.info.name : ""
}
