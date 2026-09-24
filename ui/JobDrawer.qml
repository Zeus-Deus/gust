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
        if (root.job.stopping)
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
                id: messageText
                anchors.left: stateText.right
                anchors.right: detailText.left
                anchors.rightMargin: Tokens.spaceLg
                anchors.verticalCenter: parent.verticalCenter
                // While stopping, say what it is waiting for: removing the
                // partial copy can take a while on a slow drive, and a card
                // that still says "Copying…" invites clicking Cancel again.
                text: !root.active ? ""
                      : root.job.stopping ? qsTr("Removing the partial copy — waiting for the drive to finish writing…")
                      : root.job.message
                color: Tokens.foreground
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
                elide: Text.ElideMiddle
            }

            // Bytes · speed · time left — the facts that answer "how long?".
            Text {
                id: detailText
                objectName: "jobDetail"
                anchors.right: cancelButton.visible ? cancelButton.left : parent.right
                anchors.rightMargin: cancelButton.visible ? Tokens.spaceMd : 0
                anchors.verticalCenter: parent.verticalCenter
                text: {
                    if (!root.active)
                        return ""
                    var parts = []
                    // Bytes when there are any worth counting; a batch of
                    // empty files or folders is better told in items.
                    if (root.job.stopping)
                        return ""
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

            Rectangle {
                id: cancelButton
                objectName: "jobCancel"
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                width: cancelText.implicitWidth + Tokens.spaceLg
                height: Tokens.controlHeight
                visible: root.active && root.job.state === "Running" && !root.job.stopping
                color: cancelArea.containsMouse ? Tokens.danger : "transparent"
                border.color: cancelArea.containsMouse ? Tokens.danger : Tokens.border
                border.width: Math.max(1, Tokens.borderWidth)
                radius: Tokens.radius
                Text {
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
