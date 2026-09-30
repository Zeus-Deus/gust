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

## State
See `docs/HANDOFF.md` for the full picture (what is built, bench table, what is not done).
- Gates at the custom-actions commit: test 717/0 (41 named skips), lint green, qa 397/0, theme matrix 22/22.
- Uncommitted work: none.

## Running a single qa section
`tests/qa-one.sh tests/qa.d/NN-name.sh /tmp/out` — same setup as the full suite.
New sections go in `tests/qa.d/`. FAT tests need `tests/fat-fixture.sh start` (sudo).

## Next (in order)
1. P0: `just bench-field` + `bench/RESULTS.md` done (FLEA_REPO=~/src/flea). Still: low-end
   profile (2 vCPU/2 GB/software), painted/input-ready, blank-frame trace, scroll frame
   times, menu + Quick Look latency.
2. P2 audit: screenshot + score every surface; known: banner hint contrast on white,
   no primary banner button, banner overlaps last row. (breadcrumb "/ /" fixed.)
3. P3 rest (list in HANDOFF §6), then P4, P5, P6.
