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

- **A plain drop moves on the same drive and copies onto another** (D60),
  as in every mainstream file manager: dragging between folders is how
  people move files. "Same drive" is the mount each path sits on, read
  from `/proc/self/mounts` when the drag starts; an unknown mount copies.
  Ctrl held while dropping copies; Shift moves. A right-button drag asks
  (Copy, Move or Link here). A move by drop is journaled like any other,
  so Ctrl+Z puts it back.
- Folders outside the view take drops too: sidebar places, bookmarks,
  mounted drives and path-bar crumbs. Gust's own drag becomes a platform
  drag when it leaves the pane; targets recognise it by its source and
  plan it with the same rules. A drop from another application is still
  always a copy.
- The drop target and the operation are **named before it runs**: "Copy 3
  items to backups". A drop that does not say where things went is a data
  loss report waiting to happen.
- A drop onto the folder the items already live in is a no-op, not a
  self-copy producing "file (copy)" clutter.
- A drop onto a **file** targets its containing folder, never the file: an
  accidental drop must not be read as "replace this".
- Dropping a folder into itself or into its own descendant is refused with
  a reason, because it would recurse forever.
- **A held item is visible.** While a drag is in flight the item's icon and
  name (or "N items") and what releasing would do ("Move 1 item to inbox")
  ride under the pointer, the target is ringed, and the
  status line names the operation. Without the preview a drag looked like
  nothing was happening and read as "drag does not work".
- Text dropped from another application is not a file operation. Only URIs
  Gust can resolve to real paths are accepted.
- **Two Gust windows work together** (D62). A drag from another Gust
  window is planned by the same rule as one inside a window: Move on the
  same drive, Copy onto another, Ctrl copies, Shift moves; the target names
  the operation and rings the folder while it is held. Gust recognises its
  own drags by a private MIME type built in Rust; drags from other
  applications still copy. Each network share is its own drive, so a drop
  between two servers copies.
- **The clipboard is shared through the system clipboard.** Paste reads it
  first; Gust's own record of a cut is used only while the system clipboard
  still holds that same cut, so a newer copy made in another window or
  another app wins over an older cut.

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

### 6.4.1 Connect: one list of every place

The sidebar's **Remote** heading carries a **Connect…** button (also `:`
→ "Connect to a computer or cloud…"). It opens one panel listing:

