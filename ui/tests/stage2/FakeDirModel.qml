import QtQuick
import QtQml.Models

// A bounded stand-in for the bridge's DirModel in Stage 2 view tests.
// A SortFilterProxyModel so the views see a real QAbstractItemModel: swapping
// `model` emits genuine modelAboutToBeReset/modelReset. publish() follows the
// order DESIGN §2.2.1 gives Rust: reset, announce the cursor even if unchanged,
// then rows_replaced(keep) while published_row() answers the prepared map.
// Every invokable is recorded; calls made while a reset is in progress are
// also kept apart, so a test can prove the views made none.
SortFilterProxyModel {
    id: fake
    // Never without a source: Qt 6.12's proxy crashes in rowCount() when a
    // view completes against one that has none.
    model: ListModel {}

    property int cursor: 0
    property string location: "/fixture"
    property int count: 0
    property string filter: ""
    property string shown_filter: ""
    property bool sort_pending: false
    property bool filter_pending: false
    property bool selection_pending: false
    property string empty_reason: ""
    property string sort_field: "name"
    property bool sort_ascending: true
    property string columns_side: ""
    property bool show_hidden: false
    property string places_json: ""

    /// Displayed rows currently selected.
    property var selected: []
    /// When true, sel_only behaves like a selection waiting for shared
    /// inputs: selection_pending rises and the change lands on
    /// ackSelection(), as the bridge's bounded selection intent does.
    property bool selectionBarrier: false
    property var pendingSelection: null
    function ackSelection(refused) {
        if (!refused && fake.pendingSelection !== null) fake.selected = fake.pendingSelection
        fake.pendingSelection = null
        fake.selection_pending = false
    }

    /// Old row -> new row for the publication in progress.
    property var mapping: ({})
    /// Revision a columns_navigate must carry to be admitted.
    property string admittedRevision: ""
    property bool resetting: false
    property var calls: []
    property var resetCalls: []
    property var navigations: []
    property var names: []
    /// Raw path tokens per displayed row ("" = gone) and of the folder shown.
    property var tokens: []
    property var dirs: []
    property string locationToken: "local:2f66697874757265"

    signal rows_replaced(int keep)

    // Disabled until publishSort(): rows start in source order.
    sorters: [ RoleSorter { id: byName; roleName: "name"; enabled: false } ]

    onModelReset: resetting = false
    onLayoutChanged: resetting = false

    function note(name, args) {
        const entry = [name].concat(args || [])
        calls.push(entry)
        if (resetting) resetCalls.push(entry)
    }
    function clearCalls() {
        calls = []
        resetCalls = []
        navigations = []
    }

    /// Replace the rows the way Rust publishes a projection.
    function publish(source, newCursor, keep, map) {
        fake.mapping = map || ({})
        fake.resetting = true
        fake.model = source
        fake.count = source.count
        fake.names = fake.namesOf(source)
        fake.tokens = fake.names.map(fake.tokenOf)
        fake.cursor = newCursor
        fake.cursorChanged()
        fake.rows_replaced(keep)
    }
    function show(source, newCursor) {
        fake.model = source
        fake.count = source.count
        fake.names = fake.namesOf(source)
        fake.tokens = fake.names.map(fake.tokenOf)
        fake.cursor = newCursor
    }
    /// Re-sort as a layout change (persistent indexes remapped by Qt), then
    /// announce the cursor (its file's new row, or `chosenCursor` when the
    /// publication deliberately moves it) and emit rows_replaced(2) with the
    /// permutation as the published mapping.
    function publishSort(descending, chosenCursor) {
        const before = fake.shownNames()
        fake.resetting = true
        byName.sortOrder = descending ? Qt.DescendingOrder : Qt.AscendingOrder
        byName.enabled = true
        const after = fake.shownNames()
        const at = {}
        after.forEach((n, i) => { at[n] = i })
        const map = {}
        before.forEach((n, i) => { map[i] = at[n] })
        fake.mapping = map
        fake.names = after
        fake.tokens = after.map(fake.tokenOf)
        fake.cursor = chosenCursor === undefined ? map[fake.cursor] : chosenCursor
        fake.cursorChanged()
        fake.rows_replaced(2)
    }
    /// Names in displayed order; role 0 is the fixture's first role, `name`.
    function shownNames() {
        const out = []
        for (let i = 0; i < fake.rowCount(); i++) out.push(fake.data(fake.index(i, 0), 0))
        return out
    }
    function namesOf(source) {
        const out = []
        for (let i = 0; i < source.count; i++) out.push(source.get(i).name)
        return out
    }

    function published_row(old) {
        note("published_row", [old])
        const n = fake.mapping[old]
        return n === undefined ? -1 : n
    }
    function want_rows(first, n) { note("want_rows", [first, n]) }
    function request_columns() { note("request_columns") }
    function entry_name(i) { note("entry_name", [i]); return fake.names[i] || "" }
    function entry_icon_key(i) { note("entry_icon_key", [i]); return "" }
    function entry_emblems(i) { note("entry_emblems", [i]); return "" }
    function entry_path(i) { note("entry_path", [i]); return fake.location + "/" + (fake.names[i] || "") }
    function is_dir(i) { note("is_dir", [i]); return !!fake.dirs[i] }
    function entry_token(i) { note("entry_token", [i]); return fake.tokens[i] || "" }
    function location_token() { note("location_token"); return fake.locationToken }
    function sel_contains(i) { note("sel_contains", [i]); return fake.selected.indexOf(i) >= 0 }
    function sel_count() { return fake.selected.length }
    function sel_only(i) {
        note("sel_only", [i])
        if (!fake.selectionBarrier) { fake.selected = [i]; return }
        fake.pendingSelection = [i]
        fake.selection_pending = true
    }
    function sel_toggle(i) { note("sel_toggle", [i]) }
    function sel_range_to(i) { note("sel_range_to", [i]) }
    function sel_clear() { note("sel_clear") }
    function focus_row(i) { note("focus_row", [i]); fake.cursor = i }
    function set_sort(f) { note("set_sort", [f]) }
    function navigate(path) { note("navigate", [path]) }
    function navigate_token(token) { note("navigate_token", [token]) }
    function columns_navigate(token, revision) {
        note("columns_navigate", [token, revision])
        const ok = revision !== "" && revision === fake.admittedRevision
        if (ok) fake.navigations.push(token)
        return ok
    }
    /// A waiting publication (a function that publishes real rows: reset or
    /// layout). Like the bridge, letting go of the live hold installs it
    /// synchronously, inside hold_live_updates(false).
    property var queuedRelease: null
    function hold_live_updates(on) {
        note("hold_live_updates", [on])
        if (!on && fake.queuedRelease) {
            const publish = fake.queuedRelease
            fake.queuedRelease = null
            publish()
        }
    }
    /// The raw token a displayed name stands for in these fixtures.
    function tokenOf(name) {
        let hex = ""
        for (let i = 0; i < name.length; i++) hex += name.charCodeAt(i).toString(16).padStart(2, "0")
        return "local:2f" + hex
    }
    // Targets are resolved from the rows displayed at the moment of the call.
    function clip_copy(row) { note("clip_copy", [row, fake.tokens[row] || "", fake.names[row] || ""]); return "Copied" }
    function run_prepare(row) {
        note("run_prepare", [row, fake.tokens[row] || ""])
        return JSON.stringify({ token: fake.tokens[row] || "", name: fake.names[row] || "", state: "run" })
    }
    /// Selected entries by name, for the bulk rename recorder.
    property var selectedNames: []
    function bulk_check(row, text) { return JSON.stringify({ ok: true, summary: "", changing: 2, lines: [] }) }
    /// Pairs the edited lines with the selected files in DISPLAYED order, as
    /// the bridge does, and records old name -> new name.
    function bulk_apply_names(row, text) {
        const order = fake.names.filter(n => fake.selectedNames.indexOf(n) >= 0)
        const lines = text.split("\n")
        const map = {}
        order.forEach((n, i) => { map[n] = lines[i] })
        note("bulk_apply_names", [JSON.stringify(map)])
        return ""
    }
    function bulk_preview(row, pattern) {
        const order = fake.names.filter(n => fake.selectedNames.indexOf(n) >= 0)
        note("bulk_preview", [order.join(",")])
        return JSON.stringify({ ok: true, summary: "Rename " + order.length, changing: order.length, problems: [],
                                rows: order.map(n => ({ from: n, to: n + "-1", unchanged: false })) })
    }
    function bulk_apply(row, pattern) {
        const order = fake.names.filter(n => fake.selectedNames.indexOf(n) >= 0)
        note("bulk_apply", [order.join(",")])
        return ""
    }
    // Like the bridge's drag_hold: from drag_begin to drag_end the payload is
    // kept and row-moving publications wait; drag_end lets them through.
    property bool dragHold: false
    property var queuedFilter: null
    function drag_begin(r) { note("drag_begin", [r]); fake.dragHold = true }
    function drag_end() {
        note("drag_end")
        fake.dragHold = false
        if (fake.queuedFilter !== null) { fake.shown_filter = fake.queuedFilter; fake.queuedFilter = null }
    }
    /// A finished filter: shown at once, or after the drag lets go.
    function publishFilter(q) {
        if (fake.dragHold) fake.queuedFilter = q
        else fake.shown_filter = q
    }
    function drop_on_row(r, asMove) { note("drop_on_row", [r, asMove, fake.dragHold]); return "" }
    function drop_link_on_row(r) { note("drop_link_on_row", [r, fake.dragHold]); return "" }
    function band_begin(additive) { note("band_begin", [additive]) }
    function band_update(r1, r2, c1, c2, columns) { note("band_update", [r1, r2]) }
    function drag_count() { return 1 }
    function drag_preview(r, mode) { note("drag_preview", [r, mode]); return "" }
    function drag_drop_on_row(r, mode) { note("drag_drop_on_row", [r, mode]); return "" }
    /// What dragging out to another application would carry.
    property string mimeJson: '{"uris":"file:///fixture/a0\r\n","text":"/fixture/a0","gnome":"copy\nfile:///fixture/a0"}'
    function drag_mime_json() { note("drag_mime_json"); return fake.mimeJson }
    // Destinations for Gust's own drag from the other pane: tokens only.
    function drag_preview_into_token(token, mode) { note("drag_preview_into_token", [token, mode]); return "Copy here" }
    function drag_drop_into_token(token, mode) { note("drag_drop_into_token", [token, mode]); return "" }
    // Display-text authorities: recorded so a test can prove they are unused.
    function drag_preview_into(path, mode) { note("drag_preview_into", [path, mode]); return "" }
    function drag_drop_into(path, mode) { note("drag_drop_into", [path, mode]); return "" }
    function drop_enter() { note("drop_enter") }
    function uri_preview(row, dest, uris, mode, fromGust) { note("uri_preview", [row, dest]); return "" }
    function uri_drop(row, dest, uris, mode, fromGust) { note("uri_drop", [row, dest]); return "" }
}
