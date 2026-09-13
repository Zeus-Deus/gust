# GUST — shared design contract

This file is the agreement between crates. Change it before changing a shared type.

Status legend used throughout the repo: `proven` (test + artifact), `partial`,
`untested`. Nothing is marked proven without a test that runs in `just test` or
an artifact under `artifacts/`.

## 1. Identity

```
LocationKey  = provider_id + ":" + opaque_location_bytes
```

- `provider_id` — `local`, `sftp`, `gio`, `rclone`. Stable across restarts.
- Location bytes are **OS bytes**, not `String`. Non-UTF-8 names survive a
  round trip unchanged. Display escapes control/bidi chars; the filesystem
  bytes are never rewritten for display.
- A row index, a translated label and a temporary mount path are never identity.

`EntryId` = `(dev, ino)` where the provider supplies them, else a hash of the
location bytes. Metadata and thumbnail results carry `EntryId` + `generation`
and are dropped if either no longer matches.

## 2. Capabilities

```rust
pub enum Cap { Yes, No, Unknown }

pub struct Capabilities {
    pub read: Cap, pub write: Cap,
    pub atomic_rename: Cap,      // rename(2) semantics, not "a rename button exists"
    pub trash: Cap,              // recoverable trash, not delete
    pub unix_permissions: Cap,
    pub server_side_copy: Cap,
    pub hardlinks: Cap, pub symlinks: Cap,
    pub case_sensitive: Cap,
    pub max_name_len: Option<usize>,
}
```

`Unknown` is distinct from `No`. The UI disables an action for `No`, and for
`Unknown` it attempts and reports honestly. Capabilities come from probing the
connected account, never from the provider's marketing name.

## 3. DirectorySnapshot

```rust
pub struct DirectorySnapshot {
    pub key: LocationKey,
    pub generation: u64,        // monotonic per pane; stale results discarded
    pub entries: Vec<Entry>,
    pub sort: SortSpec,
    pub complete: Completeness, // Partial{seen} | Complete | Failed{error}
}
```

Names and types arrive first. Size/mtime/mode and thumbnails are deferred and
arrive as generation-tagged patches. Provisional order is visibly distinct from
settled order (the header shows a sorting indicator until settled).

## 4. Pane / Tab

Each tab owns: location, view mode, sort, selection set, keyboard cursor,
scroll position and back/forward stack. **Selection set and keyboard cursor are
different states.** An explicit user view choice is stored per `LocationKey`
and always beats a default suggestion.

### 4.0 Persisted preferences

Stored at `$XDG_CONFIG_HOME/gust/views.toml` (falling back to
`~/.config/gust/views.toml`).

- The key is `LocationKey::to_token()` — hex-encoded OS bytes, so a folder
  whose name is not valid UTF-8 keeps its preference like any other. A
  preference file is never a place where a path becomes a `String`.
- **Only an explicit choice is recorded.** A suggested view (Grid for
  Pictures) is not written, or the suggestion would silently harden into a
  stored decision the user never made.
- The file is a *cache of preferences*, never a source of truth about the
  filesystem: an unreadable, truncated or hostile file must degrade to
  defaults, never refuse to start and never be a way to make Gust open
  something. Unknown keys and unknown view names are ignored.
- Bounded: at most 500 entries, oldest evicted first. A user who browses
  100k directories must not grow an unbounded config file.
- Written atomically (stage + rename) with owner-only permissions, so a
  crash cannot leave a half-written file and another user cannot read
  which directories were visited.

### 4.1 Selection

The selection is a set of entry **names**, not row indices. Rows renumber when
the sort changes, the filter narrows or a background job adds a file; names do
not. A name that leaves the listing leaves the selection.

- **Anchor** is the row a range extends from. Shift-movement extends from the
  anchor without moving it.
- Plain arrow movement moves the cursor and **keeps** the selection:
  `Space`, arrow, `Space` is the primary keyboard gesture for picking several
  files, and clearing on every arrow would make it impossible. A plain
  *click* does replace the selection — that is what a click means — and
  `Escape` clears it explicitly.
- `Space` toggles one entry and moves the anchor there. `Ctrl+click` toggles.
  `Shift+click` and shift-movement select the inclusive range anchor→cursor,
  replacing any previous range but preserving entries toggled with Ctrl.
- `Ctrl+A` selects everything currently visible — never entries hidden by the
  filter or by the hidden-files setting. What the user cannot see cannot be
  selected by a keystroke.
