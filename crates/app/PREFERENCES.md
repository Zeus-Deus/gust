# Stage 1 preference bridge handoff

`src/preferences.rs` owns the process-wide Settings, SortMemory, ViewMemory and
session snapshot. Disk access, external reconciliation, staging/sync/rename and
integration-status probes run on one worker. GUI setters admit field/location
intents against the shared cache. An empty setter result means **accepted**,
never a disk acknowledgement. No operation journal uses this service.

## Exact QML API

Every `DirModel` exposes these properties (the names have no camel-case override):

- `preferences_ready: bool`
- `preferences_pending: bool`
- `preferences_error: string`
- `preferences_revision: string` — last admitted global revision
- `preferences_saved_revision: string` — acknowledged global revision

Invokables:

- `preferences_subscribe()` — register this model once after construction and
  publish initial cached state. First listing also registers automatically.
- `preferences_refresh()` — register if needed and request a worker refresh of
  external files and integration status. Use when opening Settings or regaining
  relevant focus. Idle external changes are observed on refresh or the next
  mutation/flush; there is no polling timer or file-watcher promise.
- `preferences_flush() -> string` — return the admitted revision target and
  request a worker flush/retry. Never waits for disk I/O.
- `preferences_is_saved(target: string) -> bool` — exact Rust comparison against
  the saved revision; avoids JS number precision loss and skipped-ack races.

Signal: `preferences_changed()` fires after publishing all preference properties.
Read cached `settings_json()`, `view_for_here()` and `session_json()` in that
handler. The queued notification reads current owner state and coalesces to at
most one outstanding callback per model. Receiver destruction unregisters it;
failed queue delivery never panics or discards a durable acknowledgement.

`settings_json()` retains every existing field and adds:
`ready`, `pending`, `error`, `admission_error`, `revision` (string),
`saved_revision` (string), `completed_attempt` (string), and `saved` (ready,
no pending intent and no error). `completed_attempt` advances after each worker
load/save attempt, including a lock refusal or a repeated failure with identical
error text. A close guard can capture it before flush and distinguish completion
of that retry from an old error; compare it as decimal u64, like revisions.

Renderer selection occurs before Qt. Recognized `GUST_RENDERER` values or nonempty
Qt backend overrides skip the settings wait entirely. Otherwise the worker has
at most 50 ms to read the saved renderer; timeout fixes this process's choice at
Auto. The worker keeps loading, but late readiness never changes the created
renderer, writes a default setting, or navigates a model. Other documents do not
hold up this wait.

`settings_set(key, value) -> string` still returns validation/admission errors;
empty means accepted. `remember_view(view)` and `set_sort_to(field, ascending)`
admit per-location intents. Async failures arrive through the properties/signal.
`session_save(lines, active) -> string` now admits a coalesced worker intent.
It returns admission errors (and publishes them through `preferences_error`);
empty means admitted or intentionally skipped because restore is disabled or
this is an admin/chooser window. It no longer starts an independent session writer. Admin/chooser session saves and remembered view/sort writes are
suppressed. Explicit default/portal/keybinding actions retain their existing
policy and trigger a worker status refresh after success.

## QML integration contract

1. Subscribe on model creation; wait for `preferences_ready` before applying
   startup settings and restoring sessions. Restore once per window, not on every
   acknowledgement. Keep existing explicit-path/admin/chooser restrictions.
2. Refresh controls and the active location's cached view on
   `preferences_changed`. Stage 1 notifications **never sort another model**:
   menu/confirmation/drag indices must stay stable, and unrelated preference
   acknowledgements must not trigger listing-sized work. Explicit sort controls
   still sort and attempt persistence. Normally explicit navigation/reload reads
   the latest shared remembered sort. If persistence is suppressed (admin/chooser)
   or admission is rejected (including initial loading), the requested sort is
   retained as one pane-local override keyed to the current `LocationKey`.
   Metadata/unsettled reloads use that override, so runtime sorting does not
   depend on persistence success. It also wins over shared-memory changes during
   same-location reloads. A later admitted explicit sort clears it; leaving the
   location retires it, so another location and Back use their current remembered
   sorts. No rejected choice is silently persisted when readiness arrives.
   Safe deferred sort projection and hold contracts belong to Stage 2.
3. Show accepted/pending versus saved, and surface `preferences_error`. A save
   failure retains admitted intents and can retry with `preferences_flush()`.
   An admission failure has no new revision or retained mutation: it persists
   in `admission_error`, and is also appended to `error`, across worker updates,
   unrelated successes, refreshes and tab switches. The user must explicitly
   reselect that same field/location (or retry a failed subscription) to clear
   it; flush alone cannot save a rejected choice. An acknowledgement covers only
   admitted revisions. Do not display Saved, or close as saved, while any error
   remains. Refresh integration status asynchronously after explicit actions.
4. On close, admit the final session first and handle any nonempty
   `session_save()` result before proceeding. Call `preferences_flush()`, keep the
   event loop alive, and recheck `preferences_is_saved(target)` on notifications.
   Enforce a UI deadline and offer retry or an explicit unsaved-close choice on
   timeout/failure. Recheck the current revision before final close if further
   mutations are still permitted during the guard. Main has only a 500 ms
   last-chance exit drain after the event loop; that is not the interactive guard.

