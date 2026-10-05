# HANDOFF — Gust v1

**Status: v1 is done.** Every item in the frozen v1 scope (handoff §4) is built,
tested, driven in the real window, and checked by screenshot with vision. The
gates are green at the final commit. Nothing was pushed, tagged or published.
All commits are local on `master` in the Realm VM. Each commit was exported to
the host artifact store as a `git bundle` plus a `git archive` tarball with
SHA-256 sums, and verified with `sha256sum -c` and `git bundle list-heads`.

## 1. What was built, by commit

The v1 scope, in order:

| Commit | Scope item | What |
|---|---|---|
| `ac7b8c4` | Gates + sort arrow | Gates on 7a30eaa (test 724/0, lint green, qa 425/0); sort headers now read ▲ ascending / ▼ descending, test-first (qa §29 failed 3 asserts on the old glyphs) — D32 |
| `ec4f638` | Re-bench vs Flea 0.3.7 | Standard and low-end profiles; the low-end limits are applied to the run and checked on the live process (D33, D34). Results are in §2 |
| `a2c9a91` | Seven everyday features + four bug checks | Pause/Resume (D35), Owner… (D36), paste image as file (D37), Link here (D38), Open as administrator (D39), copied/cut marks (D40), Tabs at start (D41). Bug checks: folder sizes only where drawn (D42), no dead drive row and no reload on re-click (D43), arrows after the sidebar (already worked, now in qa). Two older layout bugs found by vision (D44, D45). qa §30 |
| final commit (`git log -1`) | Visual pass + release drafts + this handoff | Visual pass in tokyo-night and white with before/after pairs (`docs/screens/`, D46–D50), including a safety fix: the Trash dialog named the cursor row while the selection was what it would trash. Release drafts in `docs/release/`. STATUS updated |

Earlier work (before v1 scope, from the interim handoff): `101c891` honest test
counts · `a4541a1` viewport-scoped listing · `add03b6` keys, presets, key map ·
`b52926d` make link, permissions · `900b37f` WCAG contrast in all themes, FAT32
rules · `37911be` Ctrl+L folder jump · `2f415cd` Kind column · `82fdea2`
rubber-band select · `156c2fd` custom actions · `0643386` field bench ·
`3e4aa18` Run… behind a prompt · `7a30eaa` per-folder sort.

## 2. Bench: Flea's own field bench, same VM, cold cache, medians of 3

Flea 0.3.7 (ae9419c), Strata 0.20.1, Nautilus 50.3.1, Dolphin 26.08.0, Thunar
4.20.9. KVM, 8 vCPU, 3 GiB, llvmpipe. Full ranges and CSVs:
`bench/RESULTS.md`, `bench/field/2026-10-03-*`.

**Standard profile**

| Fixture | Entrant | First window ms | Settled ms | PSS MiB | CPU s |
|---|---|---|---|---|---|
| 100k | **Gust** | **240** | **410** | **93** | **0.24** |
| 100k | Flea 0.3.7 | 297 | 756 | 177 | 0.36 |
| 100k | Strata | 363 | 762 | 198 | 0.67 |
| 100k | Dolphin | 350 | 2876 | 208 | 3.97 |
| 100k | Nautilus | 475 | 4051 | 347 | 4.01 |
| media | **Gust** | **228** | **277** | 80 | **0.19** |
| media | Flea 0.3.7 | 310 | 848 | 186 | 0.47 |
| media | Thunar | 267 | 567 | **39** | 0.38 |
| media | Strata | 361 | 1282 | 160 | 0.67 |
| media | Dolphin | 367 | 7788 | 97 | 2.71 |
| media | Nautilus | 493 | 14205 | 279 | 7.79 |

Gust leads every column of Flea's table on both fixtures.

**Low-end profile** (2 CPUs, 2 GiB, swap off, software rendering)

| Fixture | Entrant | First window ms | Settled ms | PSS MiB | CPU s |
|---|---|---|---|---|---|
| 100k | Gust | 217 | **352** | 93 | 0.22 |
| 100k | Flea 0.3.7 | **211** | 434 | **75** | **0.18** |
| media | Gust | 242 | **321** | 76 | **0.19** |
| media | Flea 0.3.7 | **204** | 983 | **74** | 0.20 |

Gust settles first on both fixtures. On the low-end profile Flea's first window
comes up 6 ms (100k, the ranges overlap) and 38 ms (media) sooner, and its
window uses less memory on 100k. The reason is measured, not guessed: software
rendering removed Flea's llvmpipe GL cost, and about 23 MiB of Gust's 100k
figure is the listing, which Gust keeps in the window process while Flea keeps
it in a backend. `bench/RESULTS.md` has the full explanation. Moving the
listing out of the window process is on the After-v1 list.

## 3. Screenshots

- `docs/screens/after/{tokyo-night,white}/`: 19 shots each of the main window
  (rows, grid, columns), the context menu, every dialog (trash, rename, owner,
  permissions, name clash), the command palette, key map, Trash, Recent, Quick
  Look (text and image), Settings, and the transfer card (running and done).
  Every surface was asserted open before its shot was taken.
- `docs/screens/before/`: the first-pass shots of the surfaces that changed.
- `docs/screens/SCORES.md`: the §5 rubric findings, the fixes, and the criteria
  left at 1 and why.
- The v1 feature shots (paste image, owner prompt, drop menu, paused card, row
  marks, sidebar checks, restored tabs, administrator window) come from qa §30,
  which writes them to the qa output folder on every run.
- Clips: not recorded (P6 launch material, outside the v1 scope).

## 4. Test counts at the final commit

- `just test`: 750 passed, 0 failed, 41 skipped. Each skip is named with its
  reason in `target/test-skips.log` (live fixtures: SMB/NFS/docker peers,
  FAT mount, live clipboard).
