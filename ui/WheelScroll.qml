import QtQuick

// Mouse-wheel scrolling at the desktop's pace (DESIGN §6.22).
//
// Qt Quick turns every wheel event into a small flick. A high-resolution
// wheel (most current mice) reports one notch as eight small events, and
// eight tiny flicks barely move the list: five notches moved it by nothing
// at all in the qa probe, which is the "slow motion" scrolling people saw
// next to GTK apps. This scrolls a fixed distance per notch instead — the
// one GTK uses, page height ^ (2/3), about three rows in a normal window —
// and passes a touchpad's pixel deltas through unchanged so two-finger
// scrolling stays smooth.
//
// Declare it inside the ListView/GridView it scrolls and point `view` at it.
// A modified wheel (Ctrl, Shift) is left to the view's own handling.
WheelHandler {
    id: wheel
    required property Flickable view
    target: null
    acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad
    acceptedModifiers: Qt.NoModifier

    onWheel: (event) => {
        const v = wheel.view
        let dy = event.pixelDelta.y
        if (dy === 0)
            dy = event.angleDelta.y / 120 * Math.pow(v.height, 2 / 3)
        if (dy === 0) {
            event.accepted = false
            return
        }
        const top = v.originY
        const bottom = v.originY + Math.max(0, v.contentHeight - v.height)
        v.cancelFlick()
        v.contentY = Math.max(top, Math.min(v.contentY - dy, bottom))
        event.accepted = true
    }
}
