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
| **Folder sizes** | **proven by hand** | `dir_size` (8): symlinks never followed, hard links counted once, other filesystems not entered, unreadable parts make it "≥", bounded and cancellable. Live: Photos shows 7.6 MB (2 files) where `du -l` over-counts to 11 MB. Measured on a worker, local folders only. A change deep inside a sub-folder is picked up by Ctrl+R, not live |
| **Properties** (right-click, Alt+Enter) | **proven by hand** | `properties` (8): location, local times, owner by name from /etc/passwd, permissions, link target shown not followed, hostile names made visible. Lives in the Quick Look panel rather than a second window |
| **Times in the list are local** | **proven** | were UTC ("today 14:49" at 16:49 CEST); `relative_time` uses the local offset and local calendar days, older dates written out |
| **Other apps' Open/Save dialogs** (portal FileChooser) | **proven by hand** | `chooser` (7), `portal_conf` (6); evidence `C-file-dialogs`: through the real xdg-desktop-portal, an app's Open got `wanted.pdf` picked by mouse, Save returned `sub/meeting notes.txt` with a typed name, Cancel returned code 1. Opt-in (`gust --file-dialogs on` / Settings), changes one line of the user's portals.conf, fully reverted by `off`. Installed files are inert until chosen |
| **Super+Shift+F opens Gust** (Omarchy) | **proven by hand** | `omarchy_keys` (6); `gust --bind-keys on` or Settings adds one marked block to `~/.config/hypr/bindings.lua` (Omarchy's documented override file) and reloads Hyprland; a real Super+Shift+F keypress in the VM opened Gust, not Nautilus. `--bind-keys off` left the file byte-identical to before. Super+Alt+Shift+F opens the terminal's folder via `omarchy-cmd-terminal-cwd` |
| **Drag and drop with other apps** | **proven by hand** | evidence `B-drag-between-apps`: real mouse drags Gust → Nautilus and Nautilus → Gust, both byte-identical copies with the source kept. Out: text/uri-list, text/plain and the GNOME type. In: the normal planner and engine. Grid and Columns now have the drag gesture too (they had none) |
| **"Show in folder" from other apps** (`org.freedesktop.FileManager1`) | **proven by hand** | `filemanager1` (10); evidence `C-show-in-folder`: after `gust --default on` (or Settings → Default app), a real `ShowItems` call on the session bus D-Bus-activated `gust --dbus-service` and opened Gust in the folder with the file under the cursor, 60 ms. `--default off` removes only Gust's activation file; another app's is never touched |
| **Bookmarks** | **proven by hand** | `bookmarks` (6); evidence `B-bookmarks-settings`: the desktop's own `~/.config/gtk-3.0/bookmarks` (shared with Nautilus and GTK file dialogs) is the store; pin by right-click or Ctrl+B appends exactly one line; unpin restores the file byte-for-byte; sftp:// and smb:// entries from other apps are kept verbatim |
| **Settings** (gear button, Ctrl+, , `:settings`) | **proven by hand** | `settings` (7): keys (Standard/Vim), density, hidden files, sidebar, renderer (next start), default file manager. Every choice is honoured; saved 0600, atomic, one bad line resets only itself. Live: Compact/Vim applied at once and survived a restart; "Use Gust" / "Previous app" claimed and restored `mimeapps.list` byte-for-byte |
| **Names that are not valid UTF-8** | **proven** | copy, clipboard URIs and bookmarks keep the raw bytes (a Latin-1 `café` became `caf%EF%BF%BD`, naming no file); test red on the old code |
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
| Columns view | **proven by hand** | real Miller columns: the parent folder's entries (click a sibling to go there) and a preview column (a folder's contents, or a file's kind, size and first lines), read bounded on a worker; `columns` (5); evidence `B-columns` |
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
| Vim preset | proven by hand | selectable in Settings; `j` moves the cursor; persists across restarts |
| **Per-location view memory** | **proven** | `views::tests` + `just qa` §10r; survives a restart |
| Tabs | **proven** | per-tab location, history, selection, filter and view; Ctrl+T/W/Tab/1-9; last tab cannot be closed; a tab with a running job asks before closing |
| "Send with LocalSend" (mockup C) | **not possible** | LocalSend's desktop entry is `Exec=localsend` with no `%F` and no `MimeType`, it has no CLI, and it registers no D-Bus service. There is no supported way to hand it files. Recorded as unbuildable rather than pending. |
| **One copy, every format** | **proven** | `clipboard_live` 9/9 (the new check failed on the old code, which offered only the GNOME type): a copy is readable as files by other file managers, as `text/uri-list` by browsers and chat apps, and as plain text by terminals — and it survives closing the Gust window (held by `gust --clip-serve`, which exits when another app copies) |
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
| Quick Look (Space) | **proven** — now shows text in any language (it printed dots for every non-ASCII byte); bounded text peek, kind in words, mode line; escape sequences shown not obeyed; symlinks described not followed; images described without decoding here; multi-select moved to Ctrl+Space |
| Content search (`:grep`) | **proven** | scoped, recursive, cancellable; binaries skipped on a NUL sniff; symlinks not followed; bounded per file and overall, and says when it stopped early |
| Cloud remotes (rclone) | **proven** | discovery reads the installed rclone's config and opens no socket; connect mounts with a VFS write cache; "cached ≠ uploaded" shown as a notice; disconnect refuses while uploads are outstanding and never lazy-unmounts; degrades with a reason when rclone is absent |
| Cloud OAuth in Gust's own UI | not built | deliberate: `rclone config` handles every provider correctly, and Drive's flow is not a template for Proton or Filen (plan §3) |
| Two panes | **proven** | `:split` / `:unsplit`, Tab crosses the divider, F5 copies and F6 moves to the other pane through the normal journal and undo; a pane with no tabs closes itself; closing a pane keeps its tabs |
| Tab drag to split / detach | **proven by hand** | real mouse drag of a tab out of the window: a new window opened in the same folder, grid view, same cursor and selection, and Back went to the folder the tab came from; the tab closed only after the new window reported ready (`handoff` 4: a child that dies or never appears keeps the tab). DESIGN §6.20 corrected to match the code on `splitPane()` |
| 100k benchmark | **proven** | `just bench`: 100k listed+settled in 470 ms (release), first window 155 ms |
| Listing off the UI thread | **proven** | worker thread + generation-tagged snapshots; names paint before metadata |
| PSS budget (≤ 90 MiB) | **met on machines without a GPU** | 85.8–86.5 MiB with 100k entries open (3 runs). Gust detects "no GPU" from sysfs and uses Qt's raster renderer instead of llvmpipe; see below. With a GPU: not yet measured |
| **No-GPU renderer auto-selection** | **proven** | `renderer::tests` (8) on sysfs fixtures incl. this VM's virtio-without-3D shape, virgl, i915/amdgpu/nvidia, hybrid laptops, unknown drivers (never guessed); live: auto → software 66 MiB, `GUST_RENDERER=gpu` or the user's own `QT_QUICK_BACKEND` → respected, 177 MiB. Full `just qa` passes on the raster renderer |

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
| **PKGBUILD + desktop entry** | **proven** | `makepkg` builds and packages; `desktop-file-validate` clean; complete optdepends (every external program Gust calls); 0 home paths in the shipped binary; `pacman -U`/`-R` cycle clean in the VM |
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
| Close-during-work prompt | **proven by hand** | covers every tab: with a copy running in a background tab, closing the window (Hyprland `closewindow`) asked "A transfer is running in another tab"; Stop and close ended it with no partial or staging file. A background tab's job now finishes and refreshes while another tab is shown (it used to stay "Running" until its tab was shown again) |
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
proven (see the tables above), as are the Omarchy hooks: Super+Shift+F,
"Show in folder" and other apps' Open/Save dialogs.

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
4. **Memory with a real GPU is unmeasured.** Without a GPU the budget is
   met (86 MiB, raster renderer). On GPU hardware Qt uses its hardware
   renderer, whose cost this VM cannot show.
5. **Tab detach is proven by hand, not in `just qa`.** The last-tab refusal
   branch is exercised only by reading the code: with one tab the strip is
   hidden, so there is nothing to drag.
6. **Release gates are proven in the VM, not on a clean machine.** A real
   `makepkg` built the package (release, `!lto`, `--remap-path-prefix`): the
   shipped binary contains **0** `/home/` paths (the dev build had 1336), is
   5.7 MB, and `pacman -U` / run / `pacman -R` completed cleanly in the VM,
   leaving no default claimed. An AUR `-bin` package and an Omarchy repo
   submission are not done: both need accounts only the maintainer has.


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
| PSS with 100k open (llvmpipe, forced) | 208–229 MiB | < 90 MiB | miss |
| PSS with 100k open (raster, the no-GPU default) | 85.8–86.5 MiB | < 90 MiB | **pass** |
| first window (raster) | 104–105 ms | < 400 ms | pass |
| 100k listed and settled (raster) | 423–430 ms | < 2000 ms | pass |

The memory target was missed under llvmpipe, and a control experiment said
why; the fix (Qt's raster renderer when there is no GPU) is now automatic.
The analysis that led there:

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
2. **Earlier this did not fire for Hyprland's `closewindow`** (Omarchy 4.0.3,
   Qt 6.11). Re-tested on 2026-09-24 in the same VM with the current build:
   `hl.dsp.window.close` DID reach `onClosing` and the prompt appeared. The
   earlier result is kept here because it was measured; which of the two
   changed (Hyprland's Lua dispatcher, or Gust) was not isolated.

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
