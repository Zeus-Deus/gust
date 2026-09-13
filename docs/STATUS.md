# Status — what is built, what is proven, what is not

Written against the plan's rule that a passed fixture is not a product-wide
pass. Three states only:

- **proven** — there is a test or a captured artifact, and it runs in `just test`,
  `just qa` or `just theme-matrix`.
- **built** — the code exists and compiles, but no end-to-end proof yet.
- **not started** — nothing beyond types.

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
| Generation-tagged model updates | built | generation plumbed; no concurrent-navigation stress test yet |
| **Thumbnails (image + video)** | **proven** | sandboxed helper, `just qa` §10q; 200 images in <6 s with the UI live |
| Clipboard / DnD / IME / fractional scaling | not started | — |
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
| Vim preset | built | implemented, not yet asserted in QA |
| **Per-location view memory** | **proven** | `views::tests` + `just qa` §10r; survives a restart |
| Tabs | **proven** | per-tab location, history, selection, filter and view; Ctrl+T/W/Tab/1-9; last tab cannot be closed; a tab with a running job asks before closing |
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
| Rename wired to UI (F2) | proven | `just qa` §10f: lands on disk, old name gone |
| Rename conflict refused, nothing overwritten | proven | `just qa` §10e: prompt stays open, existing contents survive |
| New folder (Ctrl+N), Duplicate (Ctrl+D) | built | implemented and manually exercised; not yet asserted in QA |
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
| Job drawer with progress and cancel | proven | live screenshot; cancel path unit-tested |
| Opening a path from argv (`gust /some/dir`) | proven | live: honours the argument, falls back to $HOME |
| Permanent delete wired to UI | not started | deliberately refused with a message rather than guessed |
| Real extraction/compression | not started | policy layer only |

## M3 · connect

| Item | State | Evidence |
| --- | --- | --- |
| SSH config parsed without executing | proven | `Match exec` canary never fires |
| Include cycles, wildcards, multi-alias hosts | proven | unit tests |
| Capability probing (Unknown ≠ No) | proven | unit tests on rclone fsinfo shapes |
| Upload state: cached ≠ uploaded | proven | unit tests; confirmation bound to a revision |
| Actual sshfs/gio/rclone mounting | not started | — |
| Discovery UI, Add a place | not started | sidebar shows the honest empty state |

## M4–M5

Filter and scoped find are proven at the library level (depth bounds, no
symlink following, cancellation). Palette actions, archives, portal, packaging,
`--default` integration: **not started.**

## Known gaps worth stating plainly

1. **Thumbnails are placeholders.** The sandbox contract and output validation
   are tested, but no thumbnailer runs yet.
2. **Only Trash is wired.** Trash + undo work end to end on the real desktop.
   Copy, move, rename and permanent delete have tested engines but no UI path;
   the permanent-delete branch reports that it is unwired rather than guessing.
3. **Software rendering only.** Every performance impression from this VM is
   meaningless for a native GPU.
4. **No packaging.** No PKGBUILD, no `--default`, no rollback test.
5. **Memory/latency budgets unmeasured.** The plan's ≤250 ms paint and ≤90 MiB
   PSS targets remain hypotheses; `bench/` does not exist yet.


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
