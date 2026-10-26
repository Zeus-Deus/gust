import QtQuick
import QtTest
import com.gust.app

TestCase {
    id: test
    name: "SidebarSelection"
    width: 240
    height: 900
    visible: true
    when: windowShown

    QtObject {
        id: firstTab
        property string home_path: "/home/test"
        property string location: home_path
    }
    QtObject {
        id: secondTab
        property string home_path: "/home/test"
        property string location: "/run/gust/test-server/Documents"
    }
    Sidebar {
        id: sidebar
        anchors.fill: parent
        model: firstTab
    }
    SignalSpy { id: placeSpy; target: sidebar; signalName: "placeChosen" }
    SignalSpy { id: remoteSpy; target: sidebar; signalName: "remoteChosen" }
    SignalSpy { id: deviceSpy; target: sidebar; signalName: "deviceChosen" }
    SignalSpy { id: phoneSpy; target: sidebar; signalName: "phoneChosen" }

    function init() {
        sidebar.model = firstTab
        firstTab.location = firstTab.home_path
        sidebar.activePanel = ""
        sidebar.bookmarks = [{label: "Documents", path: "/home/test/Documents", exists: true},
                             {label: "Projects", path: "/home/test/Projects", exists: true}]
        sidebar.devices = [{name: "USB", label: "USB", state: "mounted", path: "/dev/test-usb",
                            mountPoint: "/media/usb", mounted: true}]
        sidebar.remotePlaces = [{name: "test-server", kind: "ssh", path: "/run/gust/test-server", lost: false},
                                {name: "nested", kind: "ssh", path: "/run/gust/test-server/nested", lost: false}]
        // Let repeaters and the rail layout settle before using click coordinates.
        waitForRendering(sidebar)
        placeSpy.clear()
        remoteSpy.clear()
        deviceSpy.clear()
        phoneSpy.clear()
    }

    function row(key) { return findChild(sidebar, "sidebar:" + key) }
    function selected(key) {
        compare(sidebar.currentKey, key)
        const item = row(key)
        verify(item !== null)
        verify(item.current)
        compare(item.color, Tokens.withAlpha(Tokens.accent, 0.18))
    }

    function test_places_follow_location_without_pointer_hover() {
        selected("place:0")
        const point = sidebar.placePoints().Documents
        mouseClick(sidebar, point[0], point[1])
        compare(placeSpy.count, 1)
        // A click requests navigation; only the model's resulting location
        // selects a row, so a refused navigation cannot leave a false highlight.
        selected("place:0")
        firstTab.location = placeSpy.signalArguments[0][0]
        mouseMove(sidebar, sidebar.width - 1, sidebar.height - 1)
        selected("place:1")
        verify(!row("place:0").current)
        verify(!row("bookmark:0").current, "Duplicate bookmark must not draw a second active row")
        firstTab.location = "/home/test/Pictures"
        selected("place:3")
        firstTab.location = "/home/test/Documents"
        selected("place:1")
    }

    function test_bookmarks_and_mounted_devices() {
        firstTab.location = "/home/test/Projects/src"
        selected("bookmark:1")
        const point = sidebar.placePoints()["device:USB"]
        mouseClick(sidebar, point[0], point[1])
        compare(deviceSpy.count, 1)
        compare(deviceSpy.signalArguments[0][0], "/dev/test-usb")
        compare(deviceSpy.signalArguments[0][1], true)
        firstTab.location = "/media/usb"
        selected("device:0")
        firstTab.location = "/media/usb/photos"
        selected("device:0")
        firstTab.location = "/dev/test-usb"
        compare(sidebar.currentKey, "", "The device identifier is not a browsable mount point")
        firstTab.location = "/media/usb-other"
        compare(sidebar.currentKey, "")
        sidebar.devices = [{name: "USB", label: "USB", state: "unmounted", path: "/dev/test-usb",
                            mountPoint: "", mounted: false}]
        firstTab.location = "/media/usb"
        compare(sidebar.currentKey, "")
    }

    function test_phone_uses_mount_point_and_keeps_action_identifier() {
        sidebar.devices = [{name: "test-phone", label: "Phone", state: "open", stateKey: "open",
                            path: "test-phone", mountPoint: "/run/test-gvfs/phone", mounted: true, phone: "apple"}]
        waitForRendering(sidebar)
        const point = sidebar.placePoints()["device:test-phone"]
        mouseClick(sidebar, point[0], point[1])
        compare(phoneSpy.count, 1)
        compare(phoneSpy.signalArguments[0][0], "test-phone")
        compare(phoneSpy.signalArguments[0][1], "open")
        firstTab.location = "/run/test-gvfs/phone"
        selected("device:0")
        firstTab.location += "/DCIM"
        selected("device:0")
        firstTab.location = "/run/test-gvfs/phone-other"
        compare(sidebar.currentKey, "")
    }

    function test_remote_click_children_and_path_boundaries() {
        const point = sidebar.placePoints()["remote:test-server"]
        mouseClick(sidebar, point[0], point[1])
        compare(remoteSpy.count, 1)
        firstTab.location = remoteSpy.signalArguments[0][0]
        mouseMove(sidebar, sidebar.width - 1, sidebar.height - 1)
        selected("remote:0")
        firstTab.location += "/Documents"
        selected("remote:0")
        firstTab.location = "/run/gust/test-server/nested/photos"
        selected("remote:1")
        firstTab.location = "/run/gust/test-server-other"
        compare(sidebar.currentKey, "")
        sidebar.remotePlaces = []
        firstTab.location = "/run/gust/test-server"
        compare(sidebar.currentKey, "")
    }

    function test_switching_active_tab_and_panels() {
        sidebar.model = secondTab
        selected("remote:0")
        sidebar.activePanel = "recent"
        selected("recent")
        verify(!row("remote:0").current)
        sidebar.activePanel = "trash"
        selected("trash")
        sidebar.activePanel = ""
        selected("remote:0")
        sidebar.model = firstTab
        selected("place:0")
    }
}
