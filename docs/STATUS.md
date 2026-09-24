# Status — what is built, what is proven, what is not

Written against the plan's rule that a passed fixture is not a product-wide
pass. Three states only:

- **proven** — there is a test or a captured artifact, and it runs in `just test`,
  `just qa` or `just theme-matrix`.
- **proven by hand** — driven with real mouse input in the VM, with the result
  checked on disk (`cmp`, `sha256sum`, `ls`) and screenshots kept, plus unit
  tests for the logic underneath; the UI journey itself is not yet an
  automated `just qa` step.
- **built** — the code exists and compiles, but no end-to-end proof yet.
- **not started** — nothing beyond types.

Every bug fixed since m41 has a test that was run against the old code and
failed there first.

Baseline for every result below: Omarchy 4.0.3-1 KVM guest, Qt 6.11.2,
Rust 1.98.1, cxx-qt 0.10.0, Mesa llvmpipe (software rendering, Virtio GPU),
2 GB RAM / 8 vCPU. **No native GPU was tested.**

## M0 · qualify

| Item | State | Evidence |
| --- | --- | --- |
| Rust ↔ Qt bridge, QML module, compiled resources | proven | app builds and paints; `just qa` |
| Rust-backed list model with roles | proven | 5000-entry directory listed and navigated |
| Packaged QML loading | proven | qrc alias verified; a bad path now exits 2 loudly |
| First paint on a real compositor | proven | window appears on Hyprland, screenshots captured |
| Theme token adapter | proven | 22/22 installed themes, `just theme-matrix` |
| **Live theme switching** | **proven by hand** | `theme_live` (3); evidence `B-live-theme`: `omarchy-theme-set` to catppuccin-latte, gruvbox, white and back while Gust is open — one repaint each, no restart |
| Generation-tagged model updates | built | generation plumbed; no concurrent-navigation stress test yet |
| Weak hardware: no GPU at all | **proven** | every number in this file is llvmpipe software rendering in a VM with no GPU; copies were also run with the disk capped at 2–8 MB/s |
| **Thumbnails (image + video)** | **proven** | sandboxed helper, `just qa` §10q; 200 images in <6 s with the UI live |
| IME / fractional scaling | not started | — |
| GPU matrix | not tested | llvmpipe only; NVIDIA/AMD/Intel unverified |

## M1 · browse

