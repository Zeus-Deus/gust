import QtQuick
import QtTest

// The actual JobDrawer (copied beside this file by
// crates/app/tests/work_guard_regressions.py); theme tokens are stubbed.
TestCase {
    name: "JobDrawer"
    width: 900
    height: 120
    when: windowShown

    JobDrawer { id: drawer; width: 900 }

    function snapshot(extra) {
        return Object.assign({id: "j", state: "Running", done: 0, total: 1, bytes_done: 1048576,
                              bytes_total: 4194304, message: "Copying payload.bin", refused: 0,
                              bytes_text: "1 MB of 4 MB", speed: "", eta: "", stopping: false,
                              paused: false}, extra)
    }
    function message() { return findChild(drawer, "jobMessage").text }

    function test_a_requested_stop_is_not_shown_as_done_or_cleaned() {
        drawer.job = snapshot({stopping: true})
        compare(drawer.stateText, "Stopping")
        const text = message()
        verify(text.indexOf("Stop requested") === 0, text)
        verify(!/discard|remov|clean/i.test(text), text)
        verify(!findChild(drawer, "jobCancel").visible, "no second Cancel while stopping")
    }

    function test_the_end_of_a_stop_shows_the_job_outcome() {
        drawer.job = snapshot({stopping: true})
        drawer.job = snapshot({state: "Cancelled", message: "Cancelled — finished items were kept"})
        compare(drawer.stateText, "Cancelled")
        compare(message(), "Cancelled — finished items were kept")
        compare(drawer.stoppingFor, 0)
    }
}
