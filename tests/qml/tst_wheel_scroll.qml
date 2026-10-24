import QtQuick
import QtTest

TestCase {
    name: "WheelScroll"
    when: windowShown
    visible: true
    width: 800
    height: 1400

    Flickable {
        id: view
        width: 800
        height: 700
        contentHeight: 100000
        boundsBehavior: Flickable.StopAtBounds
        WheelScroll { id: wheel; view: view }
    }

    function init() {
        wheel.scrollAnimation.stop()
        wheel.enabled = true
        view.height = 700
        view.contentHeight = 100000
        view.contentY = view.originY
        wait(20)
    }

    function turn(delta, delay = 1, modifiers = Qt.NoModifier) {
        mouseWheel(view, 300, 200, 0, delta, Qt.NoButton, modifiers, delay)
    }

    function settle() {
        tryCompare(wheel.scrollAnimation, "running", false)
    }

    function test_distance_data() {
        return [
            {tag: "classic-slow", n: 5, delta: -120, delay: 50, height: 700},
            {tag: "classic-burst", n: 5, delta: -120, delay: 1, height: 700},
            {tag: "hires-slow", n: 40, delta: -15, delay: 6, height: 700},
            {tag: "hires-burst", n: 40, delta: -15, delay: 1, height: 700},
            {tag: "long-free-spin", n: 800, delta: -15, delay: 1, height: 700},
            {tag: "small-window", n: 5, delta: -120, delay: 1, height: 400},
            {tag: "tall-window", n: 5, delta: -120, delay: 1, height: 1300}
        ]
    }

    function test_distance(data) {
        view.height = data.height
        for (let i = 0; i < data.n; ++i)
            turn(data.delta, data.delay)
        settle()
        const expected = -data.n * data.delta / 120 * Tokens.rowHeight * 3
        fuzzyCompare(view.contentY - view.originY, expected, 0.01)
    }

    function test_movement_is_smoothed_and_finishes_without_coasting() {
        turn(-120)
        verify(wheel.scrollAnimation.running)
        verify(view.contentY < Tokens.rowHeight * 3)
        tryVerify(() => view.contentY > 0)
        settle()
        const position = view.contentY
        wait(150)
        compare(view.contentY, position)
    }

    function test_reversal_discards_pending_forward_movement() {
        view.contentY = 1000
        for (let i = 0; i < 10; ++i) turn(-120)
        verify(wheel.scrollAnimation.to > view.contentY)
        const position = view.contentY
        turn(120)
        const reversedFrom = wheel.scrollAnimation.from
        verify(reversedFrom >= position)
        verify(wheel.scrollAnimation.to < reversedFrom)
        settle()
        fuzzyCompare(view.contentY, reversedFrom - Tokens.rowHeight * 3, 0.01)
    }

    function test_external_navigation_cancels_pending_movement() {
        turn(-120)
        view.contentY = 2000
        verify(!wheel.scrollAnimation.running)
        wait(150)
        compare(view.contentY, 2000)
        turn(-120)
        settle()
        fuzzyCompare(view.contentY, 2000 + Tokens.rowHeight * 3, 0.01)
    }

    function test_bounds_and_immediate_reversal() {
        view.contentY = view.contentHeight - view.height - 10
        for (let i = 0; i < 10; ++i) turn(-120)
        settle()
        compare(view.contentY, view.contentHeight - view.height)
        turn(120)
        settle()
        fuzzyCompare(view.contentY, view.contentHeight - view.height - Tokens.rowHeight * 3, 0.01)
        view.contentY = 10
        turn(120)
        settle()
        compare(view.contentY, view.originY)
    }

    function test_short_content_stays_at_origin() {
        view.contentHeight = 100
        turn(-120)
        settle()
        compare(view.contentY, view.originY)
    }

    function test_resize_and_disable_cancel_pending_movement() {
        turn(-120)
        view.height = 400
        verify(!wheel.scrollAnimation.running)
        turn(-120)
        wheel.enabled = false
        verify(!wheel.scrollAnimation.running)
    }

    function test_modified_wheel_bypasses_handler() {
        turn(-120, 1, Qt.ControlModifier)
        verify(!wheel.scrollAnimation.running)
    }
}
