# Browsing responsiveness: implementation and qualification

This report covers the retained preferences, directory-projection, scalar-sort
and directory-size-cache work. Production remains Qt Quick with an in-process
Rust core. The implementation has scoped acceptance; whole-application
performance, presented-frame latency and global resource acceptance remain open.

## Retained implementation

### Preferences and ordinary filtering

One process-owned service loads, reconciles and saves preferences on a worker.
Admission, pending changes, save acknowledgement and persistent failures are
separate states. A bounded close flush keeps the event loop responsive; failed
or unaccepted changes never become a false “Saved” acknowledgement. Cooperating
processes use nonblocking locks around reconciliation and publication. Existing
staged-file sync and rename guarantees remain; arbitrary editors and multiple
preference documents are not an atomic transaction.

Ordinary filtering prepares the query once and emits matching backing indices
in listing order. Ranked Find retains its separate ranking semantics. Selection
reconciliation borrows canonical raw names and preserves pinned/anchor behavior.

### Directory projections and publication

Bounded process-wide executors prepare immutable listing projections, metadata,
filter results and selection updates. Requests carry revision and displayed-
location identity. Newest-wins cancellation does not allow a stale completion
to replace current intent. CPU work, I/O admission and retirement remain bounded.
Displaced, rejected and partially constructed large storage retires off the GUI
thread; cancellation observation and destruction are separate measurements.

Row-derived activation, clipboard, drag and operation targets use the displayed
location and exact OS-byte names. Pending selection cannot silently operate on
an older set of targets. Live-update holds cover menus, drags and selection
bands. Qt structural notifications bracket their corresponding model changes.
Selection/mark-only publication reuses row maps rather than forcing a reset.

Metadata and row caches use lazy 64-cell pages and an exact borrowed-name index.
Thumbnail relist reuse requires fresh worker-side source-version validation.
Thumbnail caches retain two stable values per key; a third distinct value hides
stale display data and requests existing bounded refresh recovery. An active
search defers that refresh. No thumbnail payload reclamation is claimed.

### Scalar sort and size-cache storage

Admitted Size/Modified sorts prepare compact scalar/name-prefix/index records.
Every prefix collision falls back to the original exact comparator; stability,
directory grouping, direction and raw-byte behavior remain unchanged. Ordered
inputs retain their fast path. Prepared sorting may request 48 bytes per row,
versus 16 for the generic path: at 1M rows this adds 32 MB per worker, or 64 MB
across two CPU workers. This is a scoped tradeoff, not a global memory budget.

Directory-size reads return copied scalar values. Short locks cover only the
copy or replacement; no guard escapes into formatting, Qt or another cache.
Third and later scalar values replace directly. Clear remains logical,
constant-time disablement, and final destruction keeps the retirement owner.
Thumbnail storage and metadata ownership are unchanged by this specialization.

## Review correctness follow-up

Review found and corrected seven correctness/qualification gaps. Folder-size
admission now resets for same-backing row changes. Projection frames consume
only the focus revision they prepared, preserving newer relist focus. Split
scratch-tab tracking uses the intended initial destination. Deferred row menus
are cancelled by Settings and other modal overlays. Integration busy state
waits for its explicit worker refresh token instead of any preference notice.
Correctness-only search journeys default to 99,999 visible files plus their
hidden file and reject larger fixtures; ordinary 100k timing fixtures are
unchanged. The scalar cancellation helper explicitly rejects unsupported Kind.

The corrected source passed 143 required app tests, including real-Qt held-relist
and equal-viewport filter regressions; 234 offscreen QML checks with no warnings;
115 existing work/close/preference guard checks; and 22 runner/argument tests.
The held-relist regression initially used an impossible drain condition during
publication hold; its wait was corrected to the actual completion boundary.
The unsupported Kind command was exercised and exited 2 before fixture work.

Two new software-rendered native correctness journeys passed 20 explicit checks
with no QML warnings. A focused split/Settings smoke confirmed scratch-tab
cleanup after navigating away from startup and cached refresh acknowledgement.
The Copy/Undo journey preserved file identities/bytes; inspected screenshots
showed Target 1→3→1 KB, Untouched 512 B and source 2 KB. These are debug-artifact
correctness checks with private evidence, not renewed performance or platform
acceptance. Historical Stage 1–5 measurements and limitations below remain
historical evidence, and stopped timing experiments remain stopped.