- **Tailscale devices**, from `tailscale status --json`. That talks to the
  local tailscaled over its socket and contacts no device. Each row shows
  the device's short name, its MagicDNS name (or Tailscale address when
  MagicDNS is off), its OS and whether it is online. This computer is not
  listed. A stopped or logged-out Tailscale says so in words ("run
  tailscale up"), never as an empty list.
- **Computers** from `~/.ssh/config` (§6.4), **cloud storage** from rclone
  (§6.8), and "Add cloud storage" / "Add a shared folder".

Opening the panel connects nothing. A row's **Connect** is the first
traffic to that place; a connected row offers **Open** and **Disconnect**.

A Tailscale device connects over ordinary SSH (sshfs) with the user's own
keys, as a login the user names (remembered per device in
`$XDG_STATE_HOME/gust/tailnet-users`, 0600, bounded, junk ignored). It is
mounted at `$XDG_RUNTIME_DIR/gust/tailnet/<name>`, apart from SSH aliases.
Device names and targets from tailscaled are checked before use: a name
that could act as an ssh option or leave the mount directory is dropped.

**First connection to a device whose SSH key is not on record shows the
key's fingerprints and asks.** Tailscale has authenticated the device, but
the SSH key it serves is a separate thing; it is recorded in
`~/.ssh/known_hosts` (appended, never rewritten) only when the user
chooses "Trust and connect", and exactly the keys that were on screen are
recorded. Cancel records nothing and connects nothing. Checking is never
turned off; a key that later changes is refused as for any SSH host.

Connected places of every kind appear under Remote in the sidebar: click
to browse, drop files on them (§6.1, a copy across drives), right-click to
open in a new tab or disconnect.
Grouping never hides an additional mounted-but-lost route: it remains a
separate sidebar recovery/disconnect target even after another route connects.

#### Computers and connection routes

The Connect panel lists a computer once, with its SSH-config and Tailscale
connections underneath. Both use SSH; Tailscale is a network route, not a
second file-transfer protocol. Discovery groups only matching explicit
endpoints or overlapping trusted SSH host keys, never matching display
names or DNS guesses. Unknown conditional, wildcard, proxy or host-key policy
settings keep an alias separate rather than guessing its effective endpoint.
Previously trusted host-key lookups are cached against the file's identity
and revision, outside the mount-probe budget. Shared host keys can also come
from cloned machines: a group is a presentation hint, not proof that accounts
or filesystems are interchangeable. Every address stays visible in its picker.
Grouping is presentation, not a grant of trust:
each route keeps its own account, folder, port, SSH configuration and
host-key checks. SSH aliases are still passed verbatim to OpenSSH.

A computer with several routes offers **Connections…**. Its route picker
shows the actual address and account, which route is connected or lost,
and whether Tailscale is unavailable. Selecting another route is an
explicit new connection; Gust never falls back silently outside a VPN,
works around an authentication/key failure, remounts a place underneath
open file handles, or replays an interrupted copy/move/delete. Two entries
pointing at the same Tailscale endpoint are not an independent fallback.

A lost connection with another known route offers **Choose connection**,
with Cancel as the default. The picker keeps the failed route visible and
explains that existing file operations are not retried. Discovery retains
the last local-daemon peer snapshot for the lifetime of the process when
Tailscale stops, marked unavailable, so disabling it does not erase the
connection choices. This is not a claim that a peer is reachable, nor a
persistent device inventory. No new device is contacted just by looking.

#### Connection feedback

Connection prerequisites and errors live in a prominent, wrapping notice inside
Connect, not only in the status line. Missing sshfs explains how to install it
and offers **Check again**; Gust never installs packages or runs privileged
commands automatically. Checking again refreshes discovery, never connects.
Each attempt retains its capability key so a newly discovered missing helper
can still expose Install support. A worker panic completes feedback with an
explicit unknown-outcome failure, never an app-wide permanent busy state;
it neither retries nor claims to have undone external effects.
An explicit attempt keeps the panel open with the named operation in progress,
blocks duplicate attempts, and retains failures until the next action. Only a
matching completion from the initiating model clears its progress. Successful
connections close the panel; closing it manually does not cancel an attempt or
allow its later failure to steal focus. SSH, Tailscale, cloud setup and network
shares use the same feedback surface without retaining credentials.

**SSHFS upload limitation:** if a mount does not support atomic no-replace
rename, Gust refuses publication rather than risk replacing another file.
Browsing and downloading can still work; the local source is retained after
this refusal. Connecting successfully does not imply upload support. Gust
never substitutes an overwrite-capable rename or replays the refused operation.

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

Tab activation refreshes the presented pane models after updating the active
tab, its owning pane, the pane's stored tab index and its remembered view.
An `activeTab` change handler runs synchronously and may still see the old
pane index. Its early refresh is not the final projection. Even activating
the same tab or switching between equal view modes must refresh that
projection before focus and action handling resume: the pointer's pane model
and the active model used by window actions must be the same object.
QA records the presented pane bindings separately from the desired selectors;
checking only `paneModel()` would hide a stale displayed model.

Opening a tab beside the active one inserts into the shared tab list. Every
per-pane stored index at or above that insertion point must shift with its
original tab before the final activation projection. An untouched pane keeps
the same model, location and remembered view; matching its displayed model
to an already-wrong selector is not proof of preservation. Regression coverage
uses actual Qt ListModel/Instantiator insertion and separately named native
Left/Right folders. This insertion rule does not certify delayed close target
identity, cancellation acknowledgement or job/model lifetime.

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
   Columns cursor clicks report that interaction even when the cursor stays
   on the same row; taking pane ownership does not change their selection
   policy. Parent and preview-column clicks claim their pane before navigation,
   including a parent-row click that deliberately navigates nowhere. Context
   and background-tab middle-clicks keep their separate existing routes.
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
6. **A drop outcome carries its operation-owning model.** An internal drag
   across panes executes on the source model, even though the destination
   pane receives the event. Both successful starts and conflict/refusal
   answers must use that exact model; asking the receiver can lose a pending
   question or accidentally expose another transfer's question.
7. **Modifiers survive leaving the view.** Capture Ctrl/Shift's chosen mode
   on the drag proxy before starting the native drag. Other panes, sidebar
   destinations and breadcrumbs must use that mode for both preview and
   execution, not reset it to the unmodified default. Across processes the
   existing private MIME value carries the same choice.

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

### 6.9.1 Previews of everyday files

Quick Look shows each everyday kind of file the way a person expects, and
never decodes an untrusted byte in Gust's own process.

- **Images, PDFs, video frames** are rendered by the sandboxed helpers
  (§5, D56). A PDF shows every page: Left / Right or Page Up / Page Down
  turn it, and the page count comes from `pdfinfo` in the same sandbox.
  Pages are cached under `thumbnails/gust-pages/`, keyed by URI, mtime,
  page and size; an edited PDF drops its old pages.
- **Markdown** is rendered (`gust_core::markdown`) to Qt StyledText:
  headings, paragraphs, lists, task lists, quotes, code, bold, italic.
  Every character of the file is escaped first, so its own HTML is shown as
  text. Images are never loaded (a remote image is a tracking pixel) and
  links are not clickable. 256 KiB and 400 lines at most.
- **Audio plays** (`gust_thumbs::audio`). `ffmpeg` decodes the file inside
  the sandbox with no network, no home and no audio socket, and writes raw
  PCM (s16le, 48 kHz, stereo) to a pipe. Gust paces it into the desktop's
  own PCM player (`pw-play`, else `paplay`), which only ever sees samples.
  Gust owns the clock: pause stops the relay, a seek restarts the decoder
  at the new time; both take effect within the ~0.25 s already queued.
  `p` / `k` play and pause, Left / Right skip five seconds, the bar seeks
  by click or drag. Title, artist and album come from the file's tags via
  sandboxed `ffprobe`, sanitised. Closing Quick Look kills both processes.
- **Video** shows a frame and says "Enter plays it in your video player".
  Enter in Quick Look closes the preview and opens the file with its
  default application, as Enter does in the list.
- A file named like audio that is not audio says "Cannot play: …" and
  shows no controls.

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

### 6.13.2 Encrypted drives

A LUKS drive is one sidebar row that says "locked". Clicking it asks for the
passphrase in the masked prompt; Gust hands it to `udisksctl unlock
--key-file /dev/stdin` on a pipe, wipes its copy, and mounts and opens what
was unlocked. The passphrase is never in argv, the environment, a file or a
log, and the prompt's field is cleared when it closes. A wrong passphrase
reopens the prompt with the reason; nothing else changes.

The unlocked volume (a `crypt` child in lsblk) is never a row of its own:
it is the same drive, and the row's state follows it (unlocked, connected).
Eject unmounts the volume and then locks the drive, so it is safe to unplug
and asks again next time. Unlocking needs polkit's active-session rule (the
default); without it the message says the account is not allowed.