## Guarantees and boundaries

There are at most 2,048 distinct pending field/location keys and 256 subscribers.
Repeated compatible mutations replace pending values, with revision ordering
retained for eviction/replay. Saturation returns an admission error; repeated
flushes only wake the same worker. Saved revisions live in the owner and survive
lost Qt notifications. Save failures retain optimistic values and unsaved intents.
The separate rejection ledger is bounded to 2,048 keys. If that exceptional bound
is exceeded, a conservative error latch requires restarting and reselecting the
unsaved choices; it never silently declares untracked rejected choices saved.

For batches with admitted intents, the worker first takes nonblocking exclusive
advisory locks in each destination directory (also Settings for Session's restore
policy). `.gust-preferences.lock` is owner-only 0600, regular, single-link and
owned by the effective UID; its held parent must be owned and not writable by
group/other. Parent traversal and lock opening refuse symlinks. Duplicate parent
inodes share one lock. Parents and lock descriptors stay held throughout the
batch's read/reconcile/publication; publication uses those held parents even if
pathnames are swapped. Locks are closed after publication, never unlinked.
Contention refuses the batch, retains every intent and exposes an error until an
explicit flush/admission/refresh retries. There is no timed waiter or retry loop.
Initial loads and read-only refreshes never create directories or lock files.

Migration caveat: existing Gust config/state directories that are owned by the
user but writable by group or other remain readable, but worker writes refuse
with the unsafe lock-parent error. Gust does not automatically chmod them. The
user must review and secure or move that directory before explicitly retrying;
accepted intents and the visible failure remain pending meanwhile.

The worker retains each document's last successfully read source bytes (and
successful publications). A read error keeps that document's last-good choices
and accepted optimistic intents; a failed Session read also retains the last
restored public session instead of reviving previously pruned paths. Successfully
read other documents can still refresh cached choices. Any failed read prevents
all publication and acknowledgement for that batch. Recovery rereads current
sources before replay, so unrelated external changes survive. A successful
missing-file read is an intentional deletion and can replace the cache with
defaults; it is distinct from an error.

Before a write, the worker rereads the document and replays only admitted
fields/locations, preserving unrelated recognized choices and unknown fields.
Session replacement strips only the canonical Gust header comments derived from
`Session::to_text()`, replaces the logical active/tab fields, and emits the
canonical header/active/tab block first, followed by preserved unknown content
in its original relative order. Saving cannot move valid records past the core
parser's bounded prefix. Repeated identical saves are byte-idempotent. Known header
copies from older writers are also removed before the core bounded-line parser,
so already-bloated files within 64 KiB can recover their tabs and active index.
Unknown comments/fields remain, and normalization never bypasses the raw 64 KiB
read limit, symlink checks or the core tab/line bounds.
The same canonical-prefix rule applies to Sorts and Views: at most 500 retained
recognized records precede preserved unknown content. All old recognized records
are removed before emitting that canonical cache, so evicted or out-of-prefix
records cannot reappear behind it. Recognized means exactly what each unchanged
core parser accepts (both LocationKey token and known value); invalid/future
syntax remains preserved, while an explicit touched key overwrites any old value.
These preference tokens do not grant filesystem authority. Canonical own headers
are derived from each writer and emitted once. Unknown content keeps its relative
order. Settings has no line-count bound and retains its existing field-patch
serialization and raw 64 KiB bound.

After replaying the entire admitted batch, reconciliation directly decodes every
touched serialized document with its unchanged core parser. Settings/Session use
logical equality; Sorts/Views compare canonical known-state text, without treating
cache recency or dirty flags as persisted choices. A mismatch refuses the batch
before any publication or acknowledgement; optimistic intents and visible errors
remain available for explicit retry. This does not relax size, line, tab or cache
bounds, and Session's direct check does not rely on service header normalization.

Known per-location caches retain the core 500-entry bounds. Unsafe/oversized
files and changes detected between reconciliation and publication report errors
or conflicts. Writes use a private create-new 0600 stage, staged-file sync and
rename through held, symlink-refusing parent descriptors on Linux. No parent
folder fsync is claimed. Arbitrary external editors do not participate in a
cross-process lock; the final compare-to-rename check remains, but a change in
that window cannot be made into an atomic compare-and-swap with this file format.
The locks serialize cooperating Gust processes, not arbitrary editors, and do not
turn a multi-document batch into an all-or-nothing transaction.

Core typed load/save APIs remain unchanged. Preference maps are still bounded
cloned snapshots; asynchronous sort and broader memory redesign are out of scope.
Explicit default/keybinding/portal actions still run synchronous subprocesses in
their existing invokables. Moving them to workers needs a completion/status API
and matching UI integration; this lane does not change their authorized policy.

Focused unit/backend and offscreen QML evidence do not establish native desktop
acceptance. Parent owns native GPU/desktop benchmarks, screenshots and council.