| Item | State | Evidence |
| --- | --- | --- |
| Rows view | proven | default view, sortable headers, `just qa` |
| Grid view | proven | uniform tiles, row-aware Up/Down, screenshot |
| Columns view | proven | parent + current + preview pane, screenshot |
| Navigation (open, up, back, forward, breadcrumbs) | proven | `just qa` asserts each transition |
| Type-ahead selection (Standard preset) | proven | buffers keystrokes; "Pro" selects Projects |
| `/` filter | proven | narrows 11 → 1 and Escape restores |
| `:` command palette | proven | opens, filters actions, navigates to a typed path |
| Hidden files toggle | proven | Ctrl+H, 11 ↔ 15 entries |
| Natural sort (`file9` < `file10`) | proven | unit tests |
| Non-UTF-8 / unicode / bidi-spoof names | proven | listed without crashing; RLO escaped as `<U+202E>` |
| Broken symlink badge | proven | dangling link listed and marked |
| 5000-entry directory | proven | lists, settles, cursor still responds |
| Keyboard focus returns from every overlay | proven | regression test in `just qa` |
| **Right-click menus** (file, folder, several files, empty space; Menu key, Shift+F10) | **proven by hand** | evidence `B1-context-menu`: rows, grid and columns views; "Paste into folder" on a folder row; right-click inside a selection keeps it, outside replaces it; a keyboard-opened menu ignores the pointer until it moves; delete goes through the normal confirm and Trash, Ctrl+Z restores |
| **Live folder updates** | **proven by hand** | 13 tests (`dir_watch`); evidence `B-live-updates`: files made, deleted and renamed by another program appear in 160 ms with the scroll position and cursor kept; 50 000 files created outside Gust show in 1.8 s with the window responsive; a burst is 1–3 refreshes, a file being written refreshes once when closed; rows do not move while a rename prompt, menu or dialog is open; a deleted folder says "This folder no longer exists", not "empty". Local folders only — network mounts keep Ctrl+R |
| Vim preset | built | implemented, not yet asserted in QA |
| **Per-location view memory** | **proven** | `views::tests` + `just qa` §10r; survives a restart |
| Tabs | **proven** | per-tab location, history, selection, filter and view; Ctrl+T/W/Tab/1-9; last tab cannot be closed; a tab with a running job asks before closing |
| "Send with LocalSend" (mockup C) | **not possible** | LocalSend's desktop entry is `Exec=localsend` with no `%F` and no `MimeType`, it has no CLI, and it registers no D-Bus service. There is no supported way to hand it files. Recorded as unbuildable rather than pending. |
| System clipboard interop | **proven** | 12 unit + 8 live tests: a copy in Gust is readable by another file manager, a cut survives as a cut, and what Nautilus leaves is pasteable in Gust. `x-special/gnome-copied-files` with percent-encoded URIs; an http:// URI and ordinary prose are both refused as file lists; no path reaches a command line. Degrades to an internal clipboard, saying so, when wl-copy is absent. |
| Multi-file open | **proven** | 7 unit tests + live: three text files open ONE editor, not three. Respects the freedesktop distinction — `%F`/`%U` take a list, `%f`/`%u` are launched per file — because passing five paths to a `%f` program opens the first and silently drops four. Bounded at 50; a folder or executable in the selection falls back to single-row activation. |
| Open with… | **proven** | 16 unit tests + verified live against this machine's real desktop database (nvim/LibreOffice/Omawrite for text, imv/Chromium/Pinta/Kdenlive for PNG, with the default marked). Read from `mimeinfo.cache` and `mimeapps.list`, never from `gio mime`'s localized output; `NoDisplay` entries are still offered; `Exec` is parsed to argv so a hostile filename stays one argument. |
| Bulk rename | **proven** | 17 unit tests: the plan touches nothing, collisions/duplicates/invalid names refuse the WHOLE batch, a swap (`a→b`, `b→a`) does not lose a file, numbering follows the on-screen sort, no temporary names survive, and one Ctrl+Z puts every name back. The preview is mandatory — nothing renames until the user has seen every old → new. |
| Archives (list, extract, compress) | **proven** | 26 unit tests + 8 live against a genuinely hostile archive built with bsdtar: a real FIFO, an escaping symlink and a setuid file are each refused **by name**, the safe files are kept, listing writes nothing, and no staging directory survives. Compress writes to a staging name and renames on success. |
| Network shares (NFS via GIO) | **proven** | 4 live tests against a real NFSv4 export: mount, list, write reaching the backend, and the mount point predicted correctly. No credentials are asked for — NFS authorises by address, so the flow skips the password step rather than staging one it will not send. |
| Network shares (SMB via GIO) | **proven** | 11 unit tests + 7 live against a real Samba server: mount, list, write reaching the backend, predicted mount point, rejected login, and no password in any command line. Password masked in the UI and handed to GIO on stdin — never argv, log, config or places file. A GVfs mount is recognised by the stall guard. |
| Device discovery (USB, discs) | **partial** | 13 unit tests against captured `lsblk` from real hardware; system disk, swap, zram, loop and LVM correctly excluded; empty optical drive verified live in the sidebar. **Mount/eject against a physical stick is unproven — this VM has no USB device.** |
| Blocked mount does not block Gust | **proven** | measured: a direct listing of a frozen sshfs mount took 280s; the bounded probe returns in 4s. Startup, navigate and the listing worker are all guarded; only stallable filesystems pay for it |
| Accessibility (AT-SPI) | **proven** | 46 named nodes in the live tree, files exposed as list items, controls named ("Back", "Up one folder", "Rows view"); needs `QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1`, which Gust does not set globally |
| Font / spacing scaling | **proven** | starts, lists and navigates at font 9/12/18 and spacing 0.8/1.0/1.3 with no missing tokens |
| Quick Look (Space) | **proven** | bounded text peek, kind in words, mode line; escape sequences shown not obeyed; symlinks described not followed; images described without decoding here; multi-select moved to Ctrl+Space |
| Content search (`:grep`) | **proven** | scoped, recursive, cancellable; binaries skipped on a NUL sniff; symlinks not followed; bounded per file and overall, and says when it stopped early |
| Cloud remotes (rclone) | **proven** | discovery reads the installed rclone's config and opens no socket; connect mounts with a VFS write cache; "cached ≠ uploaded" shown as a notice; disconnect refuses while uploads are outstanding and never lazy-unmounts; degrades with a reason when rclone is absent |
| Cloud OAuth in Gust's own UI | not built | deliberate: `rclone config` handles every provider correctly, and Drive's flow is not a template for Proton or Filen (plan §3) |
| Two panes | **proven** | `:split` / `:unsplit`, Tab crosses the divider, F5 copies and F6 moves to the other pane through the normal journal and undo; a pane with no tabs closes itself; closing a pane keeps its tabs |
| Tab drag to split / detach | not started | needs cross-window Qt drag sessions; recorded unbuilt rather than half-built (DESIGN §6.7) |
| 100k benchmark | **proven** | `just bench`: 100k listed+settled in 470 ms (release), first window 155 ms |
| Listing off the UI thread | **proven** | worker thread + generation-tagged snapshots; names paint before metadata |
| PSS budget (≤ 90 MiB) | **MISSED — platform floor** | a BARE Qt Quick window costs 155 MiB here; Gust adds ~10 MiB. Needs a GPU machine to judge |

