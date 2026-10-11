#!/usr/bin/env python3
"""A virtual pointer for desktop QA, via /dev/uinput.

The plan's §10 lists "restore an authorised interaction runner for the
missing end-to-end input checks" as an open item: without pointer injection,
every mouse-driven control is only ever verified by looking at a screenshot.
`wtype` covers the keyboard; nothing on this system covers the pointer.

This creates a temporary virtual mouse, uses it, and removes it on exit. It
needs write access to /dev/uinput (root on most systems), so it is invoked
through `sudo` by the QA harness and never by the application.

Absolute positioning is used rather than relative motion: a relative mouse
would depend on where the pointer already was, which makes a test that
passes once fail the next time.

Usage:
    uinput-mouse.py move X Y
    uinput-mouse.py click X Y [left|right|middle|back|forward]
    uinput-mouse.py drag X1 Y1 X2 Y2 [HOLD_SECONDS]
    uinput-mouse.py rdrag X1 Y1 X2 Y2      (right button: the drop menu)
    uinput-mouse.py wheel X Y TICKS      (negative scrolls down)
    uinput-mouse.py hiwheel X Y TICKS    (the same notches as a high-resolution
                                          wheel sends them: eighths, then the
                                          classic event on every full notch)
"""

import fcntl
import os
import struct
import sys
import time

# linux/input-event-codes.h
EV_SYN, EV_KEY, EV_REL, EV_ABS = 0x00, 0x01, 0x02, 0x03
SYN_REPORT = 0
BTN_LEFT, BTN_RIGHT, BTN_MIDDLE = 0x110, 0x111, 0x112
# The side buttons most mice have: Back and Forward.
BTN_SIDE, BTN_EXTRA = 0x113, 0x114
REL_WHEEL = 0x08
REL_WHEEL_HI_RES = 0x0B
ABS_X, ABS_Y = 0x00, 0x01

# linux/uinput.h
UINPUT_MAX_NAME_SIZE = 80
UI_DEV_CREATE = 0x5501
UI_DEV_DESTROY = 0x5502
UI_SET_EVBIT = 0x40045564
UI_SET_KEYBIT = 0x40045565
UI_SET_RELBIT = 0x40045566
UI_SET_ABSBIT = 0x40045567

# The virtual screen the absolute axes map onto. Hyprland scales this to
# the real output, so it only has to be consistent, not the true resolution.
ABS_MAX = 65535


def _ioctl(fd, req, arg):
    fcntl.ioctl(fd, req, arg)


def open_device(width, height):
    """Create the virtual pointer and return its file descriptor."""
    fd = os.open("/dev/uinput", os.O_WRONLY | os.O_NONBLOCK)

    _ioctl(fd, UI_SET_EVBIT, EV_KEY)
    for btn in (BTN_LEFT, BTN_RIGHT, BTN_MIDDLE, BTN_SIDE, BTN_EXTRA):
        _ioctl(fd, UI_SET_KEYBIT, btn)
    _ioctl(fd, UI_SET_EVBIT, EV_REL)
    _ioctl(fd, UI_SET_RELBIT, REL_WHEEL)
    _ioctl(fd, UI_SET_RELBIT, REL_WHEEL_HI_RES)
    _ioctl(fd, UI_SET_EVBIT, EV_ABS)
    for axis in (ABS_X, ABS_Y):
        _ioctl(fd, UI_SET_ABSBIT, axis)

    # struct uinput_user_dev: name, id{bustype,vendor,product,version},
    # ff_effects_max, absmax[64], absmin[64], absfuzz[64], absflat[64]
    name = b"gust-qa-pointer".ljust(UINPUT_MAX_NAME_SIZE, b"\0")
    absmax = [0] * 64
    absmin = [0] * 64
    absmax[ABS_X] = ABS_MAX
    absmax[ABS_Y] = ABS_MAX

    dev = struct.pack(
        f"{UINPUT_MAX_NAME_SIZE}sHHHHi{64}i{64}i{64}i{64}i",
        name,
        0x03,  # BUS_USB
        0x1234,
        0x5678,
        1,
        0,
        *absmax,
        *absmin,
        *([0] * 64),
        *([0] * 64),
    )
    os.write(fd, dev)
    _ioctl(fd, UI_DEV_CREATE, 0)
    # The compositor needs a moment to notice a new input device, or the
    # first event is delivered to nothing and the test silently does nothing.
    time.sleep(0.4)
    return fd


def emit(fd, etype, code, value):
    # struct input_event: timeval(2x long), type, code, value
    os.write(fd, struct.pack("llHHi", 0, 0, etype, code, value))


def sync(fd):
    emit(fd, EV_SYN, SYN_REPORT, 0)


def to_abs(value, extent):
    """Map a pixel coordinate onto the virtual absolute axis."""
    if extent <= 0:
        return 0
    return max(0, min(ABS_MAX, int(value * ABS_MAX / extent)))


def move_to(fd, x, y, w, h):
    emit(fd, EV_ABS, ABS_X, to_abs(x, w))
    emit(fd, EV_ABS, ABS_Y, to_abs(y, h))
    sync(fd)
    time.sleep(0.08)


