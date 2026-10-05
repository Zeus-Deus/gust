import QtQuick
import com.gust.app

// A folder outside the file view that takes Gust's own drag: a sidebar
// place, a bookmark, a drive or a path-bar crumb (DESIGN §6.1). The drag
// carries the model it came from; the drop goes through that model's
// planner and engine like a drop in the list: Move on the same drive,
// Copy onto another, Ctrl+Z undoes it. Drags from other applications are
// not taken here.
DropArea {
    id: area
    /// The folder a drop lands in. "" takes nothing.
    property string path: ""
    /// What releasing here would do, or "" when the drag left.
    signal hover(string text)
    /// The drop happened: the source model and "" or the reason it failed.
    signal landed(var model, string err)

    keys: ["text/uri-list"]

    function sourceModel(d) {
        return d && d.source && d.source.objectName === "gustDragProxy" ? d.source.paneModel : null
    }
    onEntered: (d) => {
        const m = area.sourceModel(d)
        if (!m || area.path === "") { d.accepted = false; return }
        d.accept(Qt.CopyAction)
        area.hover(m.drag_preview_into(area.path, 0))
    }
    onExited: area.hover("")
    onDropped: (d) => {
        const m = area.sourceModel(d)
        if (!m || area.path === "") return
        d.accept(Qt.CopyAction)
        area.hover("")
        area.landed(m, m.drag_drop_into(area.path, 0))
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
