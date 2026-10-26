import QtQuick

// Keep fractional wheel rotation proportional, with a short transition rather
// than a separate flick per event. A free-spinning wheel supplies its own tail.
// Declare inside the Flickable being scrolled (DESIGN §6.22).
WheelHandler {
    id: wheel
    required property Flickable view
    target: null
    acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad
    acceptedModifiers: Qt.NoModifier

    // Match GTK's Wayland surface-unit conversion (GtkScrolledWindow's
    // MAGIC_SCROLL_FACTOR), used by Omarchy's Nautilus. The compositor has
    // already applied the user's touchpad scroll factor to these deltas.
    readonly property real touchpadScale: 2.5

    property real scrollPosition: 0
    property bool applyingScroll: false

    onScrollPositionChanged: {
        if (!view) return
        applyingScroll = true
        view.contentY = scrollPosition
        applyingScroll = false
    }

    property NumberAnimation scrollAnimation: NumberAnimation {
        target: wheel
        property: "scrollPosition"
        duration: Math.round(Tokens.animFast * 2 / 3)
        easing.type: Easing.OutCubic
    }

    // Animation writes go through scrollPosition so an independent contentY
    // change (keyboard, scrollbar, navigation) can cancel pending wheel travel.
    property Connections viewChanges: Connections {
        target: wheel.view
        ignoreUnknownSignals: true
        function onContentYChanged() {
            if (!wheel.applyingScroll) wheel.scrollAnimation.stop()
        }
        function onCurrentIndexChanged() { wheel.scrollAnimation.stop() }
        function onContentHeightChanged() { wheel.scrollAnimation.stop() }
        function onHeightChanged() { wheel.scrollAnimation.stop() }
        function onOriginYChanged() { wheel.scrollAnimation.stop() }
        function onDraggingChanged() {
            if (wheel.view.dragging) wheel.scrollAnimation.stop()
        }
    }

    onEnabledChanged: if (!enabled) scrollAnimation.stop()

    onWheel: function(event) {
        const v = wheel.view
        // Qt Wayland can label a mouse wheel as a touchpad and supply pixels
        // only on some events. Gesture phases distinguish continuous scrolling;
        // otherwise keep every wheel event on the same angular scale.
        const continuous = event.phase !== Qt.NoScrollPhase
                        || event.angleDelta.y === 0
        const dy = continuous ? event.pixelDelta.y * touchpadScale
                 : event.angleDelta.y / 120 * Tokens.rowHeight * 3
        if (dy === 0) {
            // Keep subpixel gestures on the pixel path: Qt carries their
            // rounding error into later frames. Cancel pending wheel travel
            // for vertical gestures and their begin/end frames as well.
            if (continuous && (event.angleDelta.y !== 0
                    || (event.pixelDelta.x === 0 && event.angleDelta.x === 0))) {
                scrollAnimation.stop()
                v.cancelFlick()
            }
            return
        }
        // Accumulate against the destination, not the last animation frame.
        // On reversal, start at the visible position for an immediate response.
        const continuing = scrollAnimation.running
                         && (scrollAnimation.to - v.contentY) * dy < 0
        const base = !continuous && continuing ? scrollAnimation.to : v.contentY
        scrollAnimation.stop()
        v.cancelFlick()
        const top = v.originY
        const bottom = top + Math.max(0, v.contentHeight - v.height)
        const destination = Math.max(top, Math.min(base - dy, bottom))
        if (continuous) {
            v.contentY = destination
        } else {
            scrollPosition = v.contentY
            scrollAnimation.from = v.contentY
            scrollAnimation.to = destination
            scrollAnimation.start()
        }
        event.accepted = true
    }
}