### 6.13.1 Phones and cameras

An iPhone, iPad or Android phone is not a block device, so `lsblk` never
lists it. Gust reads what the kernel and udev already wrote — sysfs
attributes and the udev database entry for the USB device
(`/run/udev/data/c189:<minor>`) — and contacts nothing to find it. A watch
on `/dev/bus/usb/<bus>/` says when something was plugged in or pulled.

| Kind | Recognised by | Opened through |
|---|---|---|
| iPhone, iPad, iPod touch | Apple vendor id and `USBMUX_SUPPORTED=1` (or an iOS model name) | `gio mount afc://<udid>/`; app files at `afc://<udid>:3/` |
| Android in File transfer mode | `ID_MTP_DEVICE=1` | `gio mount mtp://<ID_SERIAL>/` |
| Camera, or a phone set to PTP | `ID_GPHOTO2` | `gio mount gphoto2://<ID_SERIAL>/` |

- The row is named by the device itself ("Alex's iPhone") from
  `gio mount -li`, which reads the volume monitors' state and does not talk
  to the phone; the USB model name is the fallback. The path bar and title
  use the same name, never the `afc:host=<udid>` folder.
- A serial goes into a URI only if it is plain hex (Apple) or letters,
  digits and `_ - .` (MTP); anything else is not listed.
