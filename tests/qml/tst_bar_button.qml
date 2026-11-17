import QtQuick
import QtTest
import com.gust.app
import "@UI_URL@" as Gust

// The shipped toolbar button: a click acts, an unavailable button does not,
// and resting the pointer on it names it (with its key) inside the window.
TestCase {
    id: test
    name: "BarButton"
    when: windowShown
    visible: true
    width: 400
    height: 200

    property int presses: 0

    Gust.BarButton {
        id: button
        x: 368
        y: 4
        icon: "search"
        label: "Search"
        keys: "Ctrl+F"
        onClicked: test.presses++
    }

    function tip() {
        const window = button.Window.contentItem
        for (let i = 0; i < window.children.length; i++) {
            const c = window.children[i]
            if (c.z === 200) return c
        }
        return null
    }

    function init() {
        test.presses = 0
        button.available = true
        mouseMove(test, 10, 150)
        wait(20)
    }

    function test_a_click_acts_once() {
        mouseClick(button)
        compare(test.presses, 1)
    }

    function test_an_unavailable_button_does_nothing() {
        button.available = false
        mouseClick(button)
        compare(test.presses, 0)
    }

    function test_it_is_named_for_screen_readers() {
        compare(button.Accessible.name, "Search")
        compare(button.Accessible.description, "Ctrl+F")
    }

    function test_every_icon_has_a_plain_fallback() {
        const probe = Qt.createQmlObject('import "@UI_URL@" as Gust; Gust.Glyph {}', test)
        for (const name in probe.icons)
            verify((probe.plain[name] || "") !== "", name + " has no fallback character")
        probe.destroy()
    }

    function test_resting_on_it_shows_its_name_inside_the_window() {
        verify(tip() !== null)
        verify(!tip().visible)
        mouseMove(button, button.width / 2, button.height / 2)
        tryCompare(tip(), "visible", true, 2000)
        const t = tip()
        // The button sits at the right edge: the tip is pulled back in.
        verify(t.x >= 0 && t.x + t.width <= test.width, "tip leaves the window: " + t.x)
        verify(t.y >= button.y + button.height, "tip covers its own button")
        mouseMove(test, 10, 150)
        tryCompare(tip(), "visible", false, 1000)
    }

    function test_a_press_dismisses_the_tip() {
        mouseMove(button, button.width / 2, button.height / 2)
        tryCompare(tip(), "visible", true, 2000)
        mousePress(button)
        compare(tip().visible, false)
        mouseRelease(button)
    }
}
