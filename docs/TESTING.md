# Testing Gust

A successful command is evidence only for its stated scope and selected source.
Release qualification requires no skipped or ignored cases in each required gate,
an installed artifact matching the recorded build, and native desktop/theme
acceptance. Historical source tests do not qualify a current installation.

## Source and Rust tests

```sh
python3 tests/test-release-runners.py
python3 tests/test-package-artifacts.py
python3 tests/test-source-archive.py
python3 tests/check-setup-packaging.py
python3 packaging/check-dependencies.py
./tests/run-tests.sh
./tests/run-tests.sh --app
./tests/run-tests.sh --workspace
./tests/run-tests.sh --required
```

The runner uses a fresh private directory under `target/test-runs/` for each
invocation, including concurrent invocations. `cargo.log` and `skips.tsv` are raw
private evidence; `summary.json` and stdout contain aggregate counts. Instrumented
runtime skips are subtracted from libtest's pass count. Required mode sets
`GUST_ALL_REQUIRED=1` and also rejects recorded skips, ignored tests, malformed
accounting and a command that exits zero without observed tests. A compilation
failure is a failure even if no test summary was printed. Filters still select
only their stated tests; filtered-out tests are not evidence of coverage.

Failure summaries additionally expose at most 16 Rust error codes and 16 crate
names already present in Cargo.lock, plus build-script/native-compiler/test-process
failure categories. Custom-build-command headers use the same locked-crate
allowlist; fixed Qt detection/tool, QML generation and cc-rs markers expose only
booleans. These are observed markers, not an inferred dependency diagnosis.
Hosted library observation explicitly sets `GUST_PUBLIC_TEST_IDS=1`. This adds
at most 64 failed integration-test identifiers, each matched exactly to a
top-level `#[test]` declaration in committed HEAD and an allowlisted Gust crate.
Only uniquely owned integration targets qualify. Dirty source, unknown targets,
nested/unit names and unavailable Git metadata cannot supply identifiers;
unreported failures remain counted as unclassified. Local runs keep identifiers
private by default. Identifier collection never changes the test result.
Diagnostic prose, paths, source excerpts and panic payloads
remain in the private log. Gate output preserves these bounded fields so a
hosted failure can be investigated without publishing raw evidence.

`--package-unit` explicitly selects `--lib` and filters the one manual disk
benchmark `recursive::tests::copy_1000_small_files`. Its exclusion is recorded in
the summary, rather than counted as a pass. Every other ignored test still fails
required mode. The benchmark needs a separately provisioned destination and its
own measurement/acceptance; enabling all ignored tests cannot qualify it.

The ledger can account only for tests using `gust_testkit::skip!`; crate owners
must instrument new fixture early returns. Individual fixture flags such as
`GUST_SSHFS_REQUIRED=1` and `GUST_PREVIEWS_REQUIRED=1` remain supported by their
owning crates. No fixture service or live clipboard is enabled implicitly.

Use minimal selectors and the shared `/tmp/gust-build.lock` for compilation in
this development environment. `tests/release-gates.py` acquires it nonblockingly
around Rust/model checks; `packaging/build-artifact.py` owns it for release builds.
Do not nest those commands inside another acquisition of the same lock. A busy
lease is SKIP/TEMPFAIL, not a test pass. Direct Cargo commands need an externally
coordinated build lease.
The owned command boundary retires surviving descendants before returning any
immediate-child status, including success, failure or direct child termination.
Synthetic builder/gate tests verify lease exclusion while those descendants are
live, retirement before status/receipt publication, and unrelated-process safety.

## Gate orchestration

```sh
python3 tests/release-gates.py --gate static --required --output /tmp/gust-static-fresh
python3 tests/release-gates.py --gate app --required --output /tmp/gust-app-fresh
python3 tests/release-gates.py --gate models --output /tmp/gust-models-fresh
```

Output must be fresh and outside the checkout. Every gate is named PASS, FAIL or
SKIP. `--required` makes every non-PASS fatal. Without it, skips remain explicit
and are never a release qualification. `--gate` can be repeated. With no selector,
all implemented gate scopes are requested, including unavailable native gates.
Benchmark and hardware/provider acceptance remain separate coordinated work;
this runner never turns their historical evidence into a current pass.

`library-unit` selects that package unit scope; `libraries` includes integration/doc tests.
`app` runs the app Rust tests and requires Qt >= 6.11.2 build tools. An explicit
`QMAKE` is authoritative; a missing/old selection cannot fall back to system Qt.
`model-seams` runs the
portable ConnectPanel and phone-worker source-extraction harnesses. `models` also
requests tab, pane and insertion harnesses; their existing KVM/UID-1000 admission
is preserved. These seams substitute external model I/O/CXX transport and do not
establish real native job, portal, focus or clipboard behavior. App-local tests
remain owned by the app lane.

## Contained fixtures and native acceptance

`tests/make-fixture.sh` accepts only a fresh/empty owned directory below `/tmp`.
It refuses existing data, traversal and symlink ancestry instead of deleting a
caller-selected tree. Regression tests verify that refused destinations and
symlink targets retain sentinel data. This guard is not a claim that every
legacy desktop/remote fixture script has the same containment.

The native gate consumes the UI-owned `ui-flea-journeys.sh` contract. Until that
local script is available, the gate is SKIP. It may be selected from an unpushed
UI integration checkout using `--native-runner`; that does not authorize
publishing the UI code. The runner owns `/tmp/gust-qa.lock` and closes the
descriptor for children. The current local UI contract creates a private
headless labwc compositor and uses its absolute Wayland socket, with synthetic
HOME/XDG roots. It stops only its own Gust/compositor PIDs; input cannot reach
the host desktop.
Callers must not acquire the QA lease again.