## M2 · operate

| Item | State | Evidence |
| --- | --- | --- |
| Descriptor-anchored path resolution | proven | intermediate-symlink swap refused (ENOTDIR/ELOOP) |
| Copy with staging + atomic rename | proven | unit tests incl. cancel leaves no partial file |
| setuid/setgid never propagated | proven | unit test |
| Special files (FIFO/socket/device) refused | proven | unit test |
| Symlinks copied as symlinks | proven | unit test |
| Cross-device move keeps source unless verified | proven | unit test |
| Trash / restore, freedesktop format | proven | round-trip, duplicate names, occupied-path refusal |
| Trash ≠ delete wording | proven | unit test + live confirmation dialog |
| Subtree-safe undo | proven | refuses when a descendant changed or new work appeared |
| Undo never offered for permanent delete | proven | unit test |
| Journal survives a torn write; private perms | proven | unit tests |
| Archive policy (traversal, escaping links, FIFO, bombs) | proven | unit tests on the vetting layer |
| Trash wired to the UI (confirm → move → undo) | proven | `just qa` §10b: file leaves, trashinfo written, Ctrl+Z restores content |
| Undo refuses when new work occupies the path | proven | `just qa` §10c: newer file preserved, trashed copy still recoverable |
| Recursive copy/move of whole trees | proven | unit tests: nesting, symlink loops, refusals, progress monotonic |
| Background job runner (UI thread never blocks) | proven | worker thread + snapshot polling; cancel/conflict/refused tests |
| Copy wired to UI (Ctrl+C / Ctrl+V) | proven | `just qa` §10c: nested tree arrives byte-identical, source untouched |
| Move wired to UI (Ctrl+X / Ctrl+V) | proven | `just qa` §10d: tree arrives, source removed only after success |
| Rename wired to UI (F2) | proven | `just qa` §10f: lands on disk, old name gone, cursor follows the renamed file |
| Rename conflict refused, nothing overwritten | proven | `just qa` §10e: prompt stays open, existing contents survive |
| New folder (Ctrl+N or right-click), Duplicate (Ctrl+D) | proven by hand | new folder by right-click, evidence `B1-context-menu`; the cursor lands on the new folder |
| **Never overwrite by accident** (rename, extract, "Keep both") | **proven** | `data_loss_regressions` (6): each was able to replace an existing file before; each failed against the old code first. No-clobber rename (`RENAME_NOREPLACE`); "Replace" moves the old file to Trash |
| **Conflict dialog** (copy, move, drop) | **proven by hand** | 2 tests + evidence `B2-conflict-dialog`: asks before anything is written; Replace / Keep both / Skip per file in one job, "do this for all" only while more clashes remain; Escape writes nothing; Replace is disabled, with the reason, where there is no Trash; Ctrl+Z restores every original |
| **Transfer card: bytes, speed, time left, Cancel** | **proven by hand** | `transfer_progress` (7, red on the old engine); evidence `B3-transfer-card`: on a drive capped at 8 MB/s it reads "51 MB of 152 MB · 6.7 MB/s · 16 s left"; the uncancelled copy is byte-identical; Cancel says "Stopping" at once and leaves no partial or staging file |
| **Multi-select** (Space, Shift+arrows, Ctrl+A, Ctrl/Shift+click) | **proven** | `just qa` §10h; selection painted distinctly from the cursor |
| Selection survives a re-sort; drops vanished files | **proven** | unit tests + `just qa` §10i: external delete takes 5 → 4 |
| Ctrl+A never selects what the filter hides | **proven** | `just qa` §10h |
| Operations act on the selection, else the cursor | **proven** | `just qa` §10h: 3 of 5 files copied, 2 untouched |
| Multi-entry Trash with one Undo for the batch | **proven** | `just qa` §10j: 3 trashed, one Ctrl+Z restores all 3 with contents |
| Undo journal for copy/move (plan_undo wired) | **proven** | unit tests: refuses once the copy is edited or gains a new file |
| **Undo executor** (`apply_undo`) | **proven** | 9 unit tests; refuses to clobber, never follows a symlink, partial failure reported |
| **Ctrl+Z undoes a copy** | **proven** | `just qa` §10k: 3 copies removed, originals untouched |
| **Ctrl+Z undoes a move** | **proven** | `just qa` §10l: both files returned with contents |
| Undo refuses once a copy is edited | **proven** | `just qa` §10k: edit survives, nothing deleted, offer stays |
| A cancelled job is never undoable | **proven** | unit tests (controlled timing); `just qa` §10m asserts coherence |
| Escape cancels a running transfer | **proven** | `just qa` §10m; job → selection → filter, one layer per press |
| A suggestion never becomes a stored choice | **proven** | `views::tests::a_suggestion_is_never_written_to_the_file` |
| **Permanent delete** | **proven** | `permanent::tests` (12) + live on an sshfs mount: symlink target survives |
| **Remote places (SSH/SFTP)** | **proven** | 38 lib + 6 live sshfs tests against a private server; `just qa` §10aa |
| **Scoped search (`:find`)** | **proven** | `search::tests` + `just qa` §10z: recursive, cancellable, symlink-loop safe |
| **`--default` on/off** | **proven** | `defaults::tests` (10) + `just qa` §10y: claims, restores, preserves later edits |
| **PKGBUILD + desktop entry** | **proven** | `makepkg` builds and packages; `desktop-file-validate` clean |
| **Drag and drop** | **proven** | `drop::tests` (14) + `just qa` §10x: copy by default, self-drop refused |
| **Mouse: view buttons, rows, breadcrumbs, sidebar** | **proven** | `just qa` §10v via `tests/uinput-mouse.py` |
| **Destructive confirm is safe by mouse too** | **proven** | `just qa` §10w: Cancel changes nothing, confirm trashes restorably |
| **Grow/shrink tiling, narrow window** | **proven** | `just qa` §10u: shrink below minimum and grow back, no binding loops |
| **Runs with optional dependencies missing** | **proven** | verified on a PATH with no bwrap/prlimit/magick: lists, browses, no crash |
| **A missing capability is explained, not hidden** | **proven** | `just qa` §10t; "Previews need bubblewrap" shown only where relevant |
| **The RELEASE artifact passes desktop QA** | **proven** | `just qa-release` — 143/143 against `target/release/gust` |
| A corrupt preferences file cannot break startup | **proven** | `just qa` §10s, verified with a 50 KB hostile file |
| Only safe kinds are thumbnailed | **proven** | `mime::thumbnail_mime_tests`: svg/pdf/executables refused |
| A thumbnail is never decoded in-process | **proven** | `render::tests` + bwrap isolation verified live |
| Refusals are shown, not buried | **proven** | `just qa` §10k: red alert outranks the selection count and clears on the next key |
| **A killed transfer publishes no partial file** | **proven** | `just qa` §10n: SIGKILL mid-copy of 20k files leaves zero debris, source intact |
| **Crash debris is swept on listing** | **proven** | `just qa` §10o: dead-process staging files removed, a live copy's file preserved |
| Gust restarts cleanly over a crashed folder | **proven** | `just qa` §10n |
| **A full disk leaves no debris** | **proven** | `engine::out_of_space_tests`, verified against a real 2 MB tmpfs |
| **Failures are explained in words, not errno** | **proven** | `engine::plain_message_tests` + `just qa` §10p |
| Close-during-work prompt | **partial** | see the note below — a platform gap, not a code gap |
| Opening a document via the desktop handler | proven | `just qa` §10g: xdg-open receives the path as one argument |
| Never executing a file to open it | proven | `just qa` §10g: a booby-trapped script is refused with or without +x |
| Open terminal here | built | wired to the palette; no QA assertion yet |
| Job drawer with progress and cancel | proven | see the transfer card above |
| **Notice after a crash** | **proven by hand** | `crash_recovery` (10); evidence `C-crash-notice`: SIGKILL during a 402 MB copy, relaunch says "Copying holiday-video.mkv and 1 more to usb was interrupted: nothing had finished."; Open destination shows the folder with the partial file already swept; Dismiss is remembered across restarts. Jobs still running in another window, stopped at a clash, or from older builds are never reported |
| **Places connect without freezing the window** | **proven by hand** | evidence `A5-offthread`: mount/connect/eject/disconnect run on workers; a server that never answers leaves the window responsive; killed helpers are reaped (no zombies); a dead FUSE mount counts as mounted until it is really gone |
| **Cloud control port is private** | **proven** | rclone's remote control is a Unix socket under `$XDG_RUNTIME_DIR/gust/rc/`, owner-only; no TCP listener, so no other user or web page can reach it |
| Opening a path from argv (`gust /some/dir`) | proven | live: honours the argument, falls back to $HOME |
| Permanent delete wired to UI | proven | `just qa` §10ab: offered only where Trash cannot work, red confirm with Cancel focused |
| Real extraction/compression | proven | see Archives under M1 |

