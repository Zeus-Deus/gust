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

    signal closed()

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
        if (root.open && root.model && row !== undefined) root.model.request_peek_visual(row)
    }
    function hide() {
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
                height: panel.height - Tokens.spaceXl * 2
                        - Tokens.rowHeight * 3 - Tokens.spaceLg * 4
                        - (factsGrid.visible ? factsGrid.height : 0)

                // Image, PDF first page or video frame: a PNG the sandboxed
                // helper made. Only this PNG is decoded here, never the file.
                Image {
                    id: visualImage
                    anchors.fill: parent
                    visible: root.visual && root.visual.image !== undefined
                    source: visible ? "file://" + root.visual.image : ""
                    fillMode: Image.PreserveAspectFit
                    asynchronous: true
                    cache: false
                    smooth: true
                }

                // Archive: what is inside, listed without extracting.
                ListView {
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
                    visible: root.visual && root.visual.why !== undefined
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
                width: parent.width
                text: root.info && root.info.truncated
                      ? qsTr("Showing the beginning of the file")
                      : ""
                color: Tokens.foregroundDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontCaption
            }

            Text {
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