def button(fd, btn, down):
    emit(fd, EV_KEY, btn, 1 if down else 0)
    sync(fd)
    time.sleep(0.06)


BUTTONS = {"left": BTN_LEFT, "right": BTN_RIGHT, "middle": BTN_MIDDLE,
           "back": BTN_SIDE, "forward": BTN_EXTRA}


def main():
    args = sys.argv[1:]
    if not args:
        print(__doc__)
        return 2

    # Screen size, so pixel coordinates can be mapped to absolute axes.
    w = int(os.environ.get("GUST_SCREEN_W", "1280"))
    h = int(os.environ.get("GUST_SCREEN_H", "800"))

    cmd = args[0]
    fd = open_device(w, h)
    try:
        if cmd == "move":
            x, y = int(args[1]), int(args[2])
            move_to(fd, x, y, w, h)

        elif cmd == "click":
            x, y = int(args[1]), int(args[2])
            btn = BUTTONS[args[3] if len(args) > 3 else "left"]
            # A freshly opened dialog has not finished taking pointer focus
            # 120ms after the cursor arrives: the press lands on whatever was
            # underneath and the click appears to do nothing at all. Settle
            # first, and nudge so the toolkit sees real motion into the
            # widget rather than a teleport.
            move_to(fd, x - 2, y - 2, w, h)
            time.sleep(0.10)
            move_to(fd, x, y, w, h)
            time.sleep(0.35)
            button(fd, btn, True)
            time.sleep(0.05)
            button(fd, btn, False)

        elif cmd == "doubleclick":
            x, y = int(args[1]), int(args[2])
            move_to(fd, x, y, w, h)
            time.sleep(0.12)
            for _ in range(2):
                button(fd, BTN_LEFT, True)
                button(fd, BTN_LEFT, False)
                time.sleep(0.05)

        elif cmd in ("drag", "rdrag"):
            x1, y1, x2, y2 = (int(v) for v in args[1:5])
            btn = BTN_RIGHT if cmd == "rdrag" else BTN_LEFT
            # A new window under an unchanged absolute position may never get
            # pointer entry. Establish real motion before pressing, as click
            # already does; otherwise a whole-suite first drag can be a no-op.
            move_to(fd, x1 - 2, y1 - 2, w, h)
            time.sleep(0.10)
            move_to(fd, x1, y1, w, h)
            time.sleep(0.15)
            button(fd, btn, True)
            # Intermediate steps: a drag that teleports is often ignored by
            # toolkits, which start a drag only after a movement threshold.
            steps = 12
            for i in range(1, steps + 1):
                move_to(
                    fd,
                    x1 + (x2 - x1) * i // steps,
                    y1 + (y2 - y1) * i // steps,
                    w,
                    h,
                )
                # A hand takes time to cross a window. Without a pause the
                # compositor coalesces the motion and a window the pointer
                # passes over during a drag sees almost none of it.
                time.sleep(0.03)
            # Settle on the target with a last small movement, as a hand
            # does: drop targets learn where the pointer is from motion.
            move_to(fd, x2 + 2, y2, w, h)
            time.sleep(0.05)
            move_to(fd, x2, y2, w, h)
            # An optional sixth argument keeps the button down that many
            # seconds at the end, so a screenshot can catch a drag in flight
            # (an argument, not an env var: sudo drops the environment).
            hold = float(args[5]) if len(args) > 5 else 0.0
            time.sleep(max(0.2, hold))
            button(fd, btn, False)

        elif cmd == "wheel":
            x, y, ticks = int(args[1]), int(args[2]), int(args[3])
            move_to(fd, x, y, w, h)
            step = 1 if ticks > 0 else -1
            for _ in range(abs(ticks)):
                # Real wheels send both in one frame. libinput reads only the
                # high-resolution axis from a device that has one, so the
                # classic event alone would be ignored.
                emit(fd, EV_REL, REL_WHEEL_HI_RES, 120 * step)
                emit(fd, EV_REL, REL_WHEEL, step)
                sync(fd)
                time.sleep(0.05)

        elif cmd == "hiwheel":
            # A high-resolution wheel (most current mice) reports each notch
            # as several small REL_WHEEL_HI_RES steps (120 per notch) and the
            # classic REL_WHEEL only once the notch completes. Toolkits that
            # treat every small step as a tiny flick scroll in slow motion.
            x, y, ticks = int(args[1]), int(args[2]), int(args[3])
            move_to(fd, x, y, w, h)
            sign = 1 if ticks > 0 else -1
            for _ in range(abs(ticks)):
                for part in range(8):
                    emit(fd, EV_REL, REL_WHEEL_HI_RES, 15 * sign)
                    if part == 7:
                        emit(fd, EV_REL, REL_WHEEL, sign)
                    sync(fd)
                    time.sleep(0.006)
                time.sleep(0.05)

        else:
            print(f"unknown command: {cmd}", file=sys.stderr)
            return 2

        # Let the compositor deliver everything before the device vanishes.
        time.sleep(0.25)
    finally:
        try:
            _ioctl(fd, UI_DEV_DESTROY, 0)
        except OSError:
            pass
        os.close(fd)
    return 0


if __name__ == "__main__":
    sys.exit(main())