### Split re-review correction

Re-review found that waiting for the first text-destination location signal
left tracking unarmed when that destination was already the model's startup
location. Text places now use the existing filesystem-free `token_for()` API
to establish their intended raw destination immediately. The first subsequent
user navigation releases scratch ownership even if initial loading emits no
location-change signal.

The updated QML suite passed 235 checks with no warnings. A new software-native
same-startup-Home journey passed 11 explicit checks with no QML warnings: dragging
Home into an untouched split still removes the scratch tab on close; repeating
the drag, navigating to an owned child and closing the split preserves both
Home and child tabs. All three screenshots were inspected privately. Rust
sources are unchanged by this follow-up; these are correctness checks only.

## Recorded evidence and its limits

Evidence used synthetic fixtures in private temporary homes and isolated native
Wayland sessions. Local receipts bind source, executable, runner and dependency
hashes. Raw logs, state documents, screenshots, machine paths and generated
artifacts are deliberately excluded from the repository and PR.

- Ordinary-filter CPU controls checked complete result vectors in every pair.
  At 1M entries, recorded old/new medians were 8.564/4.628 ms for `file`,
  18.326/9.204 ms for `txt`, 26.117/8.399 ms for `zzz`, and 29.467/8.615 ms
  for `19`. These exclude selection, Qt and presentation.
- Core differential checks cover complete stable permutations, both directions
  and directory policies, raw names, duplicate ties, collisions, cancellation
  and allocator cleanup. App tests cover revision validation, held publication,
  selection, stale completion, cache renewal, worker admission and retirement.
- Real-Qt integration checks cover structural signals and reentrant model reads.
  Offscreen QML seams use explicit model/theme fixtures; they do not substitute
  for native CXX, filesystem or rendered-pixel acceptance.
- The final retained size-cache stage recorded 142 app tests passing with one
  existing ignored manual trial, and seven existing clippy warnings. Its native
  ledger contains 73 attempts: 72 successful journeys and one preserved failed
  correctness-fixture attempt. Successful journeys contain 1,134 passing checks.
  A prior hash-collector preflight failure launched no native journey.
- The failed search fixture exceeded the unchanged 100,000-entry scan budget
  because it also contained a hidden entry. Only that correctness fixture was
  reduced to 99,999 visible entries; no timing/resource sample was retried.
- That retained stage includes 46 theme cases: 23 available color definitions
  under software and OpenGL. A background-only theme without a color definition
  was excluded, not counted as passed. Real Copy/Undo checks and screenshots
  confirmed folder totals of 1→3→1 KB with an unchanged 512 B sibling. Older
  stages' theme receipts remain older evidence.

### Memory benefit and adverse observations

At 1M rows, isolated second-version scalar storage decreased from 72.50 MB to
32.25 MB (55.5%). A combined dense two-version component fixture decreased from
375,124,456 to 334,874,456 requested bytes (40.25 MB / 10.7%). Same-backing scalar
renewal is not established as a production path. The reachable first-version
size-first fixture decreased from 169,124,456 to 160,874,456 bytes (8.25 MB /
4.88%), about eight bytes per touched scalar cell plus one page-index vector.
Requested component bytes are not allocator usable bytes, process PSS or GPU
memory. No matched retained-byte regression occurred in the 40 allocator cases.

The size-cache preparation CPU paired median at 100k was +0.418 ms; individual
pairs added +3.069 and +5.840 ms, the latter +22.5%. The 1M paired median was
−5.304 ms. Sparse/dense read additions were +3.467/+2.421 ns at 100k and
−0.226/+6.320 ns at 1M. Median budgets did not clear the adverse pairs.

Four native resource pairs per renderer retained software PSS/USS paired
medians of +1,472/+946 KiB and startup additions of +2.159/+5.167 ms for
external toplevel/settled-listing receipts. One software pair added about
21.94/21.78 ms. Vulkan paired PSS/USS were −4,669/−4,450 KiB; one pair added
0.52 seconds sampled cumulative CPU and three threads. These remain unattributed
observations, not equivalence. Resource fixtures had no populated size cache,
so their PSS cannot demonstrate the component saving. Finite snapshots establish
neither a peak nor an indefinite plateau; sampled CPU is not lifetime CPU.

