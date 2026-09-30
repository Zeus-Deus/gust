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
- Done: bootstrap, baseline, test hygiene (D1–D3), lint green (D4–D5).
- `just qa` 315/0 on a clear desktop (the 9 baseline mouse failures were the
  Omarchy update prompt over the window).
- P0 bench harness works: `~/bench-harness/field-bench` (copy of Flea's, diff in
  `bench/harness/`), fixtures under `/home/flea-sandbox/` (btrfs, outside $HOME),
  Flea built from source at `~/src/flea` (98404bc), `~/.local/bin/omarchy-drive` shim.
  Run: `printf 'x\n' | FLEA_REPO=$HOME/src/flea STRATA_BIN=/usr/bin/strata GUST_BIN=… bash field-bench out.csv`
  (passwordless sudo in the guest; the line is only a placeholder). `~/bin/agg.py out.csv` = medians.
- P1: viewport-scoped details (D6–D8). 100k fixture, std profile: Gust leads every
  column of Flea's table (see bench/field/2026-09-30-scale-std-viewport.csv).
- Next: media fixture run, low-end profile, then P3 keyboard gaps (Ctrl+1–9 is
  claimed in STATUS but absent from the code; invert selection lib is done, not wired).
