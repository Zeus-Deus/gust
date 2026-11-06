import QtQuick
import QtTest

// The actual SettingsPanel (copied beside this file by its runner); theme
// tokens are stubbed. The window binds `s`, `status` and `error` from the
// preferences worker's snapshot; here they are plain properties.
TestCase {
    id: test
    name: "SettingsPanel"
    width: 900
    height: 700
    visible: true
    when: windowShown

    property var snap: ({ ready: true, key_preset: "standard", density: "normal", show_hidden: false,
                          sidebar: true, restore_tabs: false, renderer: "auto", renderer_now: "gpu",
                          is_default: false, omarchy_keys: false, file_dialogs: false, is_omarchy: true })
    property string status: "saved"
    property string error: ""

    SettingsPanel {
        id: panel
        s: test.snap
        status: test.status
        error: test.error
    }

    // Takes the keyboard, as a confirmation's button does over the panel.
    Item { id: thief; width: 1; height: 1 }

    SignalSpy { id: changed; target: panel; signalName: "changed" }
    SignalSpy { id: retry; target: panel; signalName: "retryRequested" }
    SignalSpy { id: defaults; target: panel; signalName: "defaultRequested" }

    function init() {
        test.status = "saved"
        test.error = ""
        panel.busyKey = ""
        test.snap = Object.assign({}, test.snap, { density: "normal", is_default: false })
        changed.clear(); retry.clear(); defaults.clear()
        panel.show()
        waitForRendering(panel)
    }
    function cleanup() { panel.hide() }

    function rowIndex(key) { return panel.rows.findIndex(r => r.key === key) }
    function statusLine() { return findChild(panel, "settingsStatus") }
    function note(key) { return findChild(panel, "settingsNote-" + key).text }

    function test_status_says_saved_only_when_acknowledged() {
        const cases = [["loading", "", "Loading settings…"], ["saving", "", "Saving…"], ["saved", "", "Saved"],
                       ["unsaved", "Settings: permission denied", "Not saved: Settings: permission denied"],
                       ["problem", "Views: unreadable", "Problem: Views: unreadable"]]
        for (const [st, err, text] of cases) {
            test.status = st
            test.error = err
            compare(panel.statusText, text)
            compare(statusLine().text, text)
            compare(panel.failed, st === "unsaved" || st === "problem")
        }
    }

    function test_failure_stays_visible_across_unrelated_keys() {
        test.status = "unsaved"
        test.error = "Settings: permission denied"
        keyClick(Qt.Key_Down)
        keyClick(Qt.Key_Up)
        keyClick(Qt.Key_Down)
        verify(statusLine().text.indexOf("Not saved") === 0, statusLine().text)
        compare(retry.count, 0, "navigation is not a retry")
    }

    function test_r_retries_only_after_a_failure() {
        keyClick(Qt.Key_R)
        compare(retry.count, 0)
        test.status = "unsaved"
        test.error = "Settings: permission denied"
        keyClick(Qt.Key_R)
        compare(retry.count, 1)
        mouseClick(statusLine())
        compare(retry.count, 2, "the status line is a retry button while failed")
    }

    function test_nothing_is_chosen_before_the_first_load() {
        test.status = "loading"
        panel.current = rowIndex("density")
        keyClick(Qt.Key_Right)
        compare(changed.count, 0)
        test.status = "saved"
        keyClick(Qt.Key_Right)
        compare(changed.count, 1)
        compare(changed.signalArguments[0][0], "density")
        compare(changed.signalArguments[0][1], "roomy")
    }

    function test_a_published_snapshot_updates_rows_in_place() {
        const before = findChild(panel, "settingsStatus")
        test.snap = Object.assign({}, test.snap, { density: "roomy" })
        compare(panel.valueOf("density"), "roomy", "bound, not assigned in show()")
        compare(findChild(panel, "settingsStatus"), before)
        // Rows do not depend on the snapshot, so they are never rebuilt.
        compare(panel.rows.length, 9)
        const omarchy = findChild(panel, "settingsNote-omarchy_keys")
        compare(omarchy.text, "Reversible")
        test.snap = Object.assign({}, test.snap, { is_omarchy: false })
        compare(omarchy.text, "Needs Omarchy")
        compare(findChild(panel, "settingsNote-omarchy_keys"), omarchy, "the same row, updated in place")
        test.snap = Object.assign({}, test.snap, { renderer_now: "software" })
        compare(note("renderer"), "Next start. Now: software")
    }

    function test_an_integration_change_is_not_requested_twice() {
        panel.current = rowIndex("default")
        keyClick(Qt.Key_Right)
        compare(defaults.count, 0, "already the leftmost choice")
        keyClick(Qt.Key_Left)
        compare(defaults.count, 1)
        compare(defaults.signalArguments[0][0], true)
        // The window marks the row while the worker re-reads its state.
        panel.busyKey = "default"
        compare(note("default"), "Checking…")
        keyClick(Qt.Key_Left)
        keyClick(Qt.Key_Right)
        compare(defaults.count, 1, "no second request before the answer")
        panel.busyKey = ""
        test.snap = Object.assign({}, test.snap, { is_default: true })
        keyClick(Qt.Key_Right)
        compare(defaults.count, 2)
        compare(defaults.signalArguments[1][0], false)
    }

    function test_refocus_keeps_the_row_and_r_retries() {
        panel.current = rowIndex("density")
        test.status = "unsaved"
        test.error = "Settings: permission denied"
        thief.forceActiveFocus()
        keyClick(Qt.Key_R)
        compare(retry.count, 0, "the keyboard is elsewhere: the native defect")
        panel.refocus()
        compare(panel.current, rowIndex("density"), "the row the user was on")
        keyClick(Qt.Key_R)
        compare(retry.count, 1)
        keyClick(Qt.Key_Down)
        compare(panel.current, rowIndex("density") + 1, "arrow keys reach it too")
    }

    function test_refocus_does_nothing_when_closed() {
        panel.hide()
        thief.forceActiveFocus()
        panel.refocus()
        verify(thief.activeFocus, "a closed panel does not take the keyboard")
    }
}
