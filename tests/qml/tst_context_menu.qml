import QtQuick
import QtTest
import com.gust.app
import "@UI_URL@" as Gust

// The shipped right-click menu in a window too short for it: it must stay
// inside the window, scroll, and keep the entry the keyboard is on in view.
TestCase {
    id: test
    name: "ContextMenuFits"
    when: windowShown
    visible: true
    width: 500
    height: 200

    Gust.ContextMenu { id: menu }
    SignalSpy { id: chosen; target: menu; signalName: "chosen" }

    function entries(n) {
        const list = []
        for (let i = 0; i < n; i++) {
            if (i > 0 && i % 5 === 0) list.push({ separator: true })
            list.push({ label: "Entry " + i, hint: "", action: "act-" + i })
        }
        list.push({ separator: true })
        list.push({ label: "Move to Trash", hint: "Delete", action: "trash", danger: true })
        return list
    }
    function frame() { return findChild(menu, "contextMenu") }
    function scroller() { return findChild(menu, "contextMenuScroller") }
    // Top and bottom of entry `i` inside the frame, as drawn now.
    function span(i) {
        let top = menu.padY
        for (let k = 0; k < i; k++) top += menu.items[k].separator ? menu.sepH : menu.rowH
        return [top - scroller().contentY, top + menu.rowH - scroller().contentY]
    }

    function cleanup() { menu.close(); chosen.clear() }

    function test_a_menu_that_fits_is_not_capped_and_does_not_scroll() {
        menu.popup(10, 10, entries(3))
        verify(!frame().scrolls)
        compare(frame().height, frame().fullHeight)
        verify(!scroller().interactive)
    }

    function test_a_long_menu_stays_inside_a_short_window() {
        menu.popup(10, 150, entries(19))
        const f = frame()
        verify(f.fullHeight > test.height, "the fixture menu must be taller than the window")
        verify(f.scrolls)
        verify(f.y >= 0, "top edge above the window: " + f.y)
        verify(f.y + f.height <= test.height, "bottom edge below the window: " + (f.y + f.height))
        verify(scroller().interactive)
    }

    function test_keyboard_travel_brings_the_last_entry_into_view_and_runs_it() {
        menu.popup(10, 10, entries(19))
        keyClick(Qt.Key_End)
        const last = menu.items.length - 1
        compare(menu.current, last)
        const s = span(last)
        verify(s[0] >= 0 && s[1] <= frame().height, "Move to Trash is outside the frame: " + s)
        keyClick(Qt.Key_Return)
        compare(chosen.count, 1)
        compare(chosen.signalArguments[0][0], "trash")
    }

    function test_wheeling_past_the_end_keeps_the_last_action_clickable() {
        menu.popup(10, 150, entries(19))
        keyClick(Qt.Key_End)
        waitForRendering(menu)
        const last = menu.itemPoints()["Move to Trash"]
        verify(last[1] > 0 && last[1] < test.height, "last row at " + last)
        mouseWheel(test, last[0], last[1], 0, -120)
        verify(menu.open, "a wheel tick inside the frame must not dismiss it")
        tryCompare(scroller(), "moving", false)
        mouseClick(test, last[0], last[1])
        compare(chosen.count, 1)
        compare(chosen.signalArguments[0][0], "trash")

        menu.popup(10, 150, entries(19))
        mouseWheel(test, test.width - 1, test.height / 2, 0, -120)
        verify(!menu.open, "a wheel tick outside still dismisses the menu")
    }

    function test_travelling_back_up_shows_the_first_entry_again() {
        menu.popup(10, 10, entries(19))
        keyClick(Qt.Key_End)
        verify(scroller().contentY > 0)
        keyClick(Qt.Key_Home)
        compare(menu.current, 0)
        const s = span(0)
        verify(s[0] >= 0 && s[1] <= frame().height, "the first entry is outside the frame: " + s)
    }

    function test_every_step_down_keeps_the_current_entry_visible() {
        menu.popup(10, 10, entries(19))
        for (let n = 0; n < menu.items.length; n++) {
            keyClick(Qt.Key_Down)
            const s = span(menu.current)
            verify(s[0] >= -1 && s[1] <= frame().height + 1,
                   "entry " + menu.current + " is outside the frame: " + s)
        }
    }

    function test_a_reopened_menu_starts_at_the_top() {
        menu.popup(10, 10, entries(19))
        keyClick(Qt.Key_End)
        menu.close()
        menu.popup(10, 10, entries(19))
        compare(scroller().contentY, 0)
    }
}