- **Nothing is mounted until the user clicks.** Then the phone is reached
  only through GVfs, the layer Nautilus uses, so one program owns the USB
  device. GVfs asks "Try again / Cancel" on gio's stdin while an iPhone is
  untrusted or locked; Gust answers Try again every three seconds and says
  in the row what the person has to do ("Unlock your iPhone and tap
  Trust"). A second click stops waiting and answers Cancel. "Don't Trust"
  is told plainly, with how to be asked again.
- Opening is published only by an admitted worker after the requested device
  interface is validated. Busy refusals, unsupported app-files requests and
  worker-start failures leave the previous phone state intact; a worker panic
  becomes an explicit failed/unknown state rather than an orphaned Opening.
- The supported Arch package installs the SSH, cloud, SMB/NFS and USB-device
  helpers as dependencies, including `gvfs-afc`/`usbmuxd` for Apple devices,
  `gvfs-mtp` for Android and `gvfs-gphoto2` for cameras. These are part of
  the file manager experience, not optional terminal homework.
- Source builds and older installations show missing capabilities prominently
  in Connect. A phone with missing support opens this setup surface instead
  of attempting a mount. **Install support** names the packages first and
  invokes only an allowlisted capability through the system's polkit prompt;
  Gust never runs as root, collects an administrator password or builds a
  shell command from a device name/address. Installations are serialized
  across tabs and results are tied to their initiating model. Failure/cancel
  stays visible and Check again is discovery-only. Unsupported systems get
  explicit package-manager guidance instead of a nonworking install button.
- Installation never connects a device, repeats an interrupted operation or
  accepts Trust. On success capabilities are refreshed and the person chooses
  Connect/Open again. An already attached Apple device may need reconnecting
  so the newly installed udev rule starts usbmuxd. Unlock/Trust remains a
  decision on the device. Only iOS-exposed media and app file-sharing areas
  are accessible; this is not unrestricted access to every app or iCloud.
- Physical-device qualification must respect its authorization boundary:
  USB detection, pairing validation and a service handshake without directory
  listing do not establish photo browsing or file-transfer qualification.
- Eject leaves every folder of the phone the window is in, unmounts photos
  and app files, and is never forced; it refuses while a transfer runs.

**Proven with stand-ins, not with a phone.** `just qa` §37 replaces sysfs,
the udev database and `gio` with a fake tree and a script that speaks gio's
own question protocol; everything from detection to eject is Gust's real
code. Browsing and transfers on physical devices remain unqualified. Separately,
USB detection, pairing validation, a trusted session and an AFC service handshake
were verified on one physical iPhone without directory listings, file metadata
queries, file reads/writes or opening photos. That connection-only check does
not establish transfer support on every Apple model.

**Physical USB storage remains unqualified.** The VM used for
qualification has no USB device: its only removable block device is an empty
virtual optical drive, which is exactly one of the cases above but not the
common one. The discovery logic is unit-tested against captured `lsblk`
output from real machines, and the empty-drive case is verified live. Mount
and eject against a physical stick remain **unproven**.

## 6.14 Network shares (SMB/NFS through GIO)

Plan §3's "NAS/shared folder" row. Gust mounts a share through GVfs and then
browses it like any other folder, because that is exactly what it becomes:
GVfs exposes it under `$XDG_RUNTIME_DIR/gvfs` as `fuse.gvfsd-fuse`, which the
stall guard (§6.12) already treats as a filesystem that can stop answering.

### The address is asked for, never guessed

The plan is explicit that discovery "can be unavailable or incomplete", so
Gust asks for the server and share rather than pretending to find them. A
typed address is the primary path; any discovery that exists is a suggestion
on top of it, never a substitute.

### Credentials are handed to GIO, not parsed out of it

The rule that shapes this code: **do not scrape localized command prompts.**
`gio mount` prints "Enter user and password for share…" in the system
language, and a version of Gust that matched on that string would break for
every user who does not run in English.

GIO reads credentials from stdin in a **fixed order** — user, then domain,
then password — when it has no interactive terminal. That order is a
protocol, not a prompt, and it is language-independent. Gust writes the three
values in that order and reads nothing back except the exit status and the
resulting mount point.

The password is written to the child's stdin pipe and nowhere else. It never
appears in argv (where `ps` would show it), never in an environment variable,
never in a log line, and never in Gust's places file — which stores the
server and share only. Re-connecting asks again, or the user's keyring
answers; Gust does not become a second place their password lives.

### NFS asks for nothing

NFS authorises by **address**, not by account: an export lists the clients
allowed to mount it. So the flow skips the username and password steps
entirely rather than collecting credentials it will never send. Only an
explicit `nfs://` selects it; nothing is inferred from an address that might
be either.

GVfs names an NFS mount `nfs:host=H,prefix=<percent-encoded export path>` —
**measured from a live mount, not guessed**. An earlier version of this code
predicted `export=<share>` and a unit test enshrined that guess; the live
test is what caught it.

### What is deliberately not built

Anonymous/guest shares and LAN discovery via Avahi.

## 6.15 Archives

Mockup C offers "Compress to .tar.zst", and §4 asks for archive listing
under Quick Look. The extraction *policy* has existed in `gust-ops` since
early on with eleven tests behind it; what was missing was anything that
called it.

### Listing is not extracting

Reading what is inside an archive must never write anything. Gust lists with
`bsdtar -tv`, parses the result, and shows it — nothing is unpacked to find
out what is in there. The listing is bounded like every other untrusted
read: at most 2000 entries are parsed for display.

### Extraction runs the policy, then stages

Every entry is vetted before a byte is written (`archive::vet_entries`):

| Rejected | Why |
|---|---|
| `../` or absolute paths | writes outside the destination |
| FIFOs, devices, sockets | the plan's own fixture produced a FIFO |
| setuid/setgid modes | a privileged binary from an untrusted archive |
| links escaping the archive | same as path traversal, one step removed |
| more than 10k entries or 2 GiB | a zip bomb is a denial of service |

What survives is extracted into a staging directory **inside the
destination**, so committing it is a same-filesystem rename. A failure
removes the staging directory: a half-extracted tree that looks like success
is worse than an error.

**Rejections are shown, not silently dropped.** An archive that contained
something Gust refused says so, names the entry and the reason, and extracts
the rest. Silently skipping is how somebody later finds a file missing and
has no idea why.

### Compressing

`tar -c --zstd` over the selection, into the current folder, through the
same job machinery as a copy — so it is cancellable, it reports progress,
and Escape stops it. The archive is written to a staging name and renamed
into place only on success, exactly as a copy is: an interrupted compress
leaves no half-written `.tar.zst` looking finished.

Names are passed as arguments after `--`, never interpolated into a string,
so a file called `--checkpoint-action=exec=sh` is a file name and nothing
else.

### What is deliberately not built

Encrypted archives, multi-part archives, and writing formats other than
`.tar.zst`. Reading covers whatever the installed bsdtar reads; a format it
does not know fails with the reason rather than a guess.

## 6.16 Bulk rename

Mockup C gives it a key of its own (`R`). It is the most dangerous ordinary
operation in a file manager: one mistake renames forty files at once, and
unlike a copy there is nothing left to compare against.

So the whole design is: **decide everything, show it, then do it.**

### A pattern, not a script

The user types a pattern with placeholders:

| Token | Becomes |
|---|---|
| `{n}` | a counter, from 1, zero-padded to the width of the largest number |
| `{name}` | the original name without its extension |
| `{ext}` | the original extension, without the dot |

`holiday-{n}.{ext}` over three JPEGs gives `holiday-1.jpg`, `holiday-2.jpg`,
`holiday-3.jpg`. Anything that is not a token is a literal, including braces
that do not form one — a file genuinely called `{n}` is possible and is left
alone.

There is no regular expression and no shell. A pattern language small enough
to explain in one table is a pattern language nobody can be surprised by.

### The preview is the feature

Nothing is renamed until the user has seen the full list of
`old name → new name`, **including the failures**. The plan is built before
a single file is touched, and the plan reports:

- every rename that would collide with an existing file
- every rename that would collide with *another rename in the same batch*
- every name the filesystem would reject (empty, `/`, NUL, `.`, `..`)
- every file whose name would not change at all

If any entry is refused, **nothing is renamed**. A partial bulk rename is
the worst outcome: the user cannot tell which half happened, and running it
again does something different. All or nothing.

### Ordering is the user's, not the disk's

Files are numbered in the order the user is looking at them — the current
sort — not in whatever order the filesystem returned. Numbering by inode
order would produce a sequence that matches nothing on screen.

### Undo

Bulk rename goes through the same journal as everything else, so Ctrl+Z puts
every name back. The undo entry records each old and new path; it is one
entry, not forty, because one Ctrl+Z should undo one user action.

### Two-phase execution

`a.txt → b.txt` and `b.txt → a.txt` in one batch is a legitimate swap, and
renaming them in sequence destroys one of them. Every rename therefore goes
to a unique temporary name first, then to its final name. A crash between
phases leaves recoverable temporary names rather than a lost file.

## 6.17 Open with…

Mockup B puts `o open with…` next to `enter open`. Enter uses the default
handler; this is for the other times.

### The list comes from the desktop database, not from a command's output

`gio mime` prints "Default application for ..." in the system language, and
§3's rule about not scraping localized prompts applies just as much here.
The handler list is read from the freedesktop database directly:

1. `mimeinfo.cache` in each application directory maps a MIME type to the
   desktop entries that registered for it — a machine-readable index with
   no prose in it.
2. `mimeapps.list` carries the user's own additions and removals, which
   take precedence over the system cache.
3. Each candidate's `.desktop` file supplies the name to show and the
   command to run.

An entry with `NoDisplay=true` or `Hidden=true` is still a valid handler and
is still offered: those flags mean "do not show me in the applications menu",
not "never open a file with me". `imv` is `NoDisplay=true` on this system and
is exactly what a person wants for an image.

### Launching

The `Exec` line is parsed, not handed to a shell. Field codes are expanded
per the freedesktop spec — `%f`/`%F` become the path, `%u`/`%U` its file URI,
and `%i`/`%c`/`%k` are dropped — and every remaining word becomes one argv
entry. A file called `; rm -rf ~` is one argument and nothing else.

`Terminal=true` entries are launched through a terminal, because an editor
with no terminal simply vanishes.

### What it will not do

It will not run an arbitrary command the user types. That is a shell prompt
wearing a file manager's clothes, and Gust already refuses to execute a file
to find out what it is (§4). The list is applications the system says can
open this type, plus nothing.

## 6.18 Opening several files at once

§9's Interop list asks for "multi-file open", and it is not the same thing as
opening one file several times.

Select five photographs, press Enter, and the right outcome is **one image
viewer showing five images** — you can move between them. Five separate
viewers, each holding one photograph, is five windows to close and no way to
flip through them.

### One launch, many arguments

The handler's `Exec` line says which it can take:

| Field code | Meaning | Gust does |
|---|---|---|
| `%F` or `%U` | accepts a list | one launch, every path appended |
| `%f` or `%u` | one file per invocation | one launch **per file** |
| neither | unknown | one launch per file, conservatively |

The distinction is in the freedesktop spec and is exactly the information
needed. `imv %F` takes them all; `xournalpp-wrapper %f` does not, and
launching it once with five arguments would open the first and silently drop
the rest — which is worse than five windows, because the files look opened.

### The bounds

At most 50 files in one activation. Past that the question is no longer
"open these" but "did you mean to", and a hundred viewer windows is a
denial of service the user performed on themselves. The refusal says the
number and suggests opening fewer.

Everything in the selection must be openable: a folder among the selected
files makes the whole action a navigation question rather than an open, so
Enter on a mixed selection opens only the cursor row, as it always did.

Executables are refused exactly as they are for a single file (§4). Nothing
about selecting several files makes running one of them safer.

## 6.19 The clipboard other applications can see

§9's Interop list asks for "current clipboard offers", and §10 records that
native Qt/GTK clipboard exchange was qualified during the plan's own
prototyping. Gust's clipboard was internal: Ctrl+C then paste into another
file manager did nothing, and a copy made elsewhere could not be pasted into
Gust. That is not a clipboard, it is a variable.

### What goes on the clipboard

Three representations of the same copy, because different desktops read
different ones:

| Target | Content | Read by |
|---|---|---|
| `x-special/gnome-copied-files` | `copy\nfile:///a\nfile:///b` | Nautilus, Nemo, Thunar, most GTK |
| `text/uri-list` | `file:///a\r\nfile:///b` | Qt/KDE, browsers, drag targets |
| `text/plain` | the paths, one per line | terminals, editors, anything |

The first line of the GNOME format is the verb — `copy` or `cut` — which is
how a cut survives the trip to another application. A paste target that
reads only `text/uri-list` gets a copy, because there is nowhere in that
format to say otherwise, and silently moving somebody's files because the
format could not express "cut" would be the worst possible default.

### Reading what others put there

Ctrl+V looks for the same three, in that order. A file manager's cut is
honoured as a move; a browser's `text/uri-list` is a copy; plain text is
tried as paths and ignored if they do not exist. Nothing is executed and no
URI scheme other than `file://` is accepted — a `http://` URI on the
clipboard is not something to paste into a folder.

File Paste reads the desktop on a worker, never while blocking Qt's event
loop: Qt may itself own and serve a newer text offer. Each file-clipboard
read has a shared two-second subprocess budget and a 512 KiB payload cap.
Missing, empty, non-file or failed reads in a Wayland session grant no
authority to revive a remembered Copy or Cut. Internal-only Paste is allowed
only when there is no Wayland session; failed sharing in an active session
must not promise an internal fallback. A new Copy/Cut, clipboard clear,
navigation, cancellation or another admitted transfer fences out delayed
clipboard results. Completion is delivered to the originating model, not
whichever tab happens to be active, and normal Copy/Move/conflict/Undo still
uses the existing transfer funnel. File Paste and Paste-into-folder keep
their existing image-fallback distinction.

Within one process, clipboard replacement is a shared generation across
every tab/model, captured before starting a read and checked again before
admission. Copy, Cut, Copy path and clearing an offer invalidate older reads,
even in another model. This generation does not claim to version clipboard
writers in other processes. Every transfer launch, including Duplicate's
direct launch, retires its model's pending read; Escape still targets an
already running job rather than an older read.

A window presents one conflict owner at a time. A delayed completion from
another model waits its turn rather than replacing that owner's question or
answer/cancel route. The apply-to-all choice starts unchecked for each new
transfer. Advancing the queue occurs only after the previous dialog has
fully closed, and vanished/cancelled owners cannot be answered.

### Why a helper process

Qt's clipboard is owned by the GUI thread and a Wayland clipboard offer must
outlive the moment it is made. `wl-copy` already does exactly this, holds
the offer for as long as it is needed, and is what the rest of the desktop
uses. Gust shells out to it with the data on **stdin**, never in argv: a
file list can be long and a file name can contain anything.

Without a Wayland session, the internal clipboard works within Gust and the
status line names that limited scope. In a Wayland session, a sharing failure
is reported as a failure rather than retaining a cut that could outlive a
newer desktop offer.

## 6.20 Dragging a tab

The plan asks for two gestures: drag a tab to a left or right drop target to
split, and drag one outside the window to detach it into a normal window
Hyprland can tile.

### Splitting

Dragging a tab over the left or right third of the pane area shows a drop
target and, on release, moves that tab into that pane — opening the second
pane if there is only one. It sets `tabList`'s `pane` field for the DRAGGED
tab directly. It deliberately does not call `splitPane()`: that moves the
*active* tab, which is right for `:split` and wrong here — the drop already
names the tab to move, and going through `splitPane()` first sent two tabs to
the right and left the left pane empty.

Dropping a tab into the pane it already belongs to does nothing, and says
nothing, because nothing happened.

### Detaching

Dragging a tab outside the window opens **a new Gust process** at that tab's
location with everything the tab had — back history, selection, cursor,
hidden-files state and view — passed in a private one-use handoff file. The
tab here closes only when the new window reports it is on screen; if the new
process dies or does not appear within 20 s, the tab stays and the reason is
shown.

A new process, not a new `Window` in this one, for a reason the plan states
directly: the detached window must be "a normal window that Hyprland can
tile". A second Qt window inside one process shares that process's fate —
one crash takes both, one stuck mount freezes both — and Gust's whole
remote design is built on the opposite assumption (§6.12). Two processes
also means the window manager sees two independent clients, which is what
tiling expects.

**A tab with a running job does not detach.** The job belongs to the process
that started it; handing its location to a new process would leave the
transfer running in the old one with nothing on screen to represent it. The
drag is refused with the reason, exactly as closing such a tab is (§6.6).

The last tab does not detach either: detaching it would close the window it
came from, which is a window-close gesture wearing a tab-drag costume.

### What the gesture must not do

A drag that starts on a tab and ends somewhere meaningless — the header, the
sidebar, off the bottom — puts the tab back. A file manager that loses a tab
because a drag ended in the wrong place is a file manager people stop
dragging tabs in.

The threshold is the same as the file-list drag: a few pixels of movement
before anything is considered a drag at all, so a slightly imprecise click
is still a click.

## 6.21 File icons and emblems

Gust draws its own file icons in the theme's colours instead of loading an
icon theme (D51). The look was chosen from a design page of variants
("3a · line drawing"): it reads at a glance, and it looks like Omarchy's
Quattro surfaces rather than like Nautilus.

- **Folders** are a thin outline in the accent colour with a lightly tinted
  fill and a solid tab. Special folders (Home, Documents, Downloads, Music,
  Pictures, Videos, Desktop, Templates, Public, from `user-dirs.dirs`) carry a
  JetBrainsMono Nerd Font glyph inside, at Grid size only.
- **Files** are a page with a folded corner, outlined in their family's
  colour, with the extension printed on it (`RS`, `JSON`, `TAR`). A name with
  no usable extension (LICENSE, .bashrc) shows lines of text instead.
- **Families** (`icons::family_of`): code `ok`, data `cyan`, plain text
  `foreground`, documents `blue`, images and video `magenta`, audio
  `orange`, archives `warn`, programs `danger`, fonts `foreground`, anything
  else `dark_foreground`. A MIME type's family comes from shared-mime-info's
  own generic icon, its media type and a short fix list (`FAMILY_FIXES`), so
  new types sort themselves with no per-extension list to keep.
- **The label** (`icons::label_for`) is the last extension, lowercase ASCII
  letters and digits, five characters at most; `.tar.*` says `tar`,
  `.AppImage` says `app`.
- **Drawn with QtQuick.Shapes** (`ui/FileIcon.qml`): two paths and a label,
  no image files. Every colour is a token, so a theme switch restyles icons
  with the window and the 22-theme screenshot matrix covers them.
- **The UI thread never reads files.** `gust_core::icons::Looks` (the MIME
  database: `globs2` and `generic-icons` from every `mime` data dir, plus
  `user-dirs.dirs`) is built on a worker (`gust-icons`). `key_for(name, kind,
  meta, path)` is pure and runs in `data()`; `Looks::look(key)` is a lookup in
  memory. Before the worker finishes, files are labelled pages without a
  family colour; `icons_rev` then re-resolves the bindings.
- **What decides the look**: `d:<path>` folder, `f[x][z]:<name>` file (MIME by
  the longest glob suffix; executable with no type → program), `broken`,
  `special`. A link to a folder looks like a folder; the link is an emblem.
  Nothing is sniffed and nothing is executed.
- **Emblems** (`icons::emblems_for`), square badges on the icon's corner in
  Grid and bare glyphs after the name in Rows: `unreadable` / `readonly`
  from the mode bits against the process's uid and groups (no `access()` per
  row), `mount` for a mount point (`statx` `STATX_ATTR_MOUNT_ROOT` with
  `AT_NO_AUTOMOUNT`, so listing never triggers an autofs mount, and bind
  mounts count), `link` or `broken-link` for a symlink. Without a Nerd Font
  they fall back to plain characters.