- Navigating to another location clears the selection. Returning does not
  restore it: a stale selection over a changed directory is dangerous.
- When the selection is empty, a command acts on the cursor row. This keeps
  every single-entry flow working unchanged.

An operation over a multi-entry selection resolves each name to a path **once**,
at the moment the command is issued, and operates on that list. It never
re-reads the selection mid-job.

## 5. Job

```
planned → running → committing → (remote-pending) → confirmed
                 ↘ cancelling → cancelled
                 ↘ conflict | failed | retrying
```

- Stable `JobId`. A failed id is never reused for a new unreviewed action.
- Per-entry results, written to the journal **before** the destructive commit.
- `remote-pending` means bytes left the machine but the backend has not
  confirmed the object. It is never displayed as "uploaded".

## 6. Operation safety (crates/ops)

- Copy into a private staging name, fsync per policy, then atomic rename into
  place. Restrictive permissions from creation — not chmod at the end.
- Recursive traversal is anchored to **held directory descriptors**
  (`openat`/`unlinkat` with `AT_` flags). A final-component `O_NOFOLLOW` is
  insufficient: an intermediate directory can be swapped mid-walk.
- Cross-device move = copy, verify, commit, then unlink **only** the source
  that was verified unchanged (dev+ino+mtime+size).
- Special files (FIFO, socket, device, symlink) are classified and handled by
  explicit policy, never copied blindly.

### 6.1 Drag and drop

A drop is an ordinary copy or move through the same engine, journal and
undo as any other — never a shortcut around them.

- **A cross-location drop defaults to Copy.** The safe operation is the
  default; Move is offered explicitly and never inferred.
- The drop target and the operation are **named before it runs**: "Copy 3
  items to backups". A drop that does not say where things went is a data
  loss report waiting to happen.
- A drop onto the folder the items already live in is a no-op, not a
  self-copy producing "file (copy)" clutter.
- A drop onto a **file** targets its containing folder, never the file: an
  accidental drop must not be read as "replace this".
- Dropping a folder into itself or into its own descendant is refused with
  a reason, because it would recurse forever.
- Text dropped from another application is not a file operation. Only URIs
  Gust can resolve to real paths are accepted.

## 6.2 Becoming the default file manager

`gust --default` registers Gust for `inode/directory`; `--default off`
restores what was there before. AGENTS.md promises this is reversible, so
the rollback is the part that must be right.

- **Record the previous value before changing anything.** Rollback restores
  exactly what was found, including "nothing was set", which is different
  from "set to some fallback".
- Only the keys Gust claims are touched. Every other association in
  `mimeapps.list` is preserved byte-for-byte, including comments and
  ordering — a user's file is not ours to rewrite.
