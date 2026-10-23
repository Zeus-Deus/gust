import QtQuick
import QtTest

// The actual InterruptedNotice (copied beside this file by
// crates/app/tests/work_guard_regressions.py); theme tokens are stubbed.
TestCase {
    name: "InterruptedNotice"
    width: 900
    height: 300
    when: windowShown

    InterruptedNotice { id: notice; width: 900 }

    function guidance() { return findChild(notice, "interruptedGuidance") }

    function cases() {
        return [
            // A remote stage kept on purpose because its place was away.
            {tag: "retained remote evidence",
             items: [{id: "a", text: "Copying payload.bin to share — nothing had finished", dest: "/places/nas/share"}]},
            // An ordinary local job whose leftovers a revisit may sweep.
            {tag: "ordinary local job",
             items: [{id: "b", text: "Moving photos and 3 more to Backup — 2 of 4 had finished", dest: "/local/Backup"},
                     {id: "c", text: "Copying a.txt — nothing had finished", dest: ""}]},
        ]
    }

    function test_never_claims_a_cleanup_it_cannot_know_data() { return cases() }
    function test_never_claims_a_cleanup_it_cannot_know(data) {
        notice.items = data.items
        verify(notice.shown)
        const text = guidance().text
        verify(text.indexOf("cleaned") < 0, text)
        verify(text.indexOf("temporary files") >= 0, text)
        verify(text.indexOf("already be complete") >= 0, text)
        verify(text.indexOf("Check the destination before running it again") >= 0, text)
        // Nothing here offers to resume or replay the job.
        verify(!/resum|retry|continu/i.test(text), text)
    }

    function test_guidance_wraps_instead_of_running_under_dismiss() {
        notice.items = cases()[0].items
        notice.width = 360
        wait(0)
        const g = guidance()
        verify(g.lineCount > 1, "a narrow window wraps the guidance")
        verify(g.x + g.width <= notice.width, "the text stays left of Dismiss")
        notice.width = 900
    }
}
