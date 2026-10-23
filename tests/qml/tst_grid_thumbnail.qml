import QtQuick
import QtTest
import "@UI_URL@" as Gust

TestCase {
    name: "GridThumbnail"
    when: windowShown
    visible: true
    width: 320
    height: 320
    readonly property string fixtures: @FIXTURE_URL@

    Item { id: frame; width: 90; height: 90 }
    Component {
        id: thumbnail
        Gust.GridThumbnail { anchors.centerIn: parent; inset: 1 }
    }

    function load(name) {
        const image = createTemporaryObject(thumbnail, frame, {source: fixtures + name + ".png"})
        verify(image !== null)
        tryCompare(image, "status", Image.Ready)
        return image
    }

    function verifyFit(image) {
        verify(image.width <= Math.max(0, frame.width - image.inset * 2) + 0.001)
        verify(image.height <= Math.max(0, frame.height - image.inset * 2) + 0.001)
        if (image.width > 0 && image.height > 0)
            fuzzyCompare(image.width / image.height, image.implicitWidth / image.implicitHeight, 0.001)
        verify(image.visible)
    }

    function init() { frame.width = 90; frame.height = 90 }

    function test_bounded_decode_data() {
        return [
            {tag: "landscape", file: "landscape", w: 128, h: 96},
            {tag: "portrait", file: "portrait", w: 96, h: 128},
            {tag: "panorama", file: "wide", w: 128, h: 8},
            {tag: "tall", file: "tall", w: 8, h: 128},
            {tag: "square", file: "square", w: 128, h: 128},
            {tag: "oversized-cache", file: "large", w: 128, h: 96}
        ]
    }

    function test_bounded_decode(data) {
        const image = load(data.file)
        compare(image.implicitWidth, data.w)
        compare(image.implicitHeight, data.h)
        verifyFit(image)
    }

    function test_zoom_fits_without_redecoding() {
        const image = load("landscape")
        const spy = signalSpy.createObject(this, {target: image, signalName: "statusChanged"})
        verify(spy.valid)
        for (const size of [[48, 48], [240, 240], [180, 50], [10, 300], [0, 0]]) {
            frame.width = size[0]
            frame.height = size[1]
            wait(1)
            compare(image.implicitWidth, 128)
            compare(image.implicitHeight, 96)
            verifyFit(image)
        }
        compare(spy.count, 0)
        spy.destroy()
    }
    Component { id: signalSpy; SignalSpy {} }

    function test_recycled_tile_changes_aspect_and_clears() {
        const image = load("landscape")
        image.source = fixtures + "portrait.png"
        tryCompare(image, "implicitHeight", 128)
        tryCompare(image, "status", Image.Ready)
        compare(image.implicitWidth, 96)
        verifyFit(image)
        image.source = ""
        tryCompare(image, "status", Image.Null)
        verify(!image.visible)
        image.source = fixtures + "wide.png"
        tryCompare(image, "status", Image.Ready)
        compare(image.implicitWidth, 128)
        compare(image.implicitHeight, 8)
        verifyFit(image)
    }
}
