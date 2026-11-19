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

    function cleanup() {
        menu.close()
        chosen.clear()
        test.height = 200
    }

    function test_a_menu_that_fits_is_not_capped_and_does_not_scroll() {
        menu.popup(10, 10, entries(3))
        verify(!frame().scrolls)
        compare(frame().height, frame().fullHeight)
        verify(!scroller().interactive)
    }

    function test_action_and_shortcut_share_the_frame_when_resized() {
        const wide = test.width
        try {
            menu.popup(wide, 10, [{label: "Two panes side by side", hint: "Ctrl+Shift+Return", action: "split"}])
            const label = findChild(menu, "menuLabel")
            const shortcut = findChild(menu, "menuShortcut")
            const preferred = frame().width
            verify(preferred < wide - 2 * Tokens.spaceSm)
            verify(!label.truncated && !shortcut.truncated)
            test.width = 150
            tryCompare(frame(), "width", test.width - 2 * Tokens.spaceSm)
            compare(frame().x, Tokens.spaceSm)
            verify(label.width > 0 && shortcut.width > 0)
            verify(label.x >= 0 && label.x + label.width < shortcut.x)
            verify(shortcut.x + shortcut.width <= frame().width)
            tryCompare(label, "truncated", true)
            tryCompare(shortcut, "truncated", true)
            test.width = wide
            tryCompare(frame(), "width", preferred)
            tryCompare(label, "truncated", false)
            tryCompare(shortcut, "truncated", false)
        } finally {
            test.width = wide
        }
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

    function openTallMenu() {
        test.height = 700
        menu.popup(10, test.height - 20, entries(19))
        compare(frame().height, frame().fullHeight)
        verify(!scroller().interactive)
        verify(frame().y > Tokens.spaceSm)
    }

    function shrinkOpenMenu() {
        test.height = 320
        tryCompare(frame(), "height", test.height - 2 * Tokens.spaceSm)
        compare(frame().y, Tokens.spaceSm)
        verify(menu.open && scroller().interactive)
    }

    function test_resize_preserves_keyboard_selection_and_bounds_scroll() {
        openTallMenu()
        keyClick(Qt.Key_End)
        const last = menu.items.length - 1
        shrinkOpenMenu()
        tryVerify(() => span(last)[1] <= frame().height)
        compare(menu.current, last)
        for (const height of [450, 700]) {
            test.height = height
            tryVerify(() => scroller().contentY <= Math.max(0, scroller().contentHeight - scroller().height))
            verify(span(last)[0] >= 0 && span(last)[1] <= frame().height)
        }
        compare(scroller().contentY, 0)
        shrinkOpenMenu()
        keyClick(Qt.Key_Home)
        keyClick(Qt.Key_End)
        compare(menu.current, last)
        verify(span(last)[1] <= frame().height)
        keyClick(Qt.Key_Return)
        compare(chosen.count, 1)
        compare(chosen.signalArguments[0][0], "trash")
    }

    function test_parked_pointer_preserves_selection_when_the_menu_moves() {
        test.height = 700
        menu.popup(10, 120, entries(19))
        waitForRendering(menu)
        const first = menu.itemPoints()["Entry 0"]
        mouseMove(test, first[0], first[1])
        mouseMove(test, first[0] + 8, first[1])
        compare(menu.current, 0)
        keyClick(Qt.Key_Home)
        shrinkOpenMenu()
        waitForRendering(menu)
        compare(menu.current, 0)
        const second = menu.itemPoints()["Entry 1"]
        mouseMove(test, second[0], second[1])
        compare(menu.current, 1)
    }

    function test_resize_allows_wheeling_to_and_clicking_the_last_action() {
        openTallMenu()
        shrinkOpenMenu()
        compare(scroller().contentY, 0)
        const x = frame().x + frame().width / 2
        for (let i = 0; i < 12 && !scroller().atYEnd; i++) {
            const before = scroller().contentY
            mouseWheel(test, x, test.height / 2, 0, -120)
            tryVerify(() => scroller().contentY > before)
        }
        tryCompare(scroller(), "atYEnd", true)
        tryCompare(scroller(), "moving", false)
        const last = menu.itemPoints()["Move to Trash"]
        verify(last[1] > 0 && last[1] < test.height)
        mouseClick(test, last[0], last[1])
        compare(chosen.count, 1)
        compare(chosen.signalArguments[0][0], "trash")
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
