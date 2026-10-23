import QtQuick
import com.gust.app

// Activity drawer: what is running, how far along, and how to stop it.
//
// The plan's rule is that a window close must not silently abandon pending
// transfers, so this stays visible until the job resolves and reports the
// outcome in words — including a partial one.
Rectangle {
    id: root

    property var model: null
    // Parsed snapshot from DirModel.poll_job().
    property var job: null
    property bool active: job !== null && job !== undefined

    signal requestCancel()
    signal requestPause()
    signal requestResume()

    /// Where the Pause/Resume button is, in window coordinates (qa).
    function pausePoint() {
        if (!pauseButton.visible) return null
        const p = pauseButton.mapToItem(null, pauseButton.width / 2, pauseButton.height / 2)
        return [Math.round(p.x), Math.round(p.y)]
    }
    readonly property bool paused: active && !!job.paused

    // When the stop was asked for. A stop waits for the read or write in
    // flight, which on a dropped connection can take many seconds; counting
    // the wait shows the card is alive without promising when it ends.
    property real stoppingSince: 0
    readonly property bool stopping: active && !!job.stopping
    onStoppingChanged: stoppingSince = stopping ? Date.now() : 0
    /// Whole seconds waited so far. Re-read on every poll, since each poll
    /// replaces `job`.
    readonly property int stoppingFor: {
        if (!root.stopping || root.stoppingSince <= 0 || !root.job)
            return 0
        return Math.floor((Date.now() - root.stoppingSince) / 1000)
    }

    visible: height > 0
    Behavior on height { NumberAnimation { duration: Tokens.animFast } }

    color: Tokens.menuBackground
    border.color: Tokens.border
    border.width: Tokens.borderWidth
    clip: true

    /// The job's state in words, as a BINDING for the same reason as
    /// stateColour: a function call would freeze at the first state.
    readonly property string stateText: {
        if (!root.active)
            return ""
        if (root.job.stopping)
            return qsTr("Stopping")
        if (root.job.paused)
            return qsTr("Paused")
        switch (root.job.state) {
        case "Running":   return qsTr("Working")
        case "Confirmed": return qsTr("Done")
        case "Cancelled": return qsTr("Cancelled")
        case "Conflict":  return qsTr("Needs a decision")
        case "Failed":    return qsTr("Problem")
        default:          return root.job.state
        }
    }


    /// The colour for the job's current state, as a BINDING. It was a
    /// function call, which QML evaluates once — so a job that later failed
    /// or was cancelled kept the colour it had when the drawer appeared.
    readonly property color stateColour: {
        if (!root.active)
            return Tokens.muted
        if (root.job.stopping || root.job.paused)
            return Tokens.warn
        switch (root.job.state) {
        case "Confirmed": return Tokens.ok
        case "Failed":
        case "Conflict":  return Tokens.danger
        case "Cancelled": return Tokens.warn
        default:          return Tokens.accent
        }
    }


    // One line of facts and a bar. The height follows the content, so a
    // larger font or spacing scale can never clip the detail line.
    height: active ? body.implicitHeight + Tokens.spaceMd * 2 : 0

    Column {
        id: body
        x: Tokens.spaceMd
        y: Tokens.spaceMd
        width: parent.width - Tokens.spaceMd * 2
        spacing: Tokens.spaceSm

        Item {
            width: parent.width
            height: Math.max(Tokens.controlHeight, stateText.implicitHeight)

            Text {
                textFormat: Text.PlainText
                id: stateText
                anchors.verticalCenter: parent.verticalCenter
                text: root.stateText
                color: root.stateColour
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
                font.bold: true
                width: Math.round(96 * Tokens.spacingScale)
                elide: Text.ElideRight
            }

            Text {
                textFormat: Text.PlainText
                id: messageText
                objectName: "jobMessage"
                anchors.left: stateText.right
                anchors.right: detailText.left
                anchors.rightMargin: Tokens.spaceLg
                anchors.verticalCenter: parent.verticalCenter
                // While stopping, say what it is waiting for. The stop is
                // REQUESTED, not done: the read or write in flight has to
                // return first, from a slow drive or a remote place that
                // stopped answering. Nothing is promised about the
                // unfinished item: a remote place may keep its temporary
                // files. A card that still said "Copying…" invited a second
                // Cancel; one that said "Cancelled" would be a lie.
                text: !root.active ? ""
                      : root.stopping ? qsTr("Stop requested — the current read or write must return before the transfer can stop")
                      : root.job.message
                color: Tokens.foreground
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
                elide: Text.ElideMiddle
            }

            // Bytes · speed · time left — the facts that answer "how long?".
            Text {
                textFormat: Text.PlainText
                id: detailText
                objectName: "jobDetail"
                anchors.right: pauseButton.visible ? pauseButton.left : (cancelButton.visible ? cancelButton.left : parent.right)
                anchors.rightMargin: cancelButton.visible ? Tokens.spaceMd : 0
                anchors.verticalCenter: parent.verticalCenter
                text: {
                    if (!root.active)
                        return ""
                    var parts = []
                    // Bytes when there are any worth counting; a batch of
                    // empty files or folders is better told in items.
                    if (root.stopping)
                        return root.stoppingFor >= 2 ? qsTr("waiting %1 s").arg(root.stoppingFor) : ""
                    if (root.job.paused)
                        return root.job.bytes_text ? root.job.bytes_text + qsTr("  ·  paused") : qsTr("paused")
                    if (root.job.bytes_text && root.job.state === "Running")
                        parts.push(root.job.bytes_text)
                    else if (root.job.total > 0)
                        parts.push(root.job.done + " / " + root.job.total + qsTr(" items"))
                    if (root.job.speed) parts.push(root.job.speed)
                    if (root.job.eta) parts.push(root.job.eta)
                    if (root.job.refused > 0)
                        parts.push(root.job.refused + qsTr(" could not be handled"))
                    return parts.join("  ·  ")
                }
                color: Tokens.muted
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
            }

            // Pause holds the copy between blocks; nothing is undone, and
            // Cancel still works while paused.
            Rectangle {
                id: pauseButton
                objectName: "jobPause"
                anchors.right: cancelButton.left
                anchors.rightMargin: Tokens.spaceMd
                anchors.verticalCenter: parent.verticalCenter
                width: pauseText.implicitWidth + Tokens.spaceXxl
                height: Tokens.controlHeight
                visible: cancelButton.visible
                color: pauseArea.containsMouse ? Tokens.withAlpha(Tokens.accent, 0.18) : "transparent"
                border.color: pauseArea.containsMouse ? Tokens.accent : Tokens.border
                border.width: Math.max(1, Tokens.borderWidth)
                radius: Tokens.radius
                Text {
                    textFormat: Text.PlainText
                    id: pauseText
                    anchors.centerIn: parent
                    text: root.paused ? qsTr("Resume") : qsTr("Pause")
                    color: Tokens.foreground
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontSmall
                }
                MouseArea {
                    id: pauseArea
                    anchors.fill: parent
                    hoverEnabled: true
                    onClicked: root.paused ? root.requestResume() : root.requestPause()
                }
                Accessible.role: Accessible.Button
                Accessible.name: root.paused ? qsTr("Resume") : qsTr("Pause")
            }

            Rectangle {
                id: cancelButton
                objectName: "jobCancel"
                anchors.right: parent.right
                anchors.rightMargin: Tokens.spaceSm
                anchors.verticalCenter: parent.verticalCenter
                width: cancelText.implicitWidth + Tokens.spaceXxl
                height: Tokens.controlHeight
                visible: root.active && root.job.state === "Running" && !root.job.stopping
                color: cancelArea.containsMouse ? Tokens.danger : "transparent"
                border.color: cancelArea.containsMouse ? Tokens.danger : Tokens.border
                border.width: Math.max(1, Tokens.borderWidth)
                radius: Tokens.radius
                Text {
                    textFormat: Text.PlainText
                    id: cancelText
                    anchors.centerIn: parent
                    text: qsTr("Cancel")
                    color: cancelArea.containsMouse ? Tokens.selectedText : Tokens.foreground
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontSmall
                }
                MouseArea {
                    id: cancelArea
                    anchors.fill: parent
                    hoverEnabled: true
                    onClicked: root.requestCancel()
                }
                Accessible.role: Accessible.Button
                Accessible.name: qsTr("Cancel")
            }
        }

        // Progress track. An unknown total shows no fake percentage.
        Rectangle {
            width: parent.width
            height: Math.max(3, Math.round(4 * Tokens.spacingScale))
            color: Tokens.backgroundDark
            radius: Tokens.radius

            Rectangle {
                height: parent.height
                radius: parent.radius
                color: root.stateColour
                width: {
                    if (!root.active || (root.job.total <= 0 && root.job.bytes_total <= 0))
                        return 0
                    // Bytes are the honest measure WHILE running: one 4 GB
                    // file among ten small ones is not 10 % of the work.
                    // Once it has ended, show what actually landed — a
                    // cancelled copy's partial file was removed, so the bar
                    // must not stay 90 % full.
                    var running = root.job.state === "Running"
                    var f = running && root.job.bytes_total > 0
                            ? root.job.bytes_done / root.job.bytes_total
                            : (root.job.total > 0 ? root.job.done / root.job.total : 0)
                    f = Math.max(0, Math.min(1, f))
                    // A started job shows a visible sliver rather than
                    // nothing, so "queued" never looks like "stuck".
                    return Math.max(f > 0 ? Tokens.spaceSm : 0, f * parent.width)
                }
                Behavior on width { NumberAnimation { duration: Tokens.animFast } }
            }
        }
    }
}