- Thumbnails still replace the icon in Grid when a sandboxed one exists.
- QA reads the looks through the `icons` field of the state dump
  (`look` = `shape\tfamily\tlabel\tglyph`), qa §31.

## 6.22 Wheel and keyboard ownership

- **Wheel.** Qt Quick turns each wheel event into a small flick. A
  high-resolution wheel reports a notch as eight events of 15/120, and eight
  tiny flicks barely move the list (5 notches: 0 px in the qa probe). Every
  scrolling list uses `WheelScroll`, which moves a fixed distance per notch
  — GTK's `page_height ^ (2/3)`, about three rows — and passes a touchpad's
  pixel deltas straight through. Modified wheels are left alone.
- **Keyboard.** One item, `listFocus` in Main.qml, owns the file-list keys.
  Views set `keyNavigationEnabled: false` and nothing in a view calls
  `forceActiveFocus()` on itself; a click on a row or on empty space reports
  `selectionTouched`, and the window refocuses `listFocus`. The rubber band
  used to focus its view, which left the arrow keys dead after a click on
  empty space in Grid until something else moved focus.
- **Sort button.** The toolbar shows the active order ("Name ▲") and opens a
  menu: Name, Kind, Size, Date modified, then Ascending / Descending.
  `set_sort_to(field, ascending)` sets it exactly; the Rows headers keep
  `set_sort(field)`, which flips direction on the active column. Both are
  remembered per folder (D31).