- **Later edits win.** If the association was changed by something else
  after Gust claimed it, `--default off` leaves that newer choice alone
  rather than stamping an old value over it (§8: "uninstall/rollback
  preserves later edits").
- Written atomically, and never as root.
- The state file lives beside Gust's own config, not in the user's XDG
  defaults, so removing Gust cannot corrupt unrelated associations.

## 6.3 Scoped search (`:find`)

Search is an explicit, scoped, cancellable walk — never a silent recursive
crawl (plan §2, §4).

- **Scope is chosen, not assumed.** `:find NAME` searches the current folder
  and below. Broadening to Home is a separate verb the user picks.
- **Never crosses a device boundary**, so a search cannot wander into a
  network mount or another disk that was merely mounted underneath.
- Runs on a worker with a cancel token; Escape cancels it. Results stream in
  so the first hits appear immediately rather than after the whole walk.
- Bounded by depth and result count: a search that would take forever
  instead stops and says it was truncated.
- Remote locations are **opt-in per location** (§9): a search never crawls a
  remote place unless that place has been enabled for it.
- Searching does not change the folder being browsed. Leaving the results
  returns to where the user was.

## 6.4 Remote places

A remote place is a *suggestion* until the user connects it. Nothing in Gust
opens a connection on its own (plan §3).

- **Discovery never connects.** SSH aliases are read from the user's config
  as literal text; `ssh -G` is never run, because it executes
  `Match exec` blocks. A discovered host is a row in a list, not a mount.
- **Connect is always an explicit action**, and it is the first moment any
  network traffic happens.
- **Host identity is verified, never bypassed.** `StrictHostKeyChecking` is
  never disabled and `UserKnownHostsFile` is never pointed at `/dev/null`.
  A changed host key stops the connection and says so plainly.
- **No secret ever reaches argv, the environment, a log or a config file.**
  Keys are referenced by path; passwords go to the agent or the keyring.
- **A blocked mount must not block Gust.** Every remote call runs on a
  worker with a timeout, and a mount that stops responding degrades to an
  error in one place rather than freezing the window.
- **Capabilities are per-backend.** A place that cannot trash shows a
  separately labelled permanent-delete action; it never silently substitutes
  one for the other.
- Thumbnails and content search are **opt-in per remote place**, never
  automatic.

## 6.5 Permanent delete

The one operation with no undo. Reached only where Trash is genuinely
unavailable — a different filesystem, a remote place — and never as a
substitute Gust chooses on its own (plan §3 rule 2).

- The confirmation **says the word permanent**, explains why Trash cannot be
  used here, and focuses **Cancel**. Enter therefore cancels.
- The target must be a **direct child of the folder on screen**, so a stale
  row index or a crafted name cannot reach elsewhere.
- A **symlink is unlinked, never followed**, including symlinks found inside
  a folder being removed — deleting a shortcut must not delete what it
  points at.
- Refuses `/`, any top-level directory, `$HOME`, the folder itself, and any
  path that still contains `..`.
- `std::fs::remove_dir_all` is deliberately **not** used; the walk is
  explicit so symlink behaviour cannot change under us.
- A refusal is an **alert**, not a status line: the listing that follows
  rewrites status, which is how a failed delete once reported nothing.

## 6.6 Tabs

Each tab owns a location, history, selection, filter and view (plan §2).
Nothing is shared between tabs except the window itself.

- A tab is **one `DirModel` instance**. The model already holds location,
  history and selection, so a tab needs no new state type — only its own
  instance and its own remembered view mode.
- `dirModel` is a *property* resolving to the active tab's model, so every
  existing call site keeps working and there is one definition of "current".
- **A tab never does I/O until it is looked at.** Opening five tabs must not
  start five listings; a background tab lists when first activated.
- Closing the last tab closes nothing — the window keeps one tab, because a
  file manager with no tab has nothing to show.
- **A running job belongs to the tab that started it.** Closing that tab
  asks first, exactly as closing the window does; the transfer is never
  silently abandoned.
- Tab order is the order they were opened. A new tab opens beside the
  active one, not at the end, so "open in new tab" lands where the eye is.

### 6.6.1 QML implementation note

Tab state lives in two places that must agree: a `ListModel` (how many tabs,
their labels) and an `Instantiator` of `DirModel` objects (the actual state).
The ListModel is authoritative for *count* because `Instantiator.count` and
`Instantiator.objectAt()` do not notify — a binding over either evaluates once
and never updates. Anything the UI must track is a property binding on a
notifying property; derived values (a tab's label) are `readonly property`
bindings **on the model itself**, never computed in a change handler, because a
handler can run before the model has published its new value.

### 6.6.2 The lint rule

`tests/lint-qml.sh` now refuses a declarative binding written over a function
call. The rule exists because this mistake cost two long debugging sessions
and produced five different-looking symptoms, none of which mentioned
bindings: an empty list, a missing tab strip, a frozen label, a pane that
rendered nothing, and a job drawer that kept the colour of a state it had
left. None logged a warning.

The check skips signal handlers (`on<Capital>:`), object keys inside function
bodies, `Tokens.qml` (its source emits change notifications so those bindings
do re-evaluate), and a short allow-list of helpers the window refreshes
explicitly. When it fires, the fix is a plain property refreshed at the
points that change it, or a binding on something that notifies.

Adding the rule immediately found two live bugs in `JobDrawer.qml`: the
drawer's colour and its status word were both function calls, so a job that
failed or was cancelled still showed "Working" in the running colour.

## 6.7 Two panes

A pane is a column of the window that owns a set of tabs. There are at most
two, side by side, never arbitrary docking (plan §2). Exactly one pane is
*active*: it holds keyboard focus, its tab strip is the one being edited, and
every existing action — navigate, copy, rename, delete — applies to it and to
nothing else.

### What a pane owns

Each pane has its own tab list, its own active tab index and its own view
memory. Because a tab already owns location, history, selection, filter and
view (§6.6), a pane needs no per-pane state beyond "which tabs, which one is
showing". Splitting therefore moves tabs between two lists; it does not
introduce a second kind of state.

### The rules

1. **One pane is active.** Focus follows an explicit action (Tab, a click);
   it never moves because a job finished or a listing settled.
2. **A second pane is opened deliberately** and closed the same way. Closing
   a pane moves its tabs into the surviving pane rather than discarding them
   — a pane is a layout, and closing a layout must not lose a location, still
   less a running job.
3. **The single-pane window is unchanged.** With one pane there is no
   splitter, no pane border, and no hint that panes exist. Chrome that earns
   nothing is not shown.
4. **F5 copies and F6 moves to the *other* pane.** Both are ordinary
   operations through the same engine, journal and undo as any other copy or
   move (§6.1); neither bypasses a confirmation that a drop would show.
   With one pane there is no other side, so both refuse with a reason rather
   than silently doing nothing.
5. **The destination is named before the operation runs.** "Copy 3 items to
   Downloads" — never "copy to the other pane", which tells the user nothing
   about where their files went.

### What is deliberately not in this version

Dragging a tab to a drop target to split, and dragging one outside to detach
into its own window, are described in the plan. They need Qt drag sessions
across window boundaries, which is a materially larger piece of work than the
split itself. The split is reachable from the palette and the keyboard; the
drag gestures are recorded as unbuilt rather than half-built.

## 6.8 Cloud remotes through rclone

rclone is the integration layer (plan §3). Gust does not implement any
provider: it reads the **installed** rclone's configured remotes and its
declared capabilities, and offers only what that rclone actually supports.
A provider Gust has never heard of works if rclone has it; a provider rclone
lacks does not appear.

### Reading the configuration is not connecting

`rclone listremotes` reads a local file. It opens no network socket, exactly
as SSH discovery does not (§6.4). Remotes appear as *suggestions*; the first
network traffic happens when the user connects, deliberately.

### Credentials

Gust never reads, writes, stores, echoes or logs a token, password or
client secret. It never passes one on a command line. Configuring a remote
is `rclone config`'s job — Gust launches it in a terminal and re-reads the
list afterwards. An obscured password in rclone's config is **not**
encryption (plan §9); if the config is encrypted, Gust asks rclone to unlock
it through rclone's own prompt and never handles the passphrase.

### "Cached" is not "uploaded"

The rule the plan calls non-negotiable. With `--vfs-cache-mode writes`, a
save returns as soon as the bytes are in the local cache; the upload happens
afterwards and can fail. A file manager that shows such a file as saved is
lying.

Gust therefore distinguishes three states per file and never conflates them:

| State | Meaning | Shown as |
|---|---|---|
| `Cached` | written locally, not yet on the backend | "not uploaded yet" |
| `Uploading` | transfer in flight | "uploading…" |
| `Uploaded` | the backend confirmed it | nothing (the normal case) |
| `Failed` | the upload failed | an alert, with the reason |

**An empty transfer queue does not mean uploaded.** rclone reports an empty
queue both when everything is uploaded and when nothing was ever queued.
Confirmation must come from the backend: the file is `Uploaded` only when a
fresh listing of the remote reports it with a matching size, and Gust asks
for that listing rather than assuming.

### Closing with unfinished uploads

Closing a window, disconnecting a place or unmounting with anything in
`Cached` or `Uploading` asks first and names the count. Gust never
lazy-unmounts a dirty mount and never kills a mount it did not create
(plan §9).

### What is deliberately not built

OAuth flows in Gust's own UI. The plan is explicit that Drive/Dropbox/
OneDrive OAuth is not a template for Proton or Filen, and a half-built OAuth
path that works for one provider is worse than sending the user to
`rclone config`, which handles every provider correctly.

## 6.9 Quick Look

Space shows what a file *is* without running it (plan §4). It never launches
the file, never executes a preview script, and never decodes untrusted bytes
outside the sandbox that already guards thumbnails (§5).

### What it shows

Facts Gust can state without interpreting the file's contents:

- the name, and the kind in words ("PNG image", not "image/png")
- size and modified time in the same words the list uses
- the mode line, so a user can see it is a symlink or not writable
- pixel dimensions for an image, read from the sandboxed thumbnailer's
  output rather than by decoding here
- the first few lines for a text file, read as bytes and shown with
  unprintable characters replaced — never interpreted, never rendered
- which application would open it, from the same lookup Enter uses, stated
  as "Opens with imv" and NOT acted upon

For anything else it says what it is and stops. "No preview" is an honest
answer; a preview that required running the file is not.

### Rules

1. **Space is Quick Look, and selection toggling moves to Ctrl+Space.** The
   plan's mockups say `space quick look` in three separate places. Multi-
   select by keyboard keeps working through Ctrl+Space and Shift+arrows.
2. **The peek is bounded.** Text peeking reads at most 64 KiB and at most
   the first 40 lines, so a 4 GB log opens instantly.
3. **Nothing is decoded on the UI thread.** A peek is a worker read that
   publishes a snapshot, exactly as listing is.
4. **Escape closes it**, before the selection and before the filter, because
   it is the most recently opened thing (§4 Escape ladder).

## 6.10 Scoped content search (`:grep`)

`:find` matches names; `:grep` matches *contents* (plan §2, §4). Both are
explicitly scoped and cancellable, and neither ever starts on its own.

### Rules

1. **It starts where you are**, and broadening is an explicit act. There is
   no silent recursive crawl, and never one through a cloud mount: a
   location on a remote is skipped unless the user asked for that location
   specifically, because content search there means downloading everything.
2. **Binary files are skipped.** A file is treated as binary if its first
   8 KiB contain a NUL. Grepping a disk image finds nothing useful and costs
   everything.
3. **Bounded per file and overall.** At most 8 MiB read per file, and the
   search stops at 500 matching files; the result says so rather than
   pretending it finished.
4. **Cancellable at any moment**, with the same token the name search uses,
   and Escape cancels it.
5. **Results are entries**, published into the same model as `:find`, so
   Rows, Grid and Columns all work unchanged and so do selection and
   operations on the results.
6. **Matches are counted, not concatenated.** The result list shows the file
   and how many lines matched; Gust does not build a giant text buffer.

## 6.11 Accessibility

Every control carries an `Accessible.role` and an `Accessible.name` that says
what it does ("Up one folder", not "button3"), and the file list exposes its
rows as list items rather than anonymous shapes. `tests/a11y.sh` reads the
live AT-SPI tree — the same interface a screen reader uses — and asserts on
names and roles, because a declaration in QML is not proof that anything
reached the bus.

**Qt only joins the accessibility bus when asked.** It needs
`QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1`, which a screen-reader user's session
already sets. Gust does **not** set it itself: the project sets no global
`QT_*` variable on the user's session, and a file manager quietly enabling a
D-Bus bridge for everyone is not its decision to make. The test enables it
for its own run.

Font size and spacing come from theme metrics, so the window must work at
sizes nobody chose while developing. The suite starts, lists and navigates at
font 9, 12 and 18 with spacing 0.8, 1.0 and 1.3, and fails on any missing
token.

### 6.11.1 An overlay that fades is still "visible"

`visible: opacity > 0` with an animated opacity means a dialog stays visible
for the whole fade-out. `focusList()` refuses to move focus while an overlay
is up, so a prompt that announced its own closure immediately left the window
with no keyboard focus at all: after cancelling a rename, every subsequent
key went nowhere. A closing overlay announces itself only once it is really
gone.

## 6.12 A stalled mount must not stall Gust

Plan §3 rule 3, and the only one of the four that cannot be satisfied by
being careful in Rust.

**A cancellation token cannot interrupt a stuck syscall.** A `readdir` or a
`stat` on a FUSE mount whose server has stopped answering blocks in
uninterruptible sleep: the thread cannot be woken, `SIGTERM` is ignored, and
`timeout(1)` cannot end it either. This was measured rather than assumed — a
listing of an sshfs mount with a SIGSTOPped sshd took **280 seconds** to
return, and only returned when the server was resumed.

So Gust does not perform the call that might hang. A short-lived child does,
and Gust kills it if it does not answer within four seconds. Killing a
process is the only thing that reliably ends a stuck FUSE wait, and even that
only takes effect when the wait finally ends — what matters is that *Gust* is
not the one waiting.

### Where the probe sits

Three places touch a path before any worker does, and all three are bounded:

| Place | Why it must be bounded |
|---|---|
| `startup_location()` | Runs before the window exists. Unbounded, Gust never paints at all. |
| `navigate()` | Runs on the UI thread. Unbounded, the whole window freezes. |
| the listing worker | Unbounded, the folder never settles and the worker never returns. |

**Only paths that can actually stall are probed.** The filesystem type comes
from `/proc/self/mounts`, which the kernel answers from its own tables and
which therefore cannot itself hang. A path on ext4/btrfs/xfs/tmpfs spawns no
child and pays nothing.

### What the user sees

The window stays on the folder it could reach. Moving somebody somewhere else
because a remote hung would lose their place for them. The status says
`Not responding`, and the notice says which place stopped answering, how long
Gust waited, and that the rest of the application still works:

> `studio stopped responding after 4 seconds. Browsing is still available
> everywhere else; reconnect to try again.`

At startup the fallback is home rather than a window that never appears.

The child is `ls -A --` with the path as a separate argument: no shell, so a
directory named `; rm -rf ~` is a directory name and nothing else. A killed
child is deliberately **not** reaped — waiting on it would block for exactly
as long as the stall being escaped.

## 6.13 Devices: USB sticks, discs and phones

Plan §3's "USB and phones" row. Gust discovers volumes and offers Mount and
Eject as **visible actions**; it never mounts anything on its own.

### Discovery is a local read

The device list comes from `lsblk --json`, which reads the kernel's own view
and cannot hang on a network. Gust does not talk to UDisks to *enumerate* —
only to mount and unmount, which are the operations that need privilege and
a user's intent. Listing devices therefore works on a machine with no UDisks
daemon at all; the actions are simply unavailable, and say so.

### What counts as a device worth showing

A volume is offered when it is **removable or hotplug** and is not the
system's own storage. Concretely:

| Shown | Not shown |
|---|---|
| USB sticks and drives (`rm` or `hotplug`) | the root disk and its partitions |
| SD cards | swap, zram |
| Optical discs, with "no disc" stated | LVM/device-mapper internals |
| Phones exposing storage | loop devices (an image file is not a device) |

An empty optical drive is listed and described as empty rather than hidden:
"nothing in the drive" is a fact the user can act on; a missing row is not.

### Mount and eject are the user's decision

Gust never auto-mounts. A device is a row with a visible action, exactly as a
remote place is. Eject refuses while a transfer is running and says which
one — pulling a filesystem out from under a copy is how people lose data,
and a file manager that allows it has not done its job.

### What is deliberately not claimed

Phones over MTP need GVfs and per-device Trust prompts, and NAS/SMB needs a
GIO mount. Neither is implemented. The plan is explicit that discovery
"can be unavailable or incomplete" and that showing a device is not a claim
that it will work — so Gust lists what the kernel reports and does not invent
support it has not got.

**This has not been qualified against physical hardware.** The VM used for
qualification has no USB device: its only removable block device is an empty
virtual optical drive, which is exactly one of the cases above but not the
common one. The discovery logic is unit-tested against captured `lsblk`
output from real machines, and the empty-drive case is verified live. Mount
and eject against a physical stick remain **unproven**.

## 7. Undo journal

An undo entry records, per affected path: the expected identity
(`dev, ino, mtime, size`) **for every descendant it will touch**, plus the
restore location or preserved overwritten bytes.

Root inode+mtime is **not** a subtree fingerprint. If any descendant changed,
a new file appeared, or a remote backend cannot conditionally validate the
inverse → **preserve the work and ask**. Never recursively delete a current
tree to satisfy an undo.

Irreversible deletion never displays an Undo affordance.

## 8. Trash vs delete

Separate commands, separate wording, separate confirmations. Where trash is
unsupported the action stays unsupported; a permanent delete is offered only
as a distinctly labelled, confirmed action with Cancel focused by default.

## 9. Threading

The UI thread never touches the filesystem. Workers own cancellation tokens.
Model mutations are short, bounded, generation-tagged and queued onto the GUI
thread. Queueing after QObject destruction returns `Err` and is handled — no
`unwrap`, no panic. `is_destroyed()` followed by queue is **not** atomic and is
not used as a lifetime guarantee.

## 10. Theme tokens

Read `~/.local/state/omarchy/current/theme/colors.toml` and `shell.toml`
through Gust's own adapter (no imports from Omarchy's private shell tree).
Watch the *parent* of `current` so an atomic symlink replacement is observed.
Missing/malformed tokens fall back cleanly to a built-in palette. No colour,
font, radius or spacing literal in QML.

## 11. Sandbox

Untrusted bytes (thumbnail decoding, archive listing) run under
`bwrap` + `prlimit` with no network, a read-only bind of the single input file
and a private output directory — or the feature is unavailable. Parent
validates helper output through a held descriptor: regular file, size bound,
expected format, no symlink/FIFO.
