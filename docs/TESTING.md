# Testing Gust

Four gates. All four must be green before a change lands.

```
just test          # 1. library behaviour — no display needed
just lint          # 2. fmt, clippy -D warnings, QML token discipline
just qa            # 3. desktop interaction on a real compositor
just theme-matrix  # 4. every installed Omarchy theme renders
```

## Safety: the tests never touch your files

Every desktop test runs with:

- `HOME` pointed at a throwaway fixture (`/tmp/gust-fixture`)
- `XDG_DATA_HOME` pointed inside that fixture, so **Trash tests use a private
  trash** and can never reach the real one
- `XDG_CACHE_HOME` likewise
- `GUST_OMARCHY_STATE` pointed at the real theme directory **read-only**, so
  theming is genuinely exercised without a fixture HOME hiding it

`tests/qa.sh` asserts at the end that it ran entirely against the fixture.
The fixture builder (`tests/make-fixture.sh`) creates only files under the
directory it is given.

## How the desktop QA works

Screenshot-only testing proves very little, so the harness does two things:

1. **Real input.** `wtype` sends actual Wayland key events to the real window.
   No synthetic signals, no internal function calls.
2. **Exact state.** When `GUST_QA_STATE` is set, Gust writes a JSON snapshot of
   its own state (location, count, cursor, filter, view, dialog state, visible
   entry names) on every change. Assertions compare against that rather than
   guessing from pixels. Screenshots are captured too, as evidence and for a
   secondary OCR check.

The state probe refuses to write anywhere outside `/tmp`, and writes nothing
at all when the variable is unset.

### Writing a new check

```bash
expect "what it means"       "d['model']['count']" "11"
expect_contains "a substring" "d['model']['cursor_name']" "Projects"
wait_for "d['view']" "grid" 10      # poll instead of sleeping
```

Always `wait_for` a state change rather than sleeping a fixed time — the
harness types faster than any human and will otherwise race the UI.

## Regression cases worth keeping

These exist because they actually broke:

| Test | What it caught |
| --- | --- |
| `type_ahead_buffers_a_multi_character_prefix` | typing "Pro" searched for "o" and opened the wrong folder |
| `rejects_a_fifo_output_without_hanging` | a FIFO where a thumbnail should be hung the process forever |
| `fallback_defines_every_required_token` | a missing token painted the header magenta |
| `a_light_theme_never_gets_a_dark_bar_from_a_stale_shell_file` | light themes rendered a dark header |
| `refuses_symlinked_intermediate_directory` | a mid-path symlink swap defeating `O_NOFOLLOW` |
| QA §10c | undo overwriting work created after a delete |
| QA "keyboard focus returns" | Escape from an overlay left the app deaf to keys |

## Running Gust by hand against the fixture

```bash
./tests/make-fixture.sh /tmp/gust-fixture
HOME=/tmp/gust-fixture \
XDG_DATA_HOME=/tmp/gust-fixture/.local/share \
GUST_OMARCHY_STATE=$HOME/.local/state/omarchy \
  ./target/debug/gust
```

`tests/vmsession` resolves a headless/VM Wayland session's environment from the
running compositor when you need it.

## Never run two QA suites at once

`tests/qa.sh` drives the one real desktop: it kills every `gust` process,
types into whatever window has focus and rewrites shared fixtures. Two runs
overlapping destroy each other and produce failures that describe only the
collision.

The suite now takes an exclusive `flock` on `/tmp/gust-qa.lock` and exits 3
rather than starting a second run, keeps its state file inside its own output
directory, and kills the app it started on any exit path. If you see
`refusing to run two at once`, a previous run is still going — check with
`pgrep -af qa.sh`.

**When a QA report looks wrong, check its age before believing it.** Compare
the fixture's timestamp against `tests/qa.sh`; a run started before an edit
is executing a different suite than the one on disk, and bash reads scripts
incrementally, so editing the file mid-run corrupts the running copy.

### The lock must not leak into children

