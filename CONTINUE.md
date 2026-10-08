# CONTINUE — state for the next builder (VM is disposable)

Updated after every commit. Read this first after a realm replacement.

## Where things are
- Guest checkout: `~/Projects/gust` (branch `master`, local commits only — never push).
- Host artifact store (survives the VM): `…/session-artifacts/<coding id>/exports/`
  holds `gust-<sha>.bundle`, `gust-<sha>.tar.gz` and `SHA256SUMS`.
- Rival sources: `~/src/flea` (Flea's own bench tools under `tools/`).

## Recovery bootstrap (~5 min)
1. Push the newest bundle from the host into the guest; `git clone -b master <bundle> ~/Projects/gust`.
2. Verify `git archive --format=tar.gz HEAD | sha256sum` against `SHA256SUMS`.
3. `sudo pacman -S --needed rust cmake just clang base-devel qt6-declarative qt6-tools qt6-svg qt6-multimedia qt6-shadertools sshfs rclone samba nfs-utils gvfs-smb gvfs-mtp gvfs-afc wf-recorder tesseract tesseract-data-eng ffmpeg bubblewrap python-evdev jq flea strata nautilus dolphin thunar zoxide avahi nss-mdns udisks2 fcitx5 fcitx5-qt heaptrack perf python-pillow`
4. `CARGO_BUILD_JOBS=4 just build test lint` (3 GiB guest: cap jobs).

## Known traps
- `just qa` takes ~16 min and drives the one desktop: never rebuild, edit
  tests/qa.sh, or launch Gust while it runs.
- A fresh guest shows an Omarchy **update terminal** and two onboarding cards
  over every window; they swallow uinput clicks. Close the terminal and run
  `omarchy-notification-dismiss "<summary>"` before `just qa`.
- `just test` counts skips from `target/test-skips.log`; a test that returns
  early without `gust_testkit::skip!` is a lie — use the macro.
- `cargo fmt --all` touches many files on this toolchain; keep it in its own commit.
- qa §30h (Open as administrator) needs `tests/admin-fixture.sh start` once per VM:
  a polkit rule letting the test user pkexec exactly this checkout's Gust builds.

## State
- v2 (2026-10-08, branch `v2`): mount-point leftovers quarantined (D63), PlainText
  everywhere (D64), two windows (D62, qa §36), phones (D65, qa §37), batched flush
  to local disks (D66), Markdown tables/footnotes (D67). qa §37 needs no hardware;
  a real phone round trip is still open. Next: shared undo across windows,
  LocalSend via `localsend-cli`, Trash on USB drives, memory (compact names).
See `docs/HANDOFF.md` for the full picture (what is built, bench table, what is not done).
- Gates at the custom-actions commit: test 719/0 (41 named skips), lint green, qa 410/0, theme matrix 22/22.
- Uncommitted work: none.
- qa §28 no longer opens a real terminal (stand-in launcher, D30); full qa 410/0 at 3a95fe5.
- Per-folder sort (D31) gated on a fresh VM: `just test` 724/0 (41 named skips),
  `just lint` green, `just qa` 425/0 (includes the new §29 header-arrow asserts).
- Sort arrows fixed (D32): ▲ ascending, ▼ descending; §29 failed 3 asserts on the old
  glyphs first, then 17/0.
- v1 scope is frozen in the owner's handoff (section 4); work it in order:
  bench vs Flea 0.3.7 → paste image → ownership → Link here → open as admin →
  pause/resume → restore tabs → copied/cut marks → Flea bug checks → visual pass →
  release drafts → HANDOFF. Anything else goes to HANDOFF's "After v1" list.
- Re-bench vs Flea 0.3.7 done (2026-10-03), standard + low-end: bench/RESULTS.md.
- The seven everyday features and the four Flea 0.3.7 bug checks are done (D35–D45,
  qa §30, 64 asserts). Gates: `just test` 749/0 (41 named skips), `just lint` green,
  `just qa` 489/0. Next: visual pass → release drafts → HANDOFF.
- **v1 is done** (handoff §4 complete). Final gates: `just test` 750/0 (41 named
  skips), `just lint` green, `just theme-matrix` 22/22, `just qa` 495/0, visual
  pass 25/0 in tokyo-night and white. See `docs/HANDOFF.md`; anything further is
  its "After v1" list, not v1 work.

## Running a single qa section
`tests/qa-one.sh tests/qa.d/NN-name.sh /tmp/out` — same setup as the full suite.
New sections go in `tests/qa.d/`. FAT tests need `tests/fat-fixture.sh start` (sudo).

## Next (in order)
1. P0: `just bench-field` + `bench/RESULTS.md` done (FLEA_REPO=~/src/flea). Still: low-end
   profile (2 vCPU/2 GB/software), painted/input-ready, blank-frame trace, scroll frame
   times, menu + Quick Look latency.
2. P2 audit: screenshot + score every remaining surface (banner and breadcrumb done).
3. P3 rest (list in HANDOFF §6), then P4, P5, P6.
4. Flea 0.3.7 gaps to close: hidden-files-last option, copied/cut rows marked, tight row
   density, Huge/Largest grid sizes, tint-today option, Columns count from window width;
   check-alikes: folder sizes only where drawn, arrows regain list focus after sidebar,
   drives with no filesystem leave no dead sidebar row, clicking the current folder does
   not reload.
