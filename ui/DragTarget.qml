import QtQuick
import com.gust.app

// A folder outside the file view that takes a drop: a sidebar place, a
// bookmark, a drive, a connected place or a path-bar crumb (DESIGN §6.1).
// Gust's own drag in this window carries the model it came from and goes
// through that model's planner. A drag from another Gust window goes through
// `model` (this window's) with the same rule: Move on the same drive, Copy
// onto another, Ctrl copies, Shift moves. Any other application's drag
// copies. Ctrl+Z undoes it.
DropArea {
    id: area
    /// The folder a drop lands in. "" takes nothing.
    property string path: ""
    /// This window's model, for drags from other windows and applications.
    /// (Not called `model`: inside a delegate that name is the row's data.)
    property var receiver: null
    /// What releasing here would do, or "" when the drag left.
    signal hover(string text)
    /// The drop happened: the model that ran it and "" or why it failed.
    signal landed(var model, string err)

    keys: ["text/uri-list"]
    property string uris: ""
    property bool fromGust: false
    property int mode: 0

    function sourceModel(d) {
        return d && d.source && d.source.objectName === "gustDragProxy" ? d.source.paneModel : null
    }
    function sourceMode(d) {
        return area.sourceModel(d) ? d.source.transferMode : 0
    }
    function read(d) {
        area.uris = d.hasUrls ? d.urls.map(u => u.toString()).join("\r\n") + "\r\n"
                              : (d.getDataAsString("text/uri-list") || "")
        const mark = d.formats.indexOf("application/x-gust-drag") >= 0
                     ? (d.getDataAsString("application/x-gust-drag") || "") : ""
        area.fromGust = mark.indexOf("gust\n") === 0
        area.mode = area.fromGust ? (parseInt(mark.split("\n")[1]) || 0) : 0
    }
    onEntered: (d) => {
        if (area.path === "") { d.accepted = false; return }
        const m = area.sourceModel(d)
        if (m) {
            d.accept(Qt.CopyAction)
            area.hover(m.drag_preview_into(area.path, area.sourceMode(d)))
            return
        }
        if (!area.receiver) { d.accepted = false; return }
        d.accept(Qt.CopyAction)
        area.receiver.drop_enter()
        area.read(d)
        area.hover(area.receiver.uri_preview(-1, area.path, area.uris, area.mode, area.fromGust))
    }
    onExited: area.hover("")
    onDropped: (d) => {
        if (area.path === "") return
        const m = area.sourceModel(d)
        d.accept(Qt.CopyAction)
        area.hover("")
        if (m) {
            area.landed(m, m.drag_drop_into(area.path, area.sourceMode(d)))
            return
        }
        if (!area.receiver) return
        area.read(d)
        area.landed(area.receiver, area.receiver.uri_drop(-1, area.path, area.uris, area.mode, area.fromGust))
    }

    Rectangle {
        anchors.fill: parent
        visible: area.containsDrag
        color: Tokens.withAlpha(Tokens.accent, 0.12)
        radius: Tokens.radius
        border.width: Math.max(2, Tokens.borderWidth * 2)
        border.color: Tokens.accent
    }
}