`flock` on fd 9 is inherited by everything the suite spawns. The app can
launch a viewer (`xdg-open`, an editor) that outlives the run, and those
grandchildren then hold the lock forever — every later run exits 3 with no
obvious cause. Every spawn closes it explicitly with `9>&-`.

To find a stuck lock: `fuser -v /tmp/gust-qa.lock`.

## Test the artifact, not just the source

`just qa` runs against `target/debug/gust`. The plan requires the release
artifact to be tested too, because that is what a user installs — different
optimisation, different codegen, and a debug-only difference would otherwise
ship unnoticed.

    just qa-release        # builds --release, then runs the full suite on it

The suite prints which binary it is exercising (`gust qa: testing <path>`)
so a green result can never be mistaken for coverage it does not have.
Override with `GUST_BIN=/path/to/gust ./tests/qa.sh <outdir>`.

## Optional dependencies

Gust must run with none of them. To reproduce a bare system, point `PATH`
at a directory containing only core utilities and start it there — no
`bwrap`, no `prlimit`, no image tools. Expected behaviour: it lists and
browses normally, Grid shows placeholder glyphs rather than broken images,
and the hint strip explains the missing capability instead of staying
silent. It must never fall back to decoding untrusted bytes in-process.

## Driving the window from Hyprland

This Omarchy build uses **Lua dispatch**, not the classic string form.
`hyprctl dispatch resizeactive exact 400 300` is a syntax error here; the
working calls are:

    hyprctl dispatch 'hl.dsp.window.float()'
    hyprctl dispatch 'hl.dsp.window.resize({ x = 520, y = 320 })'
    hyprctl dispatch 'hl.dsp.cursor.move({ x = 460, y = 400 })'

A tiled window ignores resize requests, so float it first.

## qmllint is not a syntax gate

`qmllint` gives up on a file whose imports it cannot resolve (`com.gust.app`
is registered by the Rust build, not visible to the standalone tool) and
then exits 0 on **syntactically broken QML**. A mismatched brace once passed
a full `QML lint: PASS`. `tests/lint-qml.sh` now checks brace balance
directly, which needs no imports and cannot be fooled by them.

## Pointer injection

The plan's §10 lists "restore an authorised interaction runner for the
missing end-to-end input checks" as an open item. `wtype` covers the
keyboard; nothing on this system covered the pointer, so every mouse control
was only ever verified by looking at a screenshot.

`tests/uinput-mouse.py` creates a temporary virtual mouse through
`/dev/uinput`, uses it, and removes it on exit:

    sudo python3 tests/uinput-mouse.py click 1212 59
    sudo python3 tests/uinput-mouse.py doubleclick 600 121
    sudo python3 tests/uinput-mouse.py drag 100 200 400 500
    sudo python3 tests/uinput-mouse.py wheel 460 400 -3

It uses **absolute** positioning, not relative motion: a relative mouse
depends on where the pointer already was, which makes a test that passes
once fail the next time. Verified accurate to 1px against `hyprctl
cursorpos`.

It needs write access to `/dev/uinput` (root on most systems). The QA suite
probes for it once and **reports the mouse cases as failures when injection
is unavailable**, rather than skipping silently — an untested control that
looks tested is worse than an obvious gap.

## Testing remote places without touching anybody's real machines

`tests/remote-fixture.sh start` builds a private SSH server: its own host key,
its own client key, its own `authorized_keys`, listening on 127.0.0.1:2222.
It never reads or writes the user's `~/.ssh`.

Two lessons paid for by getting this wrong:

- **Pin the session inside the fixture.** Without `ForceCommand internal-sftp
  -d <share>`, a connection lands in the login directory — which is the real
  home directory of whoever runs the test. The first live run mounted and
  listed personal folders. `ChrootDirectory` is the stronger form but needs a
  root-owned tree and a privileged sshd; an unprivileged fixture reports
  "server lacks privileges to chroot" and falls back to the whole home.
- **`Match` must be the last block in sshd_config.** Every directive after it
  belongs to that block, so a trailing `StrictModes no` fails to parse.

Live tests skip (rather than fail) when the fixture is not running, and the
QA suite reports the skip so a silent gap is visible.