## 6.23 Personal places

Home and its sidebar destinations come from the process's configured `HOME`,
not from the folder passed on the command line or a remote mount path. The
model publishes that fixed root as `home_path`; populating the sidebar reads
only this property and performs no filesystem work. Navigating, switching
panes or opening another tab must not redefine a person's Home.

The existing Documents/Downloads/Pictures/Videos/Music defaults remain children
of that root. Reading customized XDG user directories is separate work; do not
claim it is implemented. If HOME is absent, do not invent places from the
current folder. A nested HOME fixture and a real sidebar click are required
regressions, since a two-component `/tmp/fixture` masks the former path guess.

## 6.24 Merging folders

Pasting or dropping a folder onto a folder of the same name offers **Merge**
beside Skip, Replace and Keep both, but only when both are real directories
(never a symlink to one, which would merge into wherever it points).

Merge never overwrites anything, so it needs no second question:

- An entry missing at the destination is copied (or moved) in, whole.
- A folder present on both sides is entered and merged the same way.
- A file whose bytes are identical on both sides (a symlink: identical
  target) is left as it is: "already there".
- Anything else with a taken name lands beside it under a free
  "name (copy)" name, which is checked against the names this merge is
  about to create too, so two of its own items never collide.

The whole plan is made before a byte is written, and each placed item goes
through the ordinary copy/move engine (staging, claims, no-clobber
publication). A move removes source folders only once they are empty; an
"already there" file stays in the source, and the summary says so.

