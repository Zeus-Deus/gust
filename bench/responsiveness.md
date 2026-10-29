# Cursor work and stalled-source cancellation

The cursor decorations in Rows, Grid and Columns now follow Qt's attached
current-item state. The view reads the shared model cursor once; delegates do
not each read it for fill, border or accessibility selection.

`tests/view-navigation.py` loads the actual components against a recording Qt
model and a deterministic theme service. At a 1200×650 logical viewport, the
baseline used 109 shared-cursor reads per step in Rows, 298 in Grid and 39 in
Columns. The changed components used one in each view. Both 1× and 2× runs pass,
including current/previous-item paint and far jumps. These counts establish less
binding work, not a whole-application timing improvement.

Regular copies isolate risky/unknown source reads behind a process-wide limit
of four source-only workers. Cancellation checks a pending read every 25 ms;
workers that remain blocked keep their admission slots and hold no destination
or mutation authority. Unknown filesystems and arbitrary FUSE subtypes are
isolated. Known local filesystems retain direct reads. Bounded filesystem pins
allow a transfer to reuse local classifications without a per-file mount-table
scan or accepting recycled device identities.

A private synthetic FUSE fixture held the source read while the production
engine was asked to cancel. The baseline was still Running after one second;
the changed engine reported Cancelled after 25 ms. The native application was
also driven in its own software-rendered Wayland session: repeated copy/move
cancellation, staging cleanup, four retained readers, saturation refusal,
recovery, byte equality and identity-checked undo passed. Screenshots and raw
receipts stay in private external output directories. These are synthetic
filesystem results, not physical-device/provider qualification.

Local overhead was checked with 1,000 synthetic 1 KiB files through
`recursive::copy_tree`, using the same release profile and eight alternating
baseline/candidate pairs with warm OS caches. Median elapsed times were
377.8 ms baseline and 386.2 ms candidate (about 2.2% higher), with overlapping
ranges of 375.1–390.2 and 378.3–388.3 ms. This is an explicit small-file tradeoff,
not a throughput improvement. An earlier per-file classification draft was
rejected after measuring substantially greater overhead.

Cancellation still has no deadline for source open/metadata or destination
write, durability, close and cleanup calls. A cancelled blocked source worker
can remain until the filesystem answers; its resource use remains bounded.
No whole-application speed, long-session memory or GPU/theme-matrix result is
claimed here. Reproduction entry points and fixture prerequisites are in
[TESTING.md](../docs/TESTING.md).
