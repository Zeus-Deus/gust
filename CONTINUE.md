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
- Done: bootstrap, baseline, test hygiene (D1–D3), lint green (D4–D5), qa intruder check.
- P0/P1: Flea's own field bench with a Gust entrant (D9). Gust leads every column of
  Flea's table on the 100k fixture; on the media fixture it leads first window,
  settled and CPU, but **Thunar uses less PSS (41 vs 90 MiB)** — open item.
  CSVs + manifests in `bench/field/`.
- P3 done so far (each with unit tests + a qa.d section):
  keys table + `?` key map + 4 presets + Ctrl+1–9 + invert/deselect + zoom + F3 split
  + middle-click tab (20-keys), New file / templates lib (20-keys, create.rs),
  SHA-256 in Quick Look (21-hash), Make link + Permissions with verified undo
  (22-attrs), hidden files dimmed in every view.
- P2 in progress: text contrast held to WCAG (4.5:1 primary, 3:1 secondary) for all
  22 themes (`theme_contrast` test fails on 12 themes without the fix).
- Gates at b52926d: test 685/0 (39 named skips), lint green, qa 375/0.

## Running a single qa section
`tests/qa-one.sh tests/qa.d/20-keys.sh /tmp/out` — same setup and helpers as the
full suite (qa-lib.sh). New sections go in `tests/qa.d/NN-name.sh`.

## Next (in order)
1. Commit contrast; re-run the 22-theme matrix and vision-score a sample.
2. P3 remaining: sort-by-kind qa check, rubber-band select, Ctrl+L folder jump
   (zoxide/recent/bookmarks), paste image as file, FAT32 4 GiB refusal,
   custom actions, Miller column count.
3. Low-end bench profile; P4 Add-a-place dialog; P5 soak; P6 docs.