One Undo takes back the whole merge. Each placed item is a journal entry, so
Undo removes exactly what the merge created (after the usual identity
check) or moves each item back; the destination's own files are never
touched. For a move, the source folders it emptied and removed are listed
in `removed_empty_dirs` and recreated, shallowest first, before anything is
moved back.

## 6.25 Colour tags

Seven colours (Red, Orange, Yellow, Green, Blue, Purple, Gray), stored in the
`user.xdg.tags` extended attribute as a comma-separated list — the same place
Dolphin and Flea keep them, so a file tagged in one shows in the others. A
name Gust does not know (a Dolphin word tag) is kept, in its place, whenever
Gust writes the list back.

- Alt+1…7 toggles a colour on the selection (else the cursor row); over a
  mixed selection it turns the colour on for all, then off for all. Alt+0
  removes the colours. The row menu has Tag… with the same choices, and the
  palette has one line per colour.
- Reading is one `lgetxattr` per row in the metadata pass, on a worker,
  never following a link. Rows show small dots after the name; the grid
  shows them on the tile's corner. Colours come from the Omarchy theme.
- Setting is refused on a network folder (it would block the UI thread on a
  stalled server). Links cannot carry user attributes on Linux and say so.
- One Ctrl+Z puts every file's exact previous value back (absent included),
  only on the same object (dev + inode) whose tags were not changed since.
- A copy carries its tags (on the staged file, before it has its name),
  same disk or another; a destination that cannot hold them still gets the
  bytes and says nothing.

## 7. Undo journal

An undo entry records, per affected path: the expected identity
(`dev, ino, mtime, size`) **for every descendant it will touch**, plus the
restore location or preserved overwritten bytes.

Root inode+mtime is **not** a subtree fingerprint. If any descendant changed,
a new file appeared, or a remote backend cannot conditionally validate the
inverse → **preserve the work and ask**. Never recursively delete a current
tree to satisfy an undo.

Irreversible deletion never displays an Undo affordance.

A single F2 rename has a one-step inverse in its owning model. Record the
original and renamed byte paths, the renamed object's identity, and the
parent directory's device/inode. Undo verifies the held parent and target,
then restores the original name with an atomic no-replace rename and checks
the identity the kernel actually moved. A replacement in the stat/rename
window is refused; restoring its previous name also uses no-replace. If new
work occupies that name, retain both current names and explain the refusal.
NOREPLACE makes destination occupancy atomic, not source identity validation;
read-back and non-clobber recovery do not exclude every possible concurrent
writer after verification. No hidden staging or deletion is used. It never
replaces a newly occupied name, follows a symlink, or deletes folder contents.
A refusal keeps the offer available for retry; a no-op rename does not replace
the previous offer. Filesystems without safe no-replace renames refuse rather
than falling back to a check-then-overwrite. This in-memory offer is not a
claim of cross-window or restart-persistent Undo.

## 7.1 Concurrent publication and clipboard admission