```sh
GUST_DEDICATED_QA=1 python3 tests/release-gates.py --gate native --required \
  --installed-root /tmp/gust-package-fresh/root \
  --native-runner /path/to/local-ui-checkout/tests/ui-flea-journeys.sh \
  --output /tmp/gust-native-fresh
```

The private lab needs labwc, grim, wtype, wl-paste, python3 and wlr-randr; the
UI-owned runner admits these prerequisites. It does not require a host Wayland
or Hyprland session. Pointer journeys additionally need `GUST_UI_VPTR`; missing
pointer support leaves those journeys SKIP. Every HOME/XDG base and fixture
belongs inside the runner's output; the
Omarchy token source is read-only. The binary must match the current source build
receipt before input starts. Native runner exits 75 (busy) and 77 (unavailable)
are SKIP. Its `results.tsv` must account for journeys; mixed PASS/SKIP is still
SKIP even if the script exits zero. The QML warning ledger must be a regular,
empty `qml-warnings.txt`; a missing ledger or collected runtime error fails
acceptance. Raw paths, logs and images never reach public
CI summaries.

The `themes` gate additionally needs `--themes-dir`, ImageMagick and the same
installed root/UI runner. It enumerates the supplied theme set, copies each to
private state, invokes the input-free `launch` journey, checks the exact journey
receipt and requires that theme's background in the window-region capture.
Missing color files count as skips. A successful subset is not the complete
installed matrix. It never switches the global theme. The matrix wrapper does
not hold a lease while the UI runner acquires it.
Labwc acceptance does not qualify Hyprland blur, transparency or window rules;
those require separate coordinated acceptance on the intended compositor.

Legacy `tests/qa.sh`, `tests/qa-one.sh`, `tests/theme-matrix.sh`, accessibility and
benchmark drivers have wider desktop/process/fixture side effects, including
some global process termination. Do not use them on a shared/human desktop or
as the native release gate. Their old evidence does not qualify a new artifact.
No native/theme or benchmark acceptance is run automatically on public hosted CI.

## Installed artifacts and dependencies

See [release preparation](release/README.md). `installed` requires
`--installed-root` and checks bytes, modes, ELF architecture, recorded build/source
identity and host dynamic-library resolution. `dependencies` additionally reads
the Arch package database and checks required helper executables. A non-Arch
host reports that dependency gate as SKIP. Presence checks do not prove service
state, codec functionality, user authorization, TLS/provider behavior or physical
device transfers.

`.github/workflows/checks.yml` runs required source/app/portable seam checks and
native x86_64/aarch64 release builds with staging checks. Portable seams run first
using Ubuntu's Qt 6.4.2. The app and artifact jobs then select public Qt 6.11.2
(`linux_gcc_64` / `linux_gcc_arm64`) using pinned aqtinstall 3.3.0 in private runner
storage, without an account or system Qt replacement. SDK installation uses
Python 3.14.0 on both native architectures: older Python can misclassify the
official ARM Qt tools 7z archive as ZIP, producing `BadZipFile`
([upstream report](https://github.com/miurahr/aqtinstall/issues/1042#issuecomment-5329253499)).
This selects only the hosted CI toolchain; the Arch package uses its distribution
Qt and does not run aqt. Installer failures retain
raw output/tracebacks privately and expose only an exit code, fixed exception
classes and prerequisite flags. SDK metadata dry runs do not prove archive
extraction or a usable installed SDK. The unquoted `public`
property in this source snapshot is rejected by Qt 6.4.2's QML parser; an isolated
quoted-key control compiles. The chosen qualification floor does not claim to
identify the earliest compatible Qt release. No UI source is changed by CI.
This snapshot also uses `Shape.preferredRendererType`, which the
[Qt documentation](https://doc.qt.io/qt-6/qml-qtquick-shapes-shape.html#preferredRendererType-prop)
dates to Qt 6.6; fixing the one grammar error would not establish Qt 6.4 support.
The artifact jobs fetch and
verify the recipe's declared source archive, build that extracted source, then
stage it with the reviewed recipe; a checkout alias cannot satisfy source-pin
qualification. Build receipts record Qt selection. Staging checks reject missing,
mixed or older Qt libraries and require resolution inside the selected SDK. This
proves only that SDK's staging environment; the Arch package gate separately
checks the declared versioned runtime dependencies.
Library observation reports unprovisioned fixture skips explicitly. Frozen
archive extraction tests require `libarchive-dev`/`pkg-config` to build the native
helper, `libarchive-tools` for bsdtar fixtures, and runtime libarchive,
`/usr/bin/bwrap`, `/usr/bin/prlimit` and the matching helper executable. Cargo's
integration-test build supplies the helper; CI selects that exact debug sibling.
`bubblewrap` and `util-linux` are explicit hosted prerequisites. A real
`--unshare-all` user/mount namespace probe is fatal on refusal or timeout; its raw
output stays private. Kernel/runner/AppArmor policy that denies it remains
unqualified. CI does not change host namespace policy or bypass isolation.
Artifact staging runs independently of library observation only after successful
SDK/app and sandbox admission, and never after cancellation. A failing library
or sandbox step keeps the job red even if a separate artifact check passes.
The frozen SMB availability test needs both successful `gio --help` and
`/usr/share/gvfs/mounts/smb.mount`. Hosted Ubuntu provisioning explicitly includes
`libglib2.0-bin` and `gvfs-backends` for those prerequisites; GLib development
headers alone are insufficient. This availability test does not start a fixture,
authenticate or mount a share, and its success cannot qualify a provider.
Full `--required` fixtures,
KVM-only models, desktop/theme, benchmark and hardware/provider qualification
remain separately required before release; hosted build success cannot certify
them. There are no screenshot, log or receipt uploads and no self-hosted jobs
that would expose private runner identities.
