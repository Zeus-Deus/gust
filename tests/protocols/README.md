# Native QA protocol input

`wlr-virtual-pointer-unstable-v1.xml` is copied from the public wlr-protocols
mirror, with its MIT license retained. It is used only to build the private
Wayland QA pointer; no host input device is opened.

Source: https://raw.githubusercontent.com/swaywm/wlr-protocols/master/unstable/wlr-virtual-pointer-unstable-v1.xml

Fetched bytes SHA-256: `3ff6d540be0bc5228195bf072bde42117ea17945a5c2061add5d3cf97d6bb524`.
The checked-in bytes, rather than a live download, are the build input.

`wayland-pointer X Y WIDTH HEIGHT` clicks left by default. Optional `left` or
`right` selects the button; two further coordinates perform an eight-step drag
before release. All points are range checked, and every mode retains the owned
0700 lab/socket restriction. The interpolation waits are input delivery aids,
not latency measurements.
