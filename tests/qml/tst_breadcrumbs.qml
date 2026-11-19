import QtQuick
import QtTest
import com.gust.app
import "@UI_URL@" as Gust

// The shipped path bar when the path is longer than the bar: the end stays
// in view, the cut is marked, and nothing under the mark takes a click or a
// drop (a drop there would land in a folder whose name cannot be read).
TestCase {
    id: test
    name: "BreadcrumbsOverflow"
    when: windowShown
    visible: true
    width: 420
    height: 80

    QtObject {
        id: tab
        property string location: "/home/sam"
        property var parts: ["Home"]
        function breadcrumbs() {
            const out = []
            let path = "/home/sam"
            for (let i = 0; i < parts.length; i++) {
                if (i > 0) path += "/" + parts[i]
                out.push(path + "\t" + parts[i])
            }
            return out
        }
        function go(list) { parts = list; location = "/home/sam/" + list.slice(1).join("/") }
    }

    Gust.Breadcrumbs {
        id: bar
        model: tab
        width: Math.min(implicitWidth, 300)
    }
    SignalSpy { id: chosen; target: bar; signalName: "pathChosen" }

    readonly property var deep: ["Home", "Documents", "Renovation 2026", "Kitchen",
                                 "Quotes and invoices", "Accepted offers", "Signed copies"]

    function collect(item, name, out) {
        for (let i = 0; i < item.children.length; i++) {
            const c = item.children[i]
            if (c.objectName === name) out.push(c)
            collect(c, name, out)
        }
        return out
    }
    // Every crumb's drop target, outermost folder first. A crumb wholly
    // under the mark has no path of its own, so order by where it sits.
    function drops() {
        return collect(bar, "crumbDrop", []).sort((a, b) => a.parent.mapToItem(bar, 0, 0).x
                                                           - b.parent.mapToItem(bar, 0, 0).x)
    }
    function cut() { return findChild(bar, "crumbCutBox") }
    // Left and right edge of a drop target, in the bar's coordinates.
    function span(d) {
        const p = d.mapToItem(bar, 0, 0)
        return [p.x, p.x + d.width]
    }

    function init() { tab.go(["Home"]); chosen.clear(); wait(0) }

    function test_a_short_path_is_whole_and_every_crumb_takes_a_drop() {
        tab.go(["Home", "Documents"])
        tryCompare(bar, "overflowing", false)
        verify(!cut().visible)
        const all = drops()
        compare(all.length, 2)
        for (const d of all) {
            verify(d.path !== "")
            compare(d.anchors.leftMargin, 0)
        }
    }

    function test_a_long_path_keeps_its_end_in_view_and_marks_the_cut() {
        tab.go(deep)
        tryCompare(bar, "overflowing", true)
        verify(cut().visible)
        compare(drops().length, deep.length)
        // The row slides left once its new width is known.
        tryVerify(() => {
            const all = drops()
            const last = span(all[all.length - 1])
            return last[1] - last[0] > 1 && last[0] >= 0 && last[1] <= bar.width + 1
        }, 2000, "the current folder is not whole and in view")
    }

    function test_nothing_under_the_cut_mark_takes_a_drop() {
        tab.go(deep)
        tryCompare(bar, "overflowing", true)
        tryVerify(() => drops().some(d => d.path === ""), 2000,
                  "the fixture path must push at least one crumb under the mark")
        const edge = cut().width
        let hidden = 0, live = 0
        for (const d of drops()) {
            if (d.path === "") { hidden++; continue }
            live++
            const s = span(d)
            verify(s[0] >= edge - 0.5, "a drop target reaches under the mark: " + s + " < " + edge)
        }
        verify(hidden > 0, "the fixture path must push at least one crumb under the mark")
        verify(live > 0, "the visible crumbs must still take a drop")
    }

    function test_a_click_on_the_cut_mark_goes_nowhere_and_a_visible_crumb_still_works() {
        tab.go(deep)
        tryCompare(bar, "overflowing", true)
        mouseClick(bar, cut().width / 2, bar.height / 2)
        compare(chosen.count, 0)
        const all = drops()
        const s = span(all[all.length - 2])
        mouseClick(bar, (s[0] + s[1]) / 2, bar.height / 2)
        compare(chosen.count, 1)
        compare(chosen.signalArguments[0][0], all[all.length - 2].path)
    }

    function test_returning_to_a_short_path_restores_every_drop_target() {
        tab.go(deep)
        tryCompare(bar, "overflowing", true)
        tab.go(["Home", "Documents"])
        tryCompare(bar, "overflowing", false)
        for (const d of drops()) verify(d.path !== "")
    }
}
