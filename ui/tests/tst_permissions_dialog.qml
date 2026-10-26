import QtQuick
import QtTest
import com.gust.app

// The actual PermissionsDialog (copied beside this file by
// crates/app/tests/permissions_dialog_regressions.py). The model is a stand-in
// that records calls; theme tokens and FileIcon are stubs.
Item {
    id: top
    width: 900
    height: 760

    QtObject {
        id: fake
        signal permissions_ready(int token, string json)
        signal permissions_done(int token, string error, string message, bool partial)
        property string loadError: ""
        property int loads: 0
        property var applied: []
        property string applyError: ""
        function permissions_load() { loads += 1; return loadError }
        function permissions_apply(t, a, r) { applied.push([t, a, r]); return applyError }
    }

    PermissionsDialog {
        id: dialog
        property int finishedCount: 0
        property string lastMessage: ""
        property bool lastPartial: false
        property int closedCount: 0
        onFinished: (message, partial) => { finishedCount += 1; lastMessage = message; lastPartial = partial }
        onClosed: closedCount += 1
    }

    TestCase {
        name: "PermissionsDialog"
        when: windowShown

        function findChildrenNamed(item, name) {
            let out = []
            for (const c of item.children) {
                if (c.objectName === name) out.push(c)
                out = out.concat(findChildrenNamed(c, name))
            }
            if (item.contentItem && item.contentItem !== item && item.children.indexOf(item.contentItem) < 0)
                out = out.concat(findChildrenNamed(item.contentItem, name))
            return out
        }

        function facts(over) {
            const f = {
                ok: true, token: 1, total: 1, files: 1, dirs: 0, links: 0, missing: [],
                name: "launch.sh", place: "~/Projects",
                owner: { same: true, name: "alice", you: true },
                group: { same: true, name: "alice", member: true },
                owned: 1, special: 0,
                // rwxr-xr-x
                bits: [1, 1, 1, 1, 0, 1, 1, 0, 1]
            }
            for (const k in over) f[k] = over[k]
            return f
        }

        function openWith(f, tok) {
            fake.loadError = ""
            fake.applied = []
            fake.applyError = ""
            dialog.finishedCount = 0
            dialog.closedCount = 0
            dialog.lastMessage = ""
            dialog.lastPartial = false
            dialog.openFor(fake)
            verify(dialog.open)
            verify(dialog.loading, "the card says it is reading until the worker answers")
            fake.permissions_ready(tok || 1, JSON.stringify(f))
            verify(!dialog.loading)
            wait(0)
        }

        function cleanup() {
            if (dialog.open) { dialog.busy = false; dialog.close() }
            top.width = 900
            top.height = 760
            Tokens.fontBase = 12
            Tokens.spacingScale = 1
        }

        /// `it`'s rectangle in the card's coordinates.
        function inCard(it) {
            const card = dialog.children.find(c => c.objectName === "permCard")
            return it.mapToItem(card, 0, 0, it.width, it.height)
        }

        function test_a_file_shows_its_real_bits_in_words() {
            openWith(facts({}))
            compare(dialog.kind, "file")
            compare(dialog.permLabels, ["Read", "Edit", "Run"])
            compare(dialog.orig, [2, 2, 2, 2, 0, 2, 2, 0, 2])
            compare(dialog.audiences[0].title, "You")
            compare(dialog.audiences[0].sub, "Owner · alice")
            compare(dialog.audiences[1].sub, "alice · includes you")
            compare(dialog.context, "launch.sh  ·  ~/Projects")
            verify(!dialog.changed)
            compare(dialog.statusText, "No changes")
            const shown = JSON.stringify(dialog.qaState())
            verify(!/755|rwx|u\+x|chmod/.test(shown), "no mode numbers or symbols: " + shown)
        }

        function test_apply_asks_only_for_the_changed_bit_and_reports() {
            openWith(facts({}), 4)
            dialog.toggle(7) // other users: Edit
            verify(dialog.changed)
            compare(dialog.addMask, 2)
            compare(dialog.removeMask, 0)
            verify(dialog.notices.some(n => n.text.indexOf("Other users will be able to change this file") === 0))
            dialog.apply()
            compare(fake.applied, [[4, 2, 0]])
            verify(dialog.busy)
            dialog.cancel()
            verify(dialog.open, "Cancel waits while the system is applying")
            fake.permissions_done(3, "", "", false)
            verify(dialog.open, "another read's answer is not this one")
            fake.permissions_done(4, "", "Permissions changed on 1 item  ·  Ctrl+Z puts them back", false)
            compare(dialog.finishedCount, 1)
            compare(dialog.lastMessage, "Permissions changed on 1 item  ·  Ctrl+Z puts them back")
            verify(!dialog.lastPartial)
            verify(!dialog.open)
            compare(dialog.closedCount, 1)
        }

        function test_a_partial_refusal_is_passed_on() {
            openWith(facts({}), 2)
            dialog.toggle(4)
            dialog.apply()
            fake.permissions_done(2, "", "Permissions changed on 1 item; 1 not changed: b: not allowed", true)
            compare(dialog.lastMessage, "Permissions changed on 1 item; 1 not changed: b: not allowed")
            verify(dialog.lastPartial)
        }

        function test_nothing_changed_stays_open_with_the_reason() {
            openWith(facts({}), 5)
            dialog.toggle(0)
            dialog.apply()
            fake.permissions_done(5, "launch.sh: not allowed", "", false)
            verify(dialog.open)
            verify(!dialog.busy)
            compare(dialog.errorText, "launch.sh: not allowed")
            compare(dialog.notices[0].tone, "danger")
        }

        function test_escape_and_cancel_write_nothing() {
            openWith(facts({}))
            dialog.toggle(1)
            keyClick(Qt.Key_Escape)
            verify(!dialog.open)
            compare(fake.applied.length, 0)
            compare(dialog.finishedCount, 0)
            openWith(facts({}))
            dialog.toggle(1)
            mouseClick(findChild(dialog, "permCancel"))
            verify(!dialog.open)
            compare(fake.applied.length, 0)
        }

        function test_unchanged_never_applies() {
            openWith(facts({}))
            dialog.toggle(3)
            dialog.toggle(3)
            verify(!dialog.changed)
            keyClick(Qt.Key_Return)
            mouseClick(findChild(dialog, "permApply"))
            compare(fake.applied.length, 0)
            verify(dialog.open)
            verify(!findChild(dialog, "permApply").enabled)
        }

        function test_a_mixed_bit_cycles_and_only_moves_itself() {
            // Two files: owner execute and group execute differ.
            openWith(facts({ total: 2, files: 2, name: null, bits: [2, 2, 1, 2, 0, 1, 2, 0, 0] }))
            compare(dialog.orig, [2, 2, 1, 2, 0, 1, 2, 0, 0])
            verify(dialog.anyMixed)
            compare(dialog.context, "2 files  ·  ~/Projects")
            dialog.toggle(2)
            compare(dialog.draft[2], 2)
            compare([dialog.addMask, dialog.removeMask], [64, 0])
            dialog.toggle(2)
            compare(dialog.draft[2], 0)
            compare([dialog.addMask, dialog.removeMask], [0, 64])
            dialog.toggle(2)
            compare(dialog.draft[2], 1, "back to each item as it is")
            verify(!dialog.changed)
            dialog.toggle(4)
            compare([dialog.addMask, dialog.removeMask], [16, 0], "group execute stays mixed")
        }

        function test_a_folder_speaks_of_folders_and_only_itself() {
            openWith(facts({ files: 0, dirs: 1, name: "Projects", place: "~" }))
            compare(dialog.permLabels, ["List files", "Add or remove", "Access"])
            verify(dialog.notices.some(n => n.text.indexOf("This folder only.") === 0))
            dialog.toggle(5) // group Access off, group Add or remove off already
            dialog.toggle(4) // group Add or remove on, without Access
            verify(dialog.notices.some(n => n.text.indexOf("Add or remove works only together with Access") >= 0))
            dialog.toggle(2) // own Access off
            verify(dialog.notices.some(n => n.text.indexOf("You will not be able to open the folder") === 0))
            dialog.explaining = true
            wait(0)
            compare(dialog.definitions.map(d => d[0]), ["List files", "Add or remove", "Access"])
        }

        function test_files_and_folders_together_are_named_honestly() {
            openWith(facts({ total: 3, files: 2, dirs: 1, name: null, special: 1 }))
            compare(dialog.kind, "mixed")
            compare(dialog.permLabels, ["Read", "Change", "Run / Access"])
            compare(dialog.context, "2 files and 1 folder  ·  ~/Projects")
            verify(dialog.notices.some(n => n.text.indexOf("Folders change only themselves") === 0))
            verify(dialog.notices.some(n => n.text.indexOf("special system flags") >= 0))
            const terms = dialog.definitions.map(d => d[0])
            verify(terms.indexOf("Run") >= 0 && terms.indexOf("Access") >= 0, terms)
        }

        function test_someone_elses_items_are_view_only() {
            openWith(facts({ owner: { same: true, name: null, you: false },
                             group: { same: false, name: null, member: false }, owned: 0 }))
            compare(dialog.audiences[0].title, "Owner")
            compare(dialog.audiences[0].sub, "An account not named on this computer")
            compare(dialog.audiences[1].sub, "Each item's own group")
            verify(!dialog.editable)
            dialog.toggle(0)
            verify(!dialog.changed)
            compare(dialog.statusText, "View only")
            verify(!findChild(dialog, "permApply").visible, "no inert Apply")
            verify(dialog.notices[0].text.indexOf("Only the owner or an administrator") === 0)
        }

        function test_a_read_failure_is_shown_and_closes() {
            openWith({ ok: false, token: 1, error: "A link has no permissions of its own. Change the item it points to instead" })
            verify(!dialog.ready)
            compare(dialog.errorText.indexOf("A link has no permissions"), 0)
            verify(!findChild(dialog, "permApply").visible)
            keyClick(Qt.Key_Return)
            compare(fake.applied.length, 0)
            keyClick(Qt.Key_Escape)
            verify(!dialog.open)
        }

        function test_a_refused_load_says_why() {
            fake.loadError = "Nothing to change here"
            dialog.openFor(fake)
            verify(dialog.open)
            verify(!dialog.loading)
            compare(dialog.errorText, "Nothing to change here")
            dialog.close()
        }

        function test_keyboard_moves_switches_and_stays_inside() {
            openWith(facts({}), 9)
            wait(0)
            const first = findChild(dialog, "permCell0")
            verify(first.activeFocus, "the first box has the keyboard")
            keyClick(Qt.Key_Right)
            verify(findChild(dialog, "permCell1").activeFocus)
            keyClick(Qt.Key_Down)
            keyClick(Qt.Key_Down)
            verify(findChild(dialog, "permCell7").activeFocus)
            keyClick(Qt.Key_Space)
            compare(dialog.draft[7], 2)
            // Twelve stops (nine boxes, explanations, Cancel, Apply): 17 Tabs
            // from the eighth box come back round to the first.
            for (let i = 0; i < 17; i++) keyClick(Qt.Key_Tab)
            verify(findChild(dialog, "permCell0").activeFocus)
            let inside = false
            for (let it = top.Window.activeFocusItem; it; it = it.parent)
                if (it === dialog) inside = true
            verify(inside, "Tab never leaves the card")
            keyClick(Qt.Key_Return)
            compare(fake.applied, [[9, 2, 0]])
        }

        function test_a_roomy_desktop_keeps_full_columns() {
            openWith(facts({ files: 0, dirs: 1, name: "Projects", place: "~" }))
            wait(0)
            const content = findChild(dialog, "permContent")
            compare(content.colWidth, content.colIdeal, "nothing narrows when it fits")
            for (const h of findChildrenNamed(dialog, "permHeader"))
                compare(h.lineCount, 1, h.text + " stays on one line")
        }

        function test_the_smallest_window_at_the_largest_zoom_loses_nothing() {
            // Main's minimum size, Ctrl+= at its top step, Roomy density.
            top.width = 520
            top.height = 320
            Tokens.fontBase = 12 * 1.6
            Tokens.spacingScale = 1.25
            openWith(facts({ files: 0, dirs: 1, name: "Projects", place: "~" }))
            wait(0)
            const content = findChild(dialog, "permContent")
            verify(content.whoWidth > 0, "audiences keep room: " + content.whoWidth)
            for (const a of findChildrenNamed(dialog, "permAudience")) {
                verify(a.width > 0, a.text + " has room")
                verify(!a.truncated, a.text + " is not cut")
            }
            for (const h of findChildrenNamed(dialog, "permHeader"))
                verify(!h.truncated, h.text + " is whole, if wrapped")
            const flick = findChild(dialog, "permFlick")
            const card = dialog.children.find(c => c.objectName === "permCard")
            verify(flick.contentHeight > flick.height, "the choices scroll here")
            // The buttons never scroll: they sit inside the card, below the choices.
            for (const name of ["permCancel", "permApply"]) {
                const r = inCard(findChild(dialog, name))
                verify(r.x >= 0 && r.y >= 0 && r.x + r.width <= card.width && r.y + r.height <= card.height,
                       name + " is inside the card: " + JSON.stringify(r))
                verify(r.y >= flick.y + flick.height, name + " is below the choices")
            }
            // Every box is whole and reachable: focusing it scrolls it into view.
            for (let i = 0; i < 9; i++) {
                const cell = findChild(dialog, "permCell" + i)
                verify(cell.width > 0)
                dialog.focusCell(i)
                wait(0)
                const r = cell.mapToItem(flick, 0, 0, cell.width, cell.height)
                verify(r.y >= -0.5 && r.y + r.height <= flick.height + 0.5,
                       "box " + i + " is in view: " + JSON.stringify(r))
                verify(r.x >= 0 && r.x + r.width <= flick.width + 0.5, "box " + i + " fits across")
            }
            // Tab from the last box reaches the buttons, which are on screen.
            dialog.toggle(8)
            keyClick(Qt.Key_Tab)
            keyClick(Qt.Key_Tab)
            verify(findChild(dialog, "permCancel").activeFocus)
            keyClick(Qt.Key_Tab)
            verify(findChild(dialog, "permApply").activeFocus)
            keyClick(Qt.Key_Return)
            compare(fake.applied.length, 1)
        }
    }
}
