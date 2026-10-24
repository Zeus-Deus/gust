import QtQuick
import QtTest
import com.gust.app
import "@UI_URL@" as Gust

TestCase {
    id: test
    name: "FileIconSoftware"
    when: windowShown
    visible: true
    width: 600
    height: 400
    readonly property string artifacts: @ARTIFACTS@
    readonly property color background: "#202020"
    property var scene: null
    readonly property var grid: scene ? scene.view : null
    readonly property var smallIcon: scene ? scene.standalone : null

    // Qt's software grabWindow can omit the image's DPR metadata. Request
    // the full physical frame and map test coordinates explicitly instead
    // of letting QtTest crop a HiDPI frame to its top-left logical quadrant.
    Item {
        id: grabFrame
        visible: false
        width: test.width * Screen.devicePixelRatio
        height: test.height * Screen.devicePixelRatio
    }

    Component {
        id: sceneComponent
        Rectangle {
            property bool gridEnabled: false
            property alias view: grid
            property alias standalone: smallIcon
            anchors.fill: parent
            color: test.background

            GridView {
                id: grid
                visible: parent.gridEnabled
                x: 10; y: 70; width: 580; height: 260
                clip: true
                model: parent.gridEnabled ? 150 : 0
                cellWidth: 70; cellHeight: 85
                cacheBuffer: 400
                reuseItems: true
                property bool iconsVisible: true
                property string iconLook: "page\timage\t\tempty"
                property int reused: 0
                delegate: Item {
                    required property int index
                    width: grid.cellWidth; height: grid.cellHeight
                    GridView.onReused: grid.reused++
                    Gust.FileIcon {
                        objectName: "icon"
                        x: 10; y: 10
                        size: 50
                        look: grid.iconLook
                        visible: grid.iconsVisible
                    }
                }
            }

            Gust.FileIcon {
                id: smallIcon
                x: 110; y: 150
                visible: false
                look: "page\timage\t\tempty"
            }
        }
    }

    function settle() {
        verify(waitForPolish(scene))
        // Grab after a rendered frame, including visibility and delegate updates.
        wait(30)
    }

    function capture(tag) {
        settle()
        const image = grabImage(grabFrame)
        const scale = Screen.devicePixelRatio
        compare(image.width, Math.round(scene.width * scale))
        compare(image.height, Math.round(scene.height * scale))
        if (artifacts !== "")
            image.save(artifacts + tag + ".png")
        return {
            raw: image, scale: scale,
            red: (x, y) => image.red(Math.round(x * scale), Math.round(y * scale)),
            green: (x, y) => image.green(Math.round(x * scale), Math.round(y * scale)),
            blue: (x, y) => image.blue(Math.round(x * scale), Math.round(y * scale)),
            pixel: (x, y) => image.pixel(Math.round(x * scale), Math.round(y * scale))
        }
    }

    function isBackground(image, x, y) {
        return Math.abs(image.red(x, y) - 32) <= 1
            && Math.abs(image.green(x, y) - 32) <= 1
            && Math.abs(image.blue(x, y) - 32) <= 1
    }

    function verifyBackground(image, x0, y0, x1, y1, message) {
        let changed = 0
        let first = ""
        for (let y = Math.ceil(y0 * image.scale); y < Math.floor(y1 * image.scale); ++y) {
            for (let x = Math.ceil(x0 * image.scale); x < Math.floor(x1 * image.scale); ++x) {
                if (!isBackground(image.raw, x, y)) {
                    ++changed
                    if (first === "")
                        first = x + "," + y + "=" + image.raw.pixel(x, y)
                }
            }
        }
        compare(changed, 0, message + "; first changed pixel " + first)
    }

    function verifyClipped(image, tag) {
        verifyBackground(image, 0, 0, scene.width, grid.y, tag + ": above viewport")
        verifyBackground(image, 0, grid.y + grid.height, scene.width, scene.height,
                         tag + ": below viewport")
        verifyBackground(image, 0, grid.y, grid.x, grid.y + grid.height,
                         tag + ": left of viewport")
        verifyBackground(image, grid.x + grid.width, grid.y, scene.width,
                         grid.y + grid.height, tag + ": right of viewport")
    }

    function iconFrame(icon) {
        const p = icon.mapToItem(scene, 0, 0)
        return {x: p.x, y: p.y, width: icon.width, height: icon.height}
    }

    function verifyPage(image, frame, tag) {
        const x = Math.round(frame.x + frame.width * 0.5)
        const y = Math.round(frame.y + frame.height * 0.32)
        // Interior is a tint over the background, never an opaque family colour.
        // A broad alpha range tolerates raster rounding and small design changes.
        const alpha = (image.red(x, y) - 32) / (255 - 32)
        verify(alpha >= 0.06 && alpha <= 0.24,
               tag + ": expected translucent page interior, got " + image.pixel(x, y))
        verify(image.red(x, y) > image.green(x, y), tag + ": missing media tint")
        // A horizontal cross-section must also contain a substantially stronger
        // outline. This prevents an empty icon or a flat faint rectangle passing.
        let strongest = 0
        for (let dx = 0; dx < Math.ceil(frame.width); ++dx)
            strongest = Math.max(strongest, image.red(Math.round(frame.x) + dx, y))
        verify(strongest > 150, tag + ": missing visible page outline")
        // Both ends must be present: an empty or vertically truncated
        // HiDPI layer must not pass by drawing one correct cross-section.
        for (const fraction of [0.12, 0.88]) {
            const endY = Math.round(frame.y + frame.height * fraction)
            verify(!isBackground(image, x, endY), tag + ": truncated page silhouette")
        }
    }

    function visibleIcon() {
        for (let i = 0; i < grid.count; ++i) {
            const item = grid.itemAtIndex(i)
            if (item && item.y >= grid.contentY
                    && item.y + item.height <= grid.contentY + grid.height)
                return findChild(item, "icon")
        }
        fail("No fully visible grid delegate")
    }

    function init() {
        // Each test owns a new scene so a ghost from a failing case cannot
        // produce false failures in the following case.
        scene = createTemporaryObject(sceneComponent, test)
        verify(scene !== null)
        settle()
    }

    function startGrid() {
        scene.gridEnabled = true
        settle()
        compare(grid.count, 150)
    }

    function test_grid_clips_cached_icons_data() {
        return [
            {tag: "top", offset: 0},
            {tag: "partial-row", offset: 42},
            {tag: "middle", offset: 680}
        ]
    }

    function test_grid_clips_cached_icons(data) {
        startGrid()
        grid.contentY = data.offset
        verifyClipped(capture("clip-" + data.tag), data.tag)
    }

    function test_grid_translucent_fill_and_outline() {
        startGrid()
        const image = capture("grid-paint")
        verifyPage(image, iconFrame(visibleIcon()), "grid page")
    }

    function test_scroll_reuses_delegates_and_preserves_paint() {
        startGrid()
        const before = grid.reused
        const frames = []
        // Jumps exceed the cache in both directions. A third jump consumes
        // pooled delegates, rather than only creating fresh ones at the target.
        for (const index of [110, 30, 90]) {
            grid.positionViewAtIndex(index, GridView.Center)
            const image = capture("reuse-" + index)
            frames.push({image: image, index: index, icon: iconFrame(visibleIcon())})
        }
        verify(grid.reused > before, "Scenario must actually reuse delegates")
        for (const frame of frames) {
            // Delegates may now be pooled or reused again; retain coordinates
            // from the frame rather than reading their new positions.
            verifyPage(frame.image, frame.icon, "scroll to " + frame.index)
            verifyClipped(frame.image, "scroll to " + frame.index)
        }
    }

    function test_hidden_icons_leave_no_pixels() {
        startGrid()
        capture("before-hide")
        grid.iconsVisible = false
        verifyBackground(capture("icons-hidden"), 0, 0, scene.width, scene.height,
                         "hidden FileIcon delegates")
        grid.iconsVisible = true
        verifyPage(capture("icons-restored"), iconFrame(visibleIcon()), "restored icon")
    }

    function test_hidden_grid_leaves_no_pixels() {
        startGrid()
        grid.positionViewAtIndex(80, GridView.Center)
        capture("before-grid-hide")
        grid.visible = false
        verifyBackground(capture("grid-hidden"), 0, 0, scene.width, scene.height,
                         "hidden GridView ancestor")
        grid.visible = true
        const image = capture("grid-restored")
        verifyClipped(image, "restored GridView")
        verifyPage(image, iconFrame(visibleIcon()), "restored GridView")
    }

    function test_ancestor_opacity_composites_the_icon() {
        startGrid()
        const frame = iconFrame(visibleIcon())
        const x = Math.round(frame.x + frame.width * 0.5)
        const y = Math.round(frame.y + frame.height * 0.32)
        const full = capture("opacity-full")
        verifyPage(full, frame, "opaque ancestor")
        grid.opacity = 0.5
        const faded = capture("opacity-half")
        fuzzyCompare(faded.red(x, y) - 32, (full.red(x, y) - 32) * 0.5, 2)
        fuzzyCompare(faded.green(x, y) - 32, (full.green(x, y) - 32) * 0.5, 2)
        fuzzyCompare(faded.blue(x, y) - 32, (full.blue(x, y) - 32) * 0.5, 2)
        verifyClipped(faded, "translucent ancestor")
        grid.opacity = 0
        verifyBackground(capture("opacity-zero"), 0, 0, scene.width, scene.height,
                         "transparent GridView ancestor")
    }

    function test_rows_columns_scale_data() {
        return [{tag: "rows", size: 18}, {tag: "columns", size: 16},
                {tag: "large-grid", size: 72}]
    }

    function test_rows_columns_scale(data) {
        smallIcon.size = data.size
        smallIcon.visible = true
        verifyPage(capture("scale-" + data.tag), iconFrame(smallIcon), data.tag)
        smallIcon.look = "folder\t\t\t"
        const folder = capture("folder-" + data.tag)
        const p = smallIcon.mapToItem(scene, data.size * 0.5, data.size * 0.6)
        verify(!isBackground(folder, Math.round(p.x), Math.round(p.y)),
               data.tag + ": folder body must render")
        smallIcon.visible = false
        verifyBackground(capture("scale-hidden-" + data.tag), 0, 0,
                         scene.width, scene.height, data.tag + ": hidden small icon")
    }

    function test_reused_icon_repaints_theme_kind_and_size() {
        const original = String(Tokens.magenta)
        smallIcon.visible = true
        smallIcon.size = 50
        try {
            verifyPage(capture("repaint-before"), iconFrame(smallIcon), "initial page")
            Tokens.magenta = "#70e0f0"
            const recolored = capture("repaint-theme")
            const p = smallIcon.mapToItem(scene, smallIcon.size / 2, smallIcon.size * 0.32)
            verify(recolored.blue(Math.round(p.x), Math.round(p.y))
                   > recolored.red(Math.round(p.x), Math.round(p.y)), "theme must repaint the body")
            smallIcon.look = "folder\t\t\t"
            const folder = capture("repaint-folder")
            const f = smallIcon.mapToItem(scene, smallIcon.size / 2, smallIcon.size * 0.6)
            verify(!isBackground(folder, Math.round(f.x), Math.round(f.y)), "folder body must appear")
            Tokens.magenta = original
            smallIcon.look = "page\timage\t\tempty"
            for (const size of [72, 16, 50]) {
                smallIcon.size = size
                verifyPage(capture("repaint-size-" + size), iconFrame(smallIcon), "resized page")
            }
        } finally {
            Tokens.magenta = original
        }
    }
}
