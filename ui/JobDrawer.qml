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
    height: active ? Math.round(56 * Tokens.spacingScale) : 0
    Behavior on height { NumberAnimation { duration: Tokens.animFast } }

    color: Tokens.menuBackground
    border.color: Tokens.border
    border.width: Tokens.borderWidth
    clip: true

    function stateLabel() {
        if (!root.active)
            return ""
        switch (root.job.state) {
        case "Running":   return qsTr("Working")
        case "Confirmed": return qsTr("Done")
        case "Cancelled": return qsTr("Cancelled")
        case "Conflict":  return qsTr("Needs a decision")
        case "Failed":    return qsTr("Problem")
        default:          return root.job.state
        }
    }

    function stateColor() {
        if (!root.active)
            return Tokens.muted
        switch (root.job.state) {
        case "Confirmed": return Tokens.ok
        case "Failed":
        case "Conflict":  return Tokens.danger
        case "Cancelled": return Tokens.warn
        default:          return Tokens.accent
        }
    }

    Column {
        anchors.fill: parent
        anchors.margins: Tokens.spaceMd
        spacing: Tokens.spaceSm

        Row {
            width: parent.width
            spacing: Tokens.spaceMd

            Text {
                id: stateText
                text: root.stateLabel()
                color: root.stateColor()
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
                font.bold: true
                width: Math.round(110 * Tokens.spacingScale)
                elide: Text.ElideRight
            }

            Text {
                text: root.active ? root.job.message : ""
                color: Tokens.foreground
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
                elide: Text.ElideMiddle
                width: parent.width - stateText.width - cancelButton.width - Tokens.spaceMd * 2
            }

            Rectangle {
                id: cancelButton
                width: cancelText.implicitWidth + Tokens.spaceMd
                height: Tokens.controlHeight
                visible: root.active && root.job.state === "Running"
                color: cancelArea.containsMouse ? Tokens.danger : "transparent"
                border.color: Tokens.border
                border.width: Tokens.borderWidth
                radius: Tokens.radius
                Text {
                    id: cancelText
                    anchors.centerIn: parent
                    text: qsTr("Cancel")
                    color: cancelArea.containsMouse ? Tokens.selectedText : Tokens.muted
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontSmall
                }
                MouseArea {
                    id: cancelArea
                    anchors.fill: parent
                    hoverEnabled: true
                    onClicked: root.requestCancel()
                }
            }
        }

        // Progress track. An unknown total shows an indeterminate bar rather
        // than a fake percentage.
        Rectangle {
            width: parent.width
            height: Math.max(3, Math.round(5 * Tokens.spacingScale))
            color: Tokens.backgroundDark
            radius: Tokens.radius

            Rectangle {
                height: parent.height
                radius: parent.radius
                color: root.stateColor()
                width: {
                    if (!root.active || root.job.total <= 0)
                        return 0
                    var f = root.job.done / root.job.total
                    f = Math.max(0, Math.min(1, f))
                    // A started job shows a visible sliver rather than
                    // nothing, so "queued" never looks like "stuck".
                    return Math.max(f > 0 ? Tokens.spaceSm : 0, f * parent.width)
                }
                Behavior on width { NumberAnimation { duration: Tokens.animFast } }
            }
        }

        Text {
            text: {
                if (!root.active)
                    return ""
                var parts = []
                if (root.job.total > 0)
                    parts.push(root.job.done + " / " + root.job.total + qsTr(" items"))
                if (root.job.refused > 0)
                    parts.push(root.job.refused + qsTr(" could not be handled"))
                return parts.join("  ·  ")
            }
            color: Tokens.muted
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontSmall
        }
    }
}
