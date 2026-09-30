# HANDOFF — Gust toward v1 (interim)

**Status: not v1.** P0 (partly), P1 (standard profile), a large slice of P3 and the
contrast part of P2 are done and proven. P2's screen audit, the rest of P3, and
P4–P6 are not started. This file is the honest state at the last commit; the
next builder continues from `CONTINUE.md`.

Nothing was pushed, tagged or published. All commits are local on `master` in
the Realm VM; every commit was exported to the host artifact store as a
`git bundle` + `git archive` tarball with SHA-256 (verified with `sha256sum -c`).

## 1. What was built, by phase

| Commit | Phase | What |
|---|---|---|
| `101c891` | Bootstrap | Honest test counts (skips counted as SKIP via `gust-testkit`), live clipboard test opt-in, `just test-required`, lint green on Rust 1.98 |
| `8a48cc6` | Bootstrap | qa names any window covering Gust before the mouse cases (the 9 baseline "failures" were an Omarchy update prompt) |
| `a4541a1` | P0/P1 | Flea's field bench with a Gust entrant; listing reads details for the visible rows only (100k settle 2098 → 476 ms) |
| `add03b6` | P3 | One key table for shortcuts + `?` key map; Ctrl+1–9; invert / select nothing; zoom; F3 split; 4 presets; middle-click tab; New file; SHA-256 in Quick Look; media bench |
| `b52926d` | P3 | Make link and Permissions, both undoable with dev+inode+mtime checks; hidden files dimmed in Columns |
| `900b37f` | P2/P3 | WCAG contrast on every surface in all 22 themes; FAT32 ≥4 GiB refusal and FAT-safe names (real FAT32 mount test) |
| `37911be` | P3 | Ctrl+L folder jump (bookmarks, zoxide, recent) + Tab completion; `~` and trailing `/` fixed |
| `2f415cd` | P3 | Kind column, sort by kind (Flea #190) |
| `82fdea2` | P3 | Rubber-band select (real-pointer qa); STATUS over-claims corrected |
| `156c2fd` | P3 | Custom actions (`~/.config/gust/actions/*.toml`, argv only, untrusted files ignored) |

## 2. Bench — Flea's own field bench, same VM, standard profile (medians of 3 cold runs)

| Fixture | Entrant | First window | Settled | PSS | CPU |
|---|---|---|---|---|---|
| 100k files | **Gust** | **269 ms** | **476 ms** | **107 MiB** | **0.34 s** |
| 100k files | Flea 0.3.6 | 360 ms | 816 ms | 223 MiB | 0.61 s |
| 100k files | Strata / Dolphin / Nautilus | see `bench/field/2026-09-30-scale-std-*.csv` | | | |
| 2000 media | **Gust** | 289 ms | **445 ms** | 90 MiB | **0.27 s** |
| 2000 media | Thunar | 319 ms | 634 ms | **41 MiB** | 0.45 s |
| 2000 media | Flea 0.3.6 | 364 ms | 1132 ms | 224 MiB | 0.66 s |
| 2000 media | Strata | 530 ms | 1818 ms | 192 MiB | 0.80 s |
| 2000 media | Dolphin | 454 ms | 9609 ms | 108 MiB | 3.13 s |
| 2000 media | Nautilus | 640 ms | 16639 ms | 313 MiB | 8.54 s |

- Gust leads every column against Flea on both fixtures.
- **Open:** Thunar uses less memory on the media fixture (41 vs 90 MiB).
- Reproducible: `FLEA_REPO=~/src/flea just bench-field`. A second full run (2026-09-30, `bench/field/2026-09-30-*-std.csv`) agreed: Gust 212/347 ms, 109 MiB on 100k vs Flea 295/757 ms, 220 MiB; see `bench/RESULTS.md`.
- **Not measured yet:** low-end profile (2 vCPU / 2 GB / software), painted/input-ready, blank frames, scroll frame times, context-menu and Quick Look latency, SMB 10k listing.

## 3. Screenshots

Key map, Kind column and theme-matrix captures were reviewed with vision
(session artifacts). `docs/screens/` showcase set: **not built**. Clips: **not recorded**.

## 4. Test counts (at `156c2fd`)

- Unit/library: 717 passed, 0 failed, 41 skipped (each named in the ledger).
- `just qa`: 395 passed, 0 failed (real keyboard + uinput mouse, guest Hyprland).
- Theme matrix: 22 / 22.
- Contrast test: 22 themes; fails on 12 without the fix.
- Soak: **not run**.

## 5. Decisions the owner may want to reverse

All 28 are in `docs/DECISIONS.md` with a revert line. The ones most worth a look:

- **D6** viewport-scoped details (size/date sorts still read all).
- **D7** Enter on a file whose mode isn't known yet refuses once ("press Enter again").
- **D11** Mac preset maps Cmd to Ctrl (Super belongs to Hyprland).
- **D12** zoom is not saved across sessions.
- **D19** Permissions never touch setuid/setgid/sticky.
- **D20** secondary text colours are lifted (minimally) where a theme falls under 3:1.
- **D21** any ≥4 GiB file refuses the whole copy to FAT32.
- **D22** Ctrl+L opens jump mode rather than a plain path box.

## 6. Not done, and why

Ran out of session budget, not blocked. Remaining, in suggested order:

1. P0: low-end profile and the extra measurements (`just bench-field` and `bench/RESULTS.md` exist).
2. P2: screenshot and score every surface, fix, `docs/screens/` pairs. Known items: crash banner hint text low on the white theme; banner has no primary button; banner overlaps the last list row. (Breadcrumb `/ /` fixed, qa §27.)
3. P3: paste image as file, safe run-executable prompt, open as admin (polkit), ownership, "Link here" on drop, per-folder sort, tree expand, Miller column count, preview-column toggle, Markdown/audio/PDF previews, pause/resume + queue, restore tabs, i18n, pain-point regressions, Thunar memory gap.
4. P4 Add-a-place + discovery, Tailscale, MTP/AFC, udisks + `scsi_debug`, LocalSend.
5. P5 soak, stall matrix, IME, fractional scaling, AT-SPI.
6. P6 README, release drafts, clips, Omarchy PR text, launch posts.

## 7. Owner's to-do list (§8)

1. **Physical checks** (none possible in the VM):
   - USB stick: copy a folder to it, eject from the sidebar, confirm "safe to unplug".
   - Android phone (MTP) and iPhone (AFC): browse and copy both ways.
   - GPU matrix (Intel incl. Haswell, AMD, NVIDIA, hybrid). Bench on your own machine, never inside a shared session:
     ```
     cd ~/Projects/gust && git pull <bundle>   # or fetch the exported bundle
     just build && cargo build --release -p gust-app
     cd bench/harness && FLEA_REPO=~/src/flea STRATA_BIN=/usr/bin/strata \
       GUST_BIN=~/Projects/gust/target/release/gust bash ./flea-field-bench-with-gust out.csv
     ```
2. **Test tailnet auth key** and **real cloud accounts** (only needed once P4 lands).
3. **Shipping:** pull the latest bundle from the host exports into `~/Projects/gust`, review, then push, tag, make public, publish AUR, send the Omarchy PR (#10542), post — all yours.
