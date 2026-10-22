import QtQuick
import QtTest

TestCase {
    id: test
    name: "ComputerConnections"
    width: 1180
    height: 720
    when: windowShown

    ConnectPanel { id: panel }
    SignalSpy { id: connectSpy; target: panel; signalName: "connectRequested" }
    SignalSpy { id: openSpy; target: panel; signalName: "openRequested" }
    SignalSpy { id: offSpy; target: panel; signalName: "disconnectRequested" }

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
        panel.hide()
        panel.places = fixture()
        connectSpy.clear(); openSpy.clear(); offSpy.clear()
        panel.show()
        wait(0)
    }
    function cleanup() { panel.hide() }
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
