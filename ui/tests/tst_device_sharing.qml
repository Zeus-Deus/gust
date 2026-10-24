import QtQuick
import QtTest

TestCase {
    id: test
    name: "DeviceSharing"
    width: 900
    height: 900
    visible: true
    when: windowShown
    QtObject {
        id: model
        property bool sharing_busy: false
        property bool sharing_checking: false
        property string sharing_json: "{}"
        property var calls: []
        property int checks: 0
        property int stops: 0
        signal sharing_done(string action, bool success, string message, string path)
        function request_sharing() { checks += 1 }
        function device_share(action, value) { calls = calls.concat([{action: action, value: value}]); sharing_busy = true; return "" }
        function cancel_device_share() { stops += 1 }
    }
    DeviceSharingPanel { id: panel }
    function init() {
        test.width = 900; test.height = 900
        model.sharing_busy = false; model.sharing_checking = false
        model.calls = []; model.checks = 0; model.stops = 0
        panel.inboxTouched = false; panel.receivedPath = ""
        model.sharing_json = JSON.stringify({targets: [{name: "iphone-15", address: "100.64.0.2", online: true},
            {name: "ipad", address: "100.64.0.3", online: false}], send_error: "", drive_error: "",
            default_path: "/home/fixture/Gust Shared", default_label: "/home/fixture/Gust Shared",
            default_token: "local:2f686f6d652f666978747572652f4775737420536861726564",
            inbox_path: "/home/fixture/Gust Inbox", shared_path: null})
        panel.show(model, "sample.txt", "", "", "send")
        waitForRendering(panel)
    }
    function visibleTexts(item, out) {
        if (!item.visible) return out
        if (item.text !== undefined && typeof item.text === "string" && item.text !== "") out.push(item.text)
        const children = item.children || []
        for (let i = 0; i < children.length; i++) visibleTexts(children[i], out)
        return out
    }
    function cleanup() { model.sharing_busy = false; panel.hide() }
    function test_opening_only_checks_state_and_does_not_send_receive_or_share() {
        compare(model.checks, 1)
        compare(model.calls.length, 0)
        compare(panel.chosenFolder, "/home/fixture/Gust Shared")
    }
    function test_tab_stays_in_the_panel_and_reaches_real_actions() {
        const close = findChild(panel, "closeDeviceSharing")
        close.forceActiveFocus()
        keyClick(Qt.Key_Tab)
        compare(panel.focusedButton, "refreshDeviceSharing")
        keyClick(Qt.Key_Tab)
        compare(panel.focusedButton, "deviceSharingHelp")
        keyClick(Qt.Key_Tab)
        compare(panel.focusedButton, "sendTo:100.64.0.2")
        keyClick(Qt.Key_Tab)
        compare(panel.focusedButton, "closeDeviceSharing", "Skip offline receivers and actions unrelated to sending")
        compare(model.calls.length, 0)
    }
    function test_sending_only_shows_receivers_and_help_is_optional() {
        verify(findChild(panel, "sharingSendChoices").visible)
        verify(!findChild(panel, "sharingReceiveActions").visible)
        verify(!findChild(panel, "sharingFolderActions").visible)
        verify(!findChild(panel, "sharingHelp").visible)
    }
    function test_general_view_keeps_setup_behind_help_without_changing_sharing() {
        panel.show(model, "", "", "iphone-15", "")
        verify(findChild(panel, "sharingReceiveActions").visible)
        verify(findChild(panel, "sharingFolderActions").visible)
        verify(findChild(panel, "sharingSendChoices").visible, "Every direction is named in the whole sheet")
        verify(!findChild(panel, "sendTo:100.64.0.2"), "Nothing selected means nothing can be sent")
        verify(!findChild(panel, "sharingHelp").visible)
        findChild(panel, "deviceSharingHelp").clicked()
        verify(findChild(panel, "sharingHelp").visible)
        compare(model.calls.length, 0)
        findChild(panel, "deviceSharingHelp").clicked()
        verify(!panel.helpOpen)
    }
    function test_share_requires_confirmation_and_cancel_changes_nothing() {
        panel.askFolder()
        const confirm = findChild(panel, "confirmDeviceFolder")
        verify(confirm.visible)
        verify(confirm.preferCancel)
        verify(confirm.body.indexOf(panel.chosenFolder) >= 0)
        verify(confirm.body.indexOf("other people") >= 0, "Existing network grants must be disclosed")
        keyClick(Qt.Key_Escape)
        compare(model.calls.length, 0)
        panel.askFolder()
        confirm.accepted()
        compare(model.calls.length, 1)
        compare(model.calls[0].action, "publish")
        compare(model.calls[0].value, JSON.parse(model.sharing_json).default_token)
    }
    function test_sending_is_explicit_and_offline_receivers_are_disabled() {
        const offline = findChild(panel, "sendTo:100.64.0.3")
        verify(offline && !offline.enabled)
        const online = findChild(panel, "sendTo:100.64.0.2")
        verify(online && online.enabled)
        online.clicked()
        compare(model.calls.length, 1)
        compare(model.calls[0].action, "send")
        compare(model.calls[0].value, "100.64.0.2")
        panel.start("receive", "")
        compare(model.calls.length, 1, "No overlapping action while sending")
    }
    function test_hiding_does_not_cancel_and_a_stop_retains_uncertainty() {
        panel.start("send", "100.64.0.2")
        panel.hide()
        compare(model.stops, 0)
        panel.show(model, "different.txt", "/unrelated", "other", "folder")
        compare(panel.selection, "sample.txt", "Reopening cannot replace the in-flight selection")
        const stop = findChild(panel, "cancelDeviceSharing")
        stop.clicked()
        compare(model.stops, 1)
        verify(panel.message.indexOf("may already have been accepted") >= 0)
        model.sharing_busy = false
        model.sharing_done("send", false, "One file accepted; delivery of another is uncertain.", "")
        verify(panel.failed)
        verify(panel.message.indexOf("uncertain") >= 0)
    }
    function test_stop_sharing_uses_the_displayed_mapping_and_keeps_files() {
        const state = JSON.parse(model.sharing_json)
        state.shared_path = "/home/fixture/Existing folder"
        state.shared_label = state.shared_path
        state.shared_token = "local:2f686f6d652f666978747572652f4578697374696e6720666f6c646572"
        model.sharing_json = JSON.stringify(state)
        panel.askFolder()
        const confirm = findChild(panel, "confirmDeviceFolder")
        verify(confirm.body.indexOf("All files stay") >= 0)
        confirm.accepted()
        compare(model.calls[0].action, "unpublish")
        compare(model.calls[0].value, state.shared_token)
    }
    function test_a_folder_uses_its_native_token_even_when_display_labels_collide() {
        panel.show(model, "", {path: "/home/fixture/two<U+000A>lines", token: "local:2f686f6d652f666978747572652f74776f0a6c696e6573"}, "", "folder")
        panel.askFolder()
        const confirm = findChild(panel, "confirmDeviceFolder")
        verify(confirm.body.indexOf("two<U+000A>lines") >= 0)
        confirm.accepted()
        compare(model.calls[0].value, "local:2f686f6d652f666978747572652f74776f0a6c696e6573")
    }
    function test_trailing_space_stays_inside_the_confirmation_and_unshare_identity() {
        const state = JSON.parse(model.sharing_json)
        state.shared_path = "/home/fixture/Photos "
        state.shared_label = state.shared_path
        state.shared_token = "local:2f686f6d652f666978747572652f50686f746f7320"
        model.sharing_json = JSON.stringify(state)
        panel.show(model, "", "", "", "folder")
        panel.askFolder()
        const confirm = findChild(panel, "confirmDeviceFolder")
        verify(confirm.body.startsWith("“/home/fixture/Photos ”\n\n"))
        compare(confirm.path, state.shared_token)
        confirm.accepted()
        compare(model.calls[0].action, "unpublish")
        compare(model.calls[0].value, state.shared_token)
    }
    function test_error_is_retained_without_reopening_or_automatic_retry() {
        panel.start("receive", "")
        panel.hide()
        model.sharing_busy = false
        model.sharing_done("receive", false, "Some files may already have arrived.", "")
        compare(panel.open, false)
        verify(panel.failed)
        compare(model.calls.length, 1)
    }
    function test_each_direction_is_named_from_this_computer_and_devices_by_their_names() {
        panel.show(model, "sample.txt", "", "ipad", "")
        waitForRendering(panel)
        compare(findChild(panel, "sharingTitle").text, "ipad")
        const texts = visibleTexts(panel, [])
        for (const words of ["Send a copy", "Get files from your devices", "Let devices open a folder", "the original stays here"])
            verify(texts.some(t => t.indexOf(words) >= 0), words)
        verify(!texts.some(t => /iPhone|iPad/.test(t)), "Copy never guesses the device model: " + texts.filter(t => /iPhone|iPad/.test(t)))
        verify(texts.some(t => t.indexOf("Offline") >= 0), "The device the sheet is about reports that it is offline")
        // Receiving and folder access are not narrowed to the titled device.
        verify(!texts.some(t => t.indexOf("from ipad") >= 0 || t.indexOf("ipad open") >= 0))
        const first = findChild(panel, "sendTo:100.64.0.3")
        const other = findChild(panel, "sendTo:100.64.0.2")
        verify(first.mapToItem(panel, 0, 0).x < other.mapToItem(panel, 0, 0).x, "The titled device is offered first")
        compare(model.calls.length, 0)
    }
    function test_result_appears_in_the_row_that_acted() {
        panel.show(model, "", "", "", "")
        findChild(panel, "receiveDeviceFiles").clicked()
        model.sharing_busy = false
        model.sharing_done("receive", true, "2 file(s) received", "/home/fixture/Gust Inbox/Received-1")
        verify(findChild(panel, "sharingStatus:receive").visible)
        compare(panel.message, "2 files saved in Gust Inbox.")
        verify(!findChild(panel, "sharingStatus:send").visible)
        verify(!findChild(panel, "sharingStatus:folder").visible)
        const show = findChild(panel, "openDeviceInbox")
        verify(show.visible)
        compare(show.text, "Show files")
        let opened = ""
        panel.openRequested.connect(p => opened = p)
        show.clicked()
        compare(opened, "/home/fixture/Gust Inbox/Received-1")
    }
    function test_nothing_waiting_offers_no_empty_folder_to_open() {
        panel.show(model, "", "", "", "")
        findChild(panel, "receiveDeviceFiles").clicked()
        model.sharing_busy = false
        model.sharing_done("receive", true, "No files are waiting.", "/home/fixture/Gust Inbox/Received-2")
        compare(panel.message, "Nothing waiting yet.")
        verify(!findChild(panel, "openDeviceInbox").visible)
    }
    function test_folder_menu_shows_only_folder_access_and_never_replaces_the_folder_that_is_on() {
        const state = JSON.parse(model.sharing_json)
        state.shared_path = "/home/fixture/Existing folder"
        state.shared_label = state.shared_path
        state.shared_token = "local:2f686f6d652f666978747572652f4578697374696e6720666f6c646572"
        model.sharing_json = JSON.stringify(state)
        panel.show(model, "", {path: "/home/fixture/Photos", token: "local:2f686f6d652f666978747572652f50686f746f73"}, "", "folder")
        waitForRendering(panel)
        verify(findChild(panel, "sharingFolderActions").visible)
        verify(!findChild(panel, "sharingSendChoices").visible)
        verify(!findChild(panel, "sharingReceiveActions").visible)
        verify(panel.otherFolderOn)
        const texts = visibleTexts(panel, [])
        verify(texts.some(t => t.indexOf("Only one folder at a time") >= 0 && t.indexOf("Photos") >= 0))
        compare(findChild(panel, "publishDeviceFolder").text, "Turn Off…")
        panel.askFolder()
        findChild(panel, "confirmDeviceFolder").accepted()
        compare(model.calls[0].action, "unpublish")
        compare(model.calls[0].value, state.shared_token, "Turning off acts on the folder that is actually on")
    }
    function test_small_window_keeps_close_and_stop_available() {
        test.width = 520; test.height = 320
        panel.start("send", "100.64.0.2")
        waitForRendering(panel)
        const card = findChild(panel, "deviceSharingPanel")
        for (const name of ["closeDeviceSharing", "cancelDeviceSharing"]) {
            const button = findChild(panel, name)
            const point = button.mapToItem(card, button.width / 2, button.height / 2)
            verify(button.visible && point.y >= 0 && point.y < card.height, name)
        }
    }
}