- `just lint`: green (rustfmt, clippy `-D warnings` on the four library
  crates, QML token and syntax rules).
- `just qa`: 495 passed, 0 failed. It drives the real window with real key
  events and a uinput pointer inside the guest's Hyprland session. Section 30
  covers the v1 features.
- `just theme-matrix`: 22 / 22 themes render with their own colours.
- Visual pass (`tests/visual-pass-run.sh`): 25 / 0 in tokyo-night and in white.
- Soak: not run (P5, outside the v1 scope).

## 5. Decisions you may want to reverse

All 50 are in `docs/DECISIONS.md`, each with a revert line. The ones worth a
look:

- **D39** Open as administrator relaunches Gust as root through pkexec in a
  separate window. That window never opens, runs or previews files. The gvfs
  `admin://` route never mounted on Omarchy.
- **D41** Tabs at start defaults to *Fresh*. Nothing is written until you
  choose *Reopen last*.
- **D38** Link here sits behind a right-button drag. The plain drag is
  unchanged.
- **D37** Paste image takes PNG, JPEG, WebP and GIF only (never SVG), up to
  128 MiB.
- **D48** In light themes with a stale shell.toml, menus use the theme
  background rather than `lighter_background`.
- **D49** Secondary text is held to 3:1 contrast, not 4.5:1.
- From before v1: **D6** viewport-scoped details, **D7** Enter refuses once on
  an unread mode, **D19** Permissions never touches setuid/setgid/sticky,
  **D21** any file of 4 GiB or more refuses the whole copy to FAT32, **D22**
  Ctrl+L is jump mode.

## 6. Not done, and why

Everything below is outside the frozen v1 scope and was not built (scope rule).
It is listed under "After v1" below. Two things inside the scope can only be
partly proven in a VM:

- **Open as administrator on a real desktop.** In the VM, qa uses a narrow
  polkit rule (`tests/admin-fixture.sh`) instead of a password prompt. The real
  prompt from Omarchy's polkit agent is on your list.
- **The PKGBUILD draft** passes `bash -n` and `makepkg --printsrcinfo`. A real
  `makepkg` needs a published tag.

## 7. Owner's to-do list (§8), in order

1. **Take the work.** The newest export is the folder named after the final
   commit in the host artifact store, `…/session-artifacts/<coding session>/exports/<sha>/`:
   ```
   cd <that folder> && sha256sum -c SHA256SUMS
   cd ~/Projects/gust && git fetch <that folder>/gust-<sha>.bundle master:v1-review
   git log --oneline master..v1-review    # review, then: git merge --ff-only v1-review
   ```
2. **Open as administrator, by hand.** On your desktop, open Gust, press `:`
   and run *Open as administrator*. Approve the polkit prompt and check that a
   red-edged "Administrator —" window opens on the same folder.
3. **Physical checks** (not possible in the VM):
   - USB stick: copy a folder to it, eject it from the sidebar, check it says
     "safe to unplug".
   - Android phone (MTP) and iPhone (AFC): browse, and copy both ways.
   - GPU matrix (Intel including Haswell-class, AMD, NVIDIA, hybrid). Run the
     bench on your own machine, never inside a shared session:
     ```
     cd ~/Projects/gust && cargo build --release -p gust-app
     FLEA_REPO=~/src/flea just bench-field
     PROFILE=lowend FLEA_REPO=~/src/flea just bench-field
     ```
4. **Accounts** (only once P4 lands): a test-tailnet auth key and real cloud
   accounts.
5. **Shipping, all yours:**
   - set the real repository URL in `docs/release/PKGBUILD`;
   - push, tag `v0.1.0`, and make the repository public;
   - fill in `sha256sums` and run `makepkg` in a clean chroot;
   - publish to the AUR;
   - send the Omarchy #10542 PR (text in `docs/release/omarchy-pr-10542.md`);
   - replace the repo README with `docs/release/README.md`;
   - post the launch.

## After v1 (recorded, deliberately not built)

New ideas and newer-Flea features go here instead of into v1.

- Move directory listing out of the window process (about 23 MiB of the
  window's PSS on a 100k folder is the listing itself; Flea's low-end PSS lead).
- Administrator window: a GUI prompt for polkit when no agent is running
  (today pkexec needs the desktop's polkit agent); an admin:// mount if gvfs
  ever registers one on Omarchy.
- Paste image: offer a name before saving; paste into a remote folder.
- Drop menu: "Link here" for remote targets (refused today, as copy rules).
- Restore tabs: remember selection, scroll and history per tab.
- From the visual pass (scored 1, not 0; see docs/screens/SCORES.md): a
  filled primary-button style; the
  transfer card naming what it copies ("3 items → dest") and Replace tinted as
  destructive in the conflict dialog; a shadow under menus; Quick Look sized to
  short files; rendered Markdown in Quick Look.

- Tree expand in the list view, Miller column count, preview-column toggle.
- A transfer queue; i18n.
- Flea 0.3.7 settings: hidden files last, tight row density, Huge/Largest grid sizes, tint today's dates, columns sized from the window.
- P4: MTP/AFC, udisks, LocalSend; Tailscale SSH (Tailscale's own SSH server) as an alternative to plain SSH.
- P5: soak, stall matrix, IME, fractional scaling, AT-SPI.
- Extra P0 measurements: blank-frame trace, scroll frame times, menu and Quick Look latency, SMB 10k listing, whole-tree PSS.
- Launch posts, the AUR gust-bin package, the clean-install CI job.
- Move the listing out of the window process (a backend like Flea's) — the only way to close the low-end 100k window-PSS gap (93 vs 75 MiB, bench/RESULTS.md).