The final publication of staged Copy bytes is an atomic no-replace operation
on the held destination directory. An earlier absence check is not consent
to overwrite an object that appears later. Explicit Replace authorizes the
specific existing object preserved by the transaction, not another writer's
subsequent replacement; after preservation, final publication is still
no-replace. Ask, Skip and Keep Both must not become unconditional rename at
commit. If the filesystem cannot provide a safe atomic primitive, refuse
with an actionable result instead of check-then-overwrite. Apply this same
boundary to symlink publication, same-filesystem Move and inverse MoveBack;
each refusal preserves both the source and the newly occupied destination.
Unsupported-filesystem behavior and source identity admission remain separate
contracts and must not be claimed proven by destination no-clobber alone.

Paste admission treats a readable current system clipboard as authoritative
for both exact OS-byte paths and Copy/Cut verb. Equal paths with a newer Copy
must not inherit an old internal Cut. A valid non-file system offer is distinct
from an unavailable clipboard and cannot silently revive a stale file Move.
Fallback, native owner lifetime and shared cut consumption are separately
qualified; tests retain ordinary Cut, same-path Copy, different-path Copy and
newer plain-text controls.

The launch contract consumes OS-byte argv before any display conversion. Local
file URIs are decoded through the existing byte-safe URI codec; plain OS paths
are not lossy-normalized. Invalid or unsupported URI input is refused or falls
back with a truthful result, never passed as a literal `file://` path. A
non-UTF-8 plain path must not panic before the GUI starts.

Context menus fit the actual client viewport at the supported minimum size.
Overflow scrolls and reveals the keyboard-focused action; no destructive action
can remain selected outside the visible menu without being brought into view.
Closing the menu restores its prior list focus without executing an action.
Mouse and keyboard reachability need real-window tests independently of QML
geometry assertions.

Folder totals are cached display metadata, never ownership or recovery
authority. Transfer completion and Undo invalidate totals for the visible
ancestors of their recorded destination paths; Move also invalidates its
source ancestors. A mutation of the open root invalidates its child totals.
Unrelated sibling caches remain valid, and a Copy does not invalidate its
unchanged source. Path matching preserves OS bytes and components, with no
filesystem I/O on the GUI thread. Re-measurement uses the existing local,
bounded, cancellable viewport worker; no new remote tree walk is admitted.

These are acceptance contracts for the reliability work, not declarations that
the previous qualified beta already meets them. Durable claim batching must
retain before-create provenance and exact identity-bound recovery; volatile
state is never a performance substitute.

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


### 6.4.2 Owned remote helper calls

Remote service calls run on workers. `helper::CallContext { cancel: &CancelToken,
timeout: Duration }` gives each call one overall budget. `RemoteError` carries a
typed code and an actionable message; cancellation never claims rollback or
authorizes replay. Existing connection APIs remain bounded compatibility adapters.
Owned subprocesses share eight admission slots, nonblocking capped stdin/stdout/
stderr, readiness-based waits and process-group cleanup. A single reaper retains
a slot until even a kernel-stuck child is reaped; no reader join can outlive the
call deadline. Only owned children are signalled and mounts are never lazily
detached on cancellation. SSH aliases remain verbatim during diagnostic probes.
Malformed or missing rclone upload counters are unknown and block disconnect;
backend receipts require a complete parsed response, never an empty failed parse.

Owned helpers remove inherited TLS keylog settings. Reaper-start failure retains
bounded ownership, and subsequent admissions also reap completed children.

Successful SSHFS daemon startup explicitly hands off the accepted filesystem and
its SSH transport; normal helper retirement must not signal those descendants.
Only a successful starter exit with drained pipes permits this handoff. Failed,
cancelled or timed-out startup retains bounded process-group cleanup. Ordinary
one-shot helpers still terminate leftover descendants even after a successful exit.
Host-key scans offer keys for trust only when every key has a fingerprint. Any
fingerprint helper failure, cancellation or shared-budget exhaustion rejects the
whole scan; no undisplayed key may remain in a successful trust offer.

### Owned block-device helpers

`remote::block::{mount,unlock,unmount,lock}` run only on a worker, using the
existing bounded, cancellable owned helper runtime and `helper::CallContext`.
They retain CORE's device argument builders and unlock parser. Mount returns a
byte-preserving `PathBuf`; unlock returns the cleartext device string. Neither
checks candidate metadata, creates a mount directory, probes a device, or performs
automatic compensation. APP retains kernel-discovery/selection checks and passes
one cancellation token plus the remaining overall budget across unlock and mount.

`BlockFailure { error: RemoteError, effect: BlockEffect }` distinguishes proven
`NotStarted`, post-launch `Unknown`, and `Accepted` when the helper reports success
but its location cannot be decoded safely. Cancellation or timeout after launch
does not roll back a request already accepted by the device service. Callers must
check current device state before retrying an uncertain operation. Raw helper
diagnostics are not returned or logged. Secret bytes are borrowed only for stdin;
the caller retains and wipes its buffer. Existing input/output and helper-admission
bounds apply. CLI output is read in the C locale; malformed replies never trigger
path guesses or filesystem checks. Tests use private executable stand-ins, never
the real device service, mounts or credentials.

`block::list_devices(&CallContext) -> Result<Vec<gust_core::devices::Device>, BlockFailure>`
is the worker-only bounded discovery adapter. It runs `lsblk` with CORE's existing
column selection, validates a complete UTF-8 JSON envelope, and delegates device
parsing/filtering to `devices::{parse_lsblk,offerable_devices}`. Helper errors,
invalid replies, cancellation and exhausted parsing budget are failures, never an
empty-success fallback. A valid empty inventory remains successful. Discovery
errors carry `NotStarted`: no device-changing request was made, even if the
read-only discovery helper ran. Its diagnostics remain static and sanitized.
APP passes the same token and remaining overall budget through discovery and
subsequent mount/unlock/eject operations; a failed refresh must prevent them.