## Diagnostic stop decisions

The QA-only publication recorder requires both nonempty `GUST_QA_STATE` and
`GUST_QA_PUBLICATION_PROBE=1`, read at model construction. Ordinary runs leave it
off. Its bounded records retain no names or paths. Correlated Rust `data()`
bodies occupied 0.069–1.064% of retained worst end events, but the remainder
cannot be assigned to Qt CPU or delegates: elapsed intervals include scheduling,
CXX conversion and observer work.

Probe overhead was not cleared. A software/100k paired tail median increased
40.947 ms / 58.751%, while marginal medians disagreed; a later settled-input
control also crossed its registered triggers. Pure recorder/JSON mechanism
controls did not bound native tails. Causal Qt/QML selection is therefore
**STOPPED**. The recorder remains opt-in investigation infrastructure; future
reuse must address its coupling to repeated QA serialization first. No frame
probe, profiler-enabled artifact or causal UI optimization is retained.

Later bounded Kind-rank experiments met their primary speedup targets but failed
predeclared control gates. A grouped-thumbnail-state experiment saved 7 MB in
fully populated component fixtures but failed sparse-read and preparation timing
gates. Both implementations were rejected and the retained runtime restored.
Their experimental code and unexecuted native scaffolding are not published.

Identical-binary A/A diagnostics then crossed the same finite timing gates.
A separately preregistered repeated-call characterization used eight baseline
processes, 64 measured windows and 16 warmups. Within-process spread was not
clearly lower than between-process spread in any CPU/wall comparison. The
predeclared outcome stopped further component-timing-gated optimization on this
shared host. These are limited observations, not a cause, noise ceiling,
false-positive rate, equivalence result or retroactive clearance of a candidate.

Recorded load and CPU pressure were substantial, and compiler-process ownership
was not recorded. An advisory build lease did not isolate the host. Early
A/A runner/analyzer hashes were bound retrospectively; the final characterization
bound plan, order, source, binary, runner and analyzer before execution. Samples
and thresholds were not changed or pooled. Further work needs an authorized
isolated environment or separately justified evidence; no host-isolation change
was performed.

## Acceptance boundary

The retained implementation stages received final scoped Sol/Opus reviews of
9/10. The later rejection/restoration and A/A diagnostic scope took two rounds:
Sol 9→9 and Opus 8.5→9 after clearer host and analyzer-chronology disclosures.
Those diagnostic scores accept reporting and stop decisions, not a product
optimization. No strategy or diagnostic review is an overall acceptance score.

Still open: callback tails including observations up to 1.95 s, useful-content
and presented-frame goals, Name/startup uncertainty, Kind/legacy/admission gaps,
scalar scratch and whole-process memory/GPU budgets, loaded retirement and
completion tails, thumbnail retained payloads, deferred incremental edits,
cold-reload cursor behavior, stale path-based search freshness, and broader
platform, installed-package, physical-device, IME and accessibility qualification.

## Reproduction entry points

- `cargo test --locked -p gust-core` and `cargo test --locked -p gust-app`.
  The required app runner explicitly filters and reports its named isolated
  manual scheduler trial. Other ignored or skipped tests still fail required
  mode; the trial is never counted as a passing unit test.
- `python3 tests/stage2-ui-regressions.py --help` and the existing preference,
  work-guard and model regression runners.
- `python3 tests/qa-optimization.py --help` for private native journeys. Supply
  fresh output directories and explicit render-node/driver inputs for GPU runs.
  The runner owns its QA lease; do not drive the user's desktop or terminate
  unrelated processes. Keep the publication probe off for ordinary checks.
- Core examples `bench_filter`, `bench_projection`, `bench_scalar_projection`,
  `bench_sort_projection_acceptance`, `bench_sort_projection_pairs` and
  `bench_selection_handoff` exercise synthetic CPU/ownership controls.
- `bench/qt-floor/` is a diagnostic Qt control, not the production frontend.

Input-to-QA receipts, client swaps, rendered screenshots and compositor
presentation are distinct endpoints. None of these controls establish physical
input-to-presented-frame latency or complete installed release acceptance.