## M3 · connect

| Item | State | Evidence |
| --- | --- | --- |
| SSH config parsed without executing | proven | `Match exec` canary never fires |
| Include cycles, wildcards, multi-alias hosts | proven | unit tests |
| Capability probing (Unknown ≠ No) | proven | unit tests on rclone fsinfo shapes |
| Upload state: cached ≠ uploaded | proven | unit tests; confirmation bound to a revision |
| Actual sshfs/gio/rclone mounting | proven | `sshfs_live`, SMB/NFS live tests, rclone against WebDAV; `just qa` §10aa |
| Discovery UI, Add a place | proven | `just qa` §10aa: saved computers listed without connecting |

## M4–M5

Filter, scoped find, palette actions, archives, packaging and `--default` are
proven (see the tables above). **Not started:** the file-chooser portal (other
apps' Open/Save dialogs), the `org.freedesktop.FileManager1` D-Bus service
("Show in folder" from a browser), a Settings screen, folder sizes, a
Properties window, bookmarks, and dragging files out to other apps.

## Known gaps worth stating plainly

This list is rewritten whenever something ships. Items 1, 2 and 4 of the
original list (placeholder thumbnails, only-Trash-is-wired, no packaging)
were all closed and have been removed rather than left to rot: a stale gap
list is worse than none, because it makes working features look broken.

1. **Device mount/eject is unproven on real hardware.** Discovery is built
   and tested — the sidebar lists removable volumes and discs, and the
   exclusions that matter (system disk, swap, zram, loop, LVM) have unit
   tests. But this VM's only removable block device is an empty virtual DVD
   drive, so *mounting a stick and ejecting it* has never run. The argv is
   tested; the round trip is not.
2. **Tailscale is not built.** There is no tailnet here, so peer discovery
   could not be qualified even if written; the plan is explicit that online
   status grants neither SSH access nor a username, so the interesting part
   is the authorisation check, which is exactly what cannot be tested.
   Phones over MTP need GVfs and per-device Trust prompts and are unbuilt
   for the same reason: no phone.
3. **Cloud is proven mechanically, not per provider.** The rclone integration
   is tested end to end against a local WebDAV backend. Drive, Dropbox,
   Proton and Filen each have their own auth and capability quirks; a fixture
   passing is not provider certification.
4. **The memory target is missed on software rendering.** Every number
   here comes from llvmpipe in a VM with no GPU. First window 155 ms and
   100k entries settled in 470 ms are well inside target, but PSS is
   212 MiB against ≤ 90 MiB — and a bare Qt Quick window with no Gust code
   already costs 155 MiB here, 59 MiB of it llvmpipe. Machines with no GPU
   are a real target (not everyone has one), so this is not waved away:
   `QT_QUICK_BACKEND=software` (Qt's raster renderer, no LLVM) is the next
   thing to measure. It has not been measured yet.
5. **Tab drag-to-split and drag-out-to-detach are not built.** The split
   itself works from the palette and the keyboard; the drag gestures need
   cross-window Qt drag sessions and are recorded unbuilt rather than
   half-built (DESIGN §6.7).
6. **A real `pacman -U` install/uninstall cycle has not been run.** The
   package builds, the binary inside it has been extracted and run, and the
   `--default` claim/rollback round trip is tested against a fake HOME — but
   installing to the live system would touch the user's machine.


## Measured performance (this VM only)

Run `just bench`. Numbers below are a release build inside the development VM
(AMD Ryzen 5 7600, 8 vCPU, 2.8 GiB RAM, **llvmpipe software rendering**).

| Measure | Result | Target | |
| --- | --- | --- | --- |
| names listed, 100k entries | 127 ms | < 400 ms | pass |
| names + metadata, 100k | 230 ms | < 1500 ms | pass |
| first window on screen | 155 ms | < 400 ms | pass |
| 100k listed and settled in the UI | 470 ms | < 2000 ms | pass |
| still answers keys at 100k | End reaches row 99999 | responds | pass |
| PSS with 100k open | 212 MiB | < 90 MiB | **miss** |

The memory target is missed, and a control experiment says why.

**A bare Qt Quick window with no Gust code in it costs 155 MiB PSS on this
machine.** That is the M0 spike: one QML file, one rectangle, nothing else.
Gust's whole file manager — sidebar, three views, model, theme adapter, job
runner — brings the baseline to 165 MiB. Gust's own contribution is about
**10 MiB**.

Breakdown of that 155 MiB floor:

- 59 MiB is the software renderer (`libLLVM` + gallium + mesa shared memory),
  which a machine with a real GPU does not pay.
- 96 MiB is Qt Quick itself on this platform.

So the ≤ 90 MiB budget is not reachable by *any* Qt Quick application here,
however it is written. Three honest conclusions:

1. The number must be re-measured on hardware with a real GPU before the
   budget can be judged at all. The 59 MiB renderer cost disappears there.
2. If the budget still fails on real hardware, it is a target for the
   *toolkit choice*, not for Gust's code, and the plan should say so.
3. Gust's own growth is worth watching regardless: ~47 MiB for 100k entries
   (~490 B/entry, mostly heap-allocated names). Interning names or boxing
   metadata would shrink it, but it is not what breaks the budget.

The listing and latency figures come from `gust-core` directly and are
hardware-honest. The memory figure is a property of this VM.


## Close during work — what was actually found

The plan requires twice that "a window close must not silently abandon
pending transfers". Two things were built for it, and one of them does not
work on this platform:

1. **`onClosing` handler** (implemented). Blocks the close, asks whether to
   stop the transfer, cancels the job and waits for the worker before
   quitting. This is correct and fires for toolkit-reported closes.
2. **It does not fire for Hyprland's `closewindow`.** Verified with a
   control: a bare seven-line Qt Quick window with nothing but an
   `onClosing` handler behaves identically — the handler never runs and the
   window stays open. **This is a Qt 6.11 / Wayland gap, not Gust's code.**

What protects the user regardless of how the process dies:

- A copy is staged under `.gust-staging-<pid>-<name>` with owner-only
  permissions and renamed into place atomically, so **a partial file is
  never published under its real name** — proven by SIGKILL mid-copy of
  20,000 files.
- Every listing sweeps staging files whose owning PID is gone, so crash
  debris never accumulates and is never shown. A staging file belonging to a
  *running* process is left alone, because taking it would corrupt a copy
  that is still in flight.
- A cancelled job removes its own staging file.

So the data-safety requirement holds. The *courtesy* of asking before
abandoning a transfer holds only for closes the toolkit reports. Worth
re-testing on a future Qt, or with a `QCoreApplication` event filter, before
claiming it universally.
