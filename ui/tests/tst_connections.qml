import QtQuick
import QtTest

TestCase {
    id: test
    name: "ComputerConnections"
    width: 1180
    height: 720
    visible: true
    when: windowShown

    ConnectPanel { id: panel }
    SignalSpy { id: connectSpy; target: panel; signalName: "connectRequested" }
    SignalSpy { id: openSpy; target: panel; signalName: "openRequested" }
    SignalSpy { id: offSpy; target: panel; signalName: "disconnectRequested" }
    SignalSpy { id: refreshSpy; target: panel; signalName: "refreshRequested" }
    SignalSpy { id: actionSpy; target: panel; signalName: "actionRequested" }

    function test_setup_does_not_go_blank_while_capabilities_are_loading() {
        panel.places = {}; refreshSpy.clear(); actionSpy.clear()
        panel.showSupport("apple")
        verify(panel.statusTitle.length > 0, "A device click must explain that support is being checked")
        compare(refreshSpy.count, 1)
        panel.places = {setup: [{key: "apple", packages: ["gvfs-afc"], installable: true}]}
        verify(panel.statusBody.indexOf("gvfs-afc") >= 0, "Disclose packages when the snapshot arrives")
        panel.places = {setup: [{key: "apple", packages: [], installable: true}]}
        compare(panel.statusTitle, "Connection support is installed")
        compare(actionSpy.count, 0)
        verify(panel.statusBody.indexOf("Signing in is checked") >= 0, "Installed helpers are not a resolved sign-in")
        compare(connectSpy.count, 0, "Readiness is not permission to connect")
    }
    function test_a_new_connect_view_does_not_retain_old_phone_setup_context() {
        panel.places = {setup: [{key: "apple", packages: ["gvfs-afc"], installable: true}]}
        panel.showSupport("apple"); compare(panel.requirement.key, "apple")
        panel.hide()
        const p = fixture(); p.sshfs = false
        p.setup = [{key: "ssh", packages: ["sshfs"], installable: true}]
        panel.places = p; panel.show()
        verify(panel.requirement !== null, "Old phone setup context must not hide the current SSH requirement")
        compare(panel.requirement.key, "ssh", "New Connect must use its own relevant prerequisite")
    }
    function test_install_button_remains_visible_at_minimum_window_size() {
        test.width = 520; test.height = 320
        const p = fixture(); p.sshfs = false
        p.setup = [{key: "ssh", packages: ["sshfs", "openssh", "fuse3"], installable: true}]
        panel.places = p
        waitForRendering(panel)
        const button = findChild(panel, "connectionInstallSupport")
        const notice = findChild(panel, "connectionNotice")
        verify(button && button.visible)
        const pt = button.mapToItem(notice, button.width / 2, button.height / 2)
        verify(pt.y >= 0 && pt.y < notice.height, "Primary setup action must not be hidden below a clipped message: y=" + pt.y + ", notice=" + notice.height)
    }
    function test_install_support_is_keyboard_accessible_and_disabled_while_busy() {
        const p = fixture(); p.sshfs = false
        p.setup = [{key: "ssh", packages: ["sshfs"], installable: true}]
        panel.places = p; waitForRendering(panel)
        const button = findChild(panel, "connectionInstallSupport")
        button.forceActiveFocus(); actionSpy.clear()
        keyClick(Qt.Key_Return)
        compare(actionSpy.count, 1)
        panel.beginFeedback("setup-install", "ssh", "Installing…")
        actionSpy.clear(); panel.installSupport()
        compare(actionSpy.count, 0, "Never start a second package installation")
    }
    function test_unsupported_installation_explains_the_manual_route_without_a_dead_button() {
        const p = fixture(); p.sshfs = false
        p.setup = [{key: "ssh", packages: ["sshfs"], installable: false}]
        panel.places = p; waitForRendering(panel)
        const button = findChild(panel, "connectionInstallSupport")
        compare(button.visible, false)
        actionSpy.clear(); panel.installSupport()
        compare(actionSpy.count, 0)
        verify(panel.statusBody.indexOf("package manager") >= 0)
    }
    function test_missing_support_has_a_real_install_action_without_connecting() {
        const p = fixture()
        p.sshfs = false
        p.setup = [{key: "ssh", packages: ["sshfs"], installable: true}]
        panel.places = p
        waitForRendering(panel)
        const install = findChild(panel, "connectionInstallSupport")
        verify(install && install.visible, "Missing support must offer an install button, not terminal homework")
        actionSpy.clear()
        mouseClick(install, install.width / 2, install.height / 2)
        compare(actionSpy.count, 1)
        compare(actionSpy.signalArguments[0][0], "setup-install:ssh")
        compare(connectSpy.count, 0, "Installation is not permission to connect")
        verify(panel.statusBody.indexOf("sshfs") >= 0, "State the packages before authorization")
        verify(panel.statusBody.indexOf("sudo pacman") < 0, "No terminal command as the primary remedy")
    }

    function route(kind, name, target) {
        return { kind: kind, name: name, detail: target, user: "media", path: "/fixture/" + kind,
                 connected: false, lost: false, online: true, available: true,
                 requires_tailnet: kind === "tailnet", endpoint_ids: [target + ":22"] }
    }
    function fixture() {
        return { sshfs: true, computers: [{name: "studio", routes: [
                    route("tailnet", "studio", "studio.tail.test"),
                    route("ssh", "studio-lan", "192.0.2.10")]}], cloud: [] }
    }
    function actionableRows() { return panel.rows.filter(r => r.type === "place") }
    function init() {
        test.width = 1180; test.height = 720
        panel.hide()
        panel.supportKey = ""
        refreshSpy.clear()
        panel.operation = null
        panel.feedbackTitle = ""; panel.feedbackBody = ""; panel.feedbackFailed = false
        panel.places = fixture()
        connectSpy.clear(); openSpy.clear(); offSpy.clear()
        panel.show()
        wait(0)
    }
    function cleanup() { panel.hide() }
    function test_reopening_with_an_unchanged_snapshot_selects_a_real_row_for_enter() {
        panel.places = {computers: [], cloud: [], mobile: [
            {name: "ipad", target: "100.64.0.3", os: "iPadOS", online: true, available: true}]}
        panel.show(); wait(0)
        compare(panel.selectionKey, "mobile:ipad")
        panel.hide(); panel.show(); wait(0)
        compare(panel.selectionKey, "mobile:ipad")
        actionSpy.clear()
        keyClick(Qt.Key_Return)
        compare(actionSpy.count, 1)
        compare(actionSpy.signalArguments[0][0], "device-share:ipad")
        compare(connectSpy.count, 0)
    }
    function test_missing_sshfs_has_a_prominent_actionable_notice() {
        const p = fixture()
        p.sshfs = false
        panel.places = p
        compare(panel.statusTitle, "Set up computer connections")
        verify(panel.statusBody.indexOf("sshfs") >= 0)
        verify(panel.statusBody.indexOf("sudo pacman") < 0)
        waitForRendering(panel)
        const notice = findChild(panel, "connectionNotice")
        verify(notice !== null, "Notice object must exist")
        compare(notice.visible, true, "Notice must be visible; panel=" + panel.visible + ", test=" + test.visible)
        verify(notice.height > 0, "Notice height=" + notice.height + ", panel=" + panel.height)
        const check = findChild(panel, "connectionCheckAgain")
        verify(check && check.visible)
    }

    function test_apple_mobile_devices_offer_sharing_without_an_ssh_attempt() {
        const p = fixture()
        p.mobile = [{name: "iphone-15", target: "100.64.0.2", os: "iOS", online: true, available: true},
                    {name: "ipad", target: "100.64.0.3", os: "iPadOS", online: false, available: false}]
        panel.places = p
        actionSpy.clear()
        const phones = panel.rows.filter(r => r.kind === "mobile")
        compare(phones.length, 2)
        panel.activate(panel.rows.findIndex(r => r.name === "iphone-15"))
        compare(actionSpy.count, 1)
        compare(actionSpy.signalArguments[0][0], "device-share:iphone-15")
        panel.activate(panel.rows.findIndex(r => r.name === "ipad"))
        compare(actionSpy.count, 2, "Offline devices must still offer setup guidance")
        compare(connectSpy.count, 0, "An iPhone must never enter the SSH account/key flow")
    }
    function test_attempt_stays_visible_and_retains_failure() {
        compare(panel.busy, false)
        verify(panel.beginFeedback("remote-connect", "studio-lan", "Connecting to studio-lan…"))
        compare(panel.busy, true)
        compare(panel.statusTitle, "Connecting to studio-lan…")
        panel.activate(panel.rows.findIndex(r => r.type === "place"))
        compare(connectSpy.count, 0, "A second attempt must not start")
        verify(panel.completeFeedback("remote-connect", "studio-lan", "Connection refused. Check SSH on that computer."))
        compare(panel.busy, false)
        compare(panel.open, true)
        compare(panel.feedbackFailed, true)
        verify(panel.statusBody.indexOf("Connection refused") >= 0)
    }
    function test_check_again_is_a_discovery_action_not_a_connection() {
        const p = fixture(); p.sshfs = false; panel.places = p
        waitForRendering(panel)
        const check = findChild(panel, "connectionCheckAgain")
        mouseClick(check, check.width / 2, check.height / 2)
        compare(refreshSpy.count, 1)
        compare(connectSpy.count, 0)
        p.sshfs = true; panel.places = JSON.parse(JSON.stringify(p))
        compare(panel.statusTitle, "", "An installed dependency must not keep nagging")
    }
    function test_hidden_attempt_completion_does_not_reopen_or_forget_failure() {
        panel.beginFeedback("remote-connect", "studio-lan", "Connecting…")
        panel.hide()
        compare(panel.busy, true, "Close is not cancellation")
        verify(panel.completeFeedback("remote-connect", "studio-lan", "Timed out"))
        compare(panel.open, false, "A late error must not steal focus")
        panel.show()
        compare(panel.statusBody, "Timed out")
    }
    function test_minimum_window_keeps_notice_and_list_inside_the_card() {
        test.width = 520; test.height = 320
        const p = fixture(); p.sshfs = false; panel.places = p
        waitForRendering(panel)
        const list = findChild(panel, "connectList")
        verify(list.height > 0, "List height=" + list.height)
        const notice = findChild(panel, "connectionNotice")
        const point = notice.mapToItem(panel, 0, notice.height)
        verify(point.y <= panel.height, "Notice must fit at the app's minimum size")
    }
    function test_success_does_not_leave_an_empty_notice_on_reopen() {
        panel.beginFeedback("remote-connect", "studio-lan", "Connecting…")
        verify(panel.completeFeedback("remote-connect", "studio-lan", ""))
        compare(panel.open, false)
        panel.show()
        compare(panel.statusTitle, "")
    }
    function test_single_tailnet_computer_connects_to_its_address_not_its_display_summary() {
        const p = fixture()
        p.computers[0].routes = [route("tailnet", "studio", "studio.tail.test")]
        panel.places = p
        panel.activate(panel.rows.findIndex(r => r.type === "place"))
        compare(connectSpy.count, 1)
        compare(connectSpy.signalArguments[0][2], "studio.tail.test", "Tailscale is a display summary, not the SSH destination")
    }
    function test_one_computer_not_two_sources() {
        compare(actionableRows().length, 1, "One computer must not appear twice")
        compare(actionableRows()[0].name, "studio")
        compare(connectSpy.count, 0, "Discovery must not connect")
    }
    function test_choose_route_then_connect_exact_alias() {
        const i = panel.rows.findIndex(r => r.type === "place")
        panel.activate(i)
        compare(connectSpy.count, 0, "Choosing a computer must not silently select a route")
        compare(actionableRows().length, 2)
        const ssh = panel.rows.findIndex(r => r.kind === "ssh")
        panel.activate(ssh)
        compare(connectSpy.count, 1)
        compare(connectSpy.signalArguments[0][0], "ssh")
        compare(connectSpy.signalArguments[0][1], "studio-lan")
        compare(connectSpy.signalArguments[0][2], "192.0.2.10")
    }
    function test_lost_route_unavailable_and_alternate_requires_explicit_activation() {
        const p = fixture()
        p.computers[0].routes[0].connected = true
        p.computers[0].routes[0].lost = true
        p.computers[0].routes[0].available = false
        panel.places = p
        panel.activate(panel.rows.findIndex(r => r.type === "place"))
        compare(actionableRows().length, 2)
        const tail = panel.rows.findIndex(r => r.kind === "tailnet")
        panel.activate(tail)
        compare(connectSpy.count, 0, "Unavailable Tailscale route must not connect")
        compare(openSpy.count, 0, "Lost mount must not open as if healthy")
        verify(panel.rows.some(r => r.type === "note" && r.text.indexOf("not retried") >= 0))
        panel.activate(panel.rows.findIndex(r => r.kind === "ssh"))
        compare(connectSpy.count, 1)
        compare(connectSpy.signalArguments[0][1], "studio-lan")
    }
    function test_healthy_connection_opens_without_switching() {
        const p = fixture()
        p.computers[0].routes[1].connected = true
        panel.places = p
        panel.activate(panel.rows.findIndex(r => r.type === "place"))
        compare(openSpy.count, 1)
        compare(openSpy.signalArguments[0][0], "/fixture/ssh")
        compare(connectSpy.count, 0)
    }
    function test_recovery_only_offers_a_new_independent_available_route() {
        const p = fixture()
        p.computers[0].routes[0].connected = true
        p.computers[0].routes[0].lost = true
        p.computers[0].routes[0].available = false
        panel.places = p
        compare(panel.recoveryCandidate([], "/fixture/tailnet/subfolder"), "tailnet:studio")
        compare(panel.recoveryCandidate(["tailnet:studio"], "/fixture/tailnet"), "")
        compare(panel.recoveryCandidate([], "/unrelated"), "")
        p.computers[0].routes[1].available = false
        panel.places = p
        compare(panel.recoveryCandidate([], "/fixture/tailnet"), "")
    }
    function test_recovery_rejects_shared_endpoints_vpn_and_lost_alternates() {
        const p = fixture()
        const a = p.computers[0].routes[0]
        const b = p.computers[0].routes[1]
        a.connected = true; a.lost = true
        b.endpoint_ids = a.endpoint_ids
        panel.places = p
        compare(panel.recoveryCandidate([], a.path), "", "Same destination is not an alternate route")
        b.endpoint_ids = ["different:22"]; b.requires_tailnet = true
        panel.places = JSON.parse(JSON.stringify(p))
        compare(panel.recoveryCandidate([], a.path), "", "Two VPN routes depend on the same VPN")
        b.requires_tailnet = false; b.connected = true; b.lost = true
        panel.places = JSON.parse(JSON.stringify(p))
        compare(panel.recoveryCandidate([], a.path), "", "A lost alternative cannot recover browsing")
    }
    function test_offline_tailnet_is_not_suggested_for_lost_ssh() {
        const p = fixture()
        p.computers[0].routes[1].connected = true
        p.computers[0].routes[1].lost = true
        p.computers[0].routes[0].online = false
        panel.places = p
        compare(panel.recoveryCandidate([], "/fixture/ssh"), "")
    }
    function test_lost_mount_requires_disconnect_before_connect() {
        const p = fixture()
        p.computers[0].routes[0].connected = true
        p.computers[0].routes[0].lost = true
        panel.places = p
        panel.showRoutes("tailnet:studio")
        wait(0)
        panel.activate(panel.rows.findIndex(r => r.kind === "tailnet"))
        compare(connectSpy.count, 0, "Do not reuse an already-mounted dead filesystem")
        compare(openSpy.count, 0)
        compare(panel.rows[panel.current].kind, "ssh", "Default to a usable alternate")
    }
    function test_ssh_availability_hint_does_not_block_explicit_proxy_routes() {
        const p = fixture()
        p.computers[0].routes[1].available = false
        panel.places = p
        panel.showRoutes("ssh:studio-lan")
        panel.activate(panel.rows.findIndex(r => r.kind === "ssh"))
        compare(connectSpy.count, 1)
    }
    function test_keyboard_choice_survives_refresh_reordering() {
        const p = fixture()
        panel.activate(panel.rows.findIndex(r => r.type === "place"))
        wait(0)
        keyClick(Qt.Key_Down)
        compare(panel.rows[panel.current].name, "studio-lan")
        p.computers[0].routes.reverse()
        panel.places = p
        wait(0)
        compare(panel.rows[panel.current].name, "studio-lan")
        keyClick(Qt.Key_Return)
        compare(connectSpy.count, 1)
        compare(connectSpy.signalArguments[0][1], "studio-lan")
    }
    function test_removed_selection_cannot_connect_a_replacement() {
        panel.activate(panel.rows.findIndex(r => r.type === "place"))
        wait(0)
        keyClick(Qt.Key_Down)
        panel.places = {sshfs: true, computers: [{name:"new", routes:[route("ssh", "other", "192.0.2.22")]}], cloud:[]}
        wait(0)
        keyClick(Qt.Key_Return)
        compare(connectSpy.count, 0, "Removal must not silently select another host")
    }
    function test_keyboard_can_choose_routes_for_a_connected_computer() {
        const p = fixture()
        p.computers[0].routes[0].connected = true
        panel.places = p
        panel.select(panel.rows.findIndex(r => r.type === "place"))
        keyClick(Qt.Key_Right)
        compare(actionableRows().length, 2)
        compare(openSpy.count, 0)
    }
    function test_escape_goes_back_before_closing() {
        panel.activate(panel.rows.findIndex(r => r.type === "place"))
        compare(actionableRows().length, 2)
        keyClick(Qt.Key_Escape)
        compare(panel.open, true)
        compare(actionableRows().length, 1)
        keyClick(Qt.Key_Escape)
        compare(panel.open, false)
    }
}
