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
`app` runs the app Rust tests and requires Qt build tools. `model-seams` runs the
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
publishing the UI code. The runner owns `/tmp/gust-qa.lock`, closes the descriptor
for children, starts/kills only its own Gust PID and verifies focus before input.
Callers must not acquire the QA lease again.

```sh
GUST_DEDICATED_QA=1 python3 tests/release-gates.py --gate native --required \
  --installed-root /tmp/gust-package-fresh/root \
  --native-runner /path/to/local-ui-checkout/tests/ui-flea-journeys.sh \
  --output /tmp/gust-native-fresh
```

Select a synthetic Wayland/Hyprland session only after coordinating with its
owner. Every HOME/XDG base and fixture belongs inside the runner's output; the
Omarchy token source is read-only. The binary must match the current source build
receipt before input starts. Native runner exits 75 (busy) and 77 (unavailable)
are SKIP. Its `results.tsv` must account for journeys; mixed PASS/SKIP is still
SKIP even if the script exits zero. Raw paths, logs and images never reach public
CI summaries.

The `themes` gate additionally needs `--themes-dir`, ImageMagick and the same
installed root/UI runner. It enumerates the supplied theme set, copies each to
private state, invokes the input-free `launch` journey, checks the exact journey
receipt and requires that theme's background in the window-region capture.
Missing color files count as skips. A successful subset is not the complete
installed matrix. It never switches the global theme. The matrix wrapper does
not hold a lease while the UI runner acquires it.

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
native x86_64/aarch64 release builds with staging checks. The artifact jobs fetch and
verify the recipe's declared source archive, build that extracted source, then
stage it with the reviewed recipe; a checkout alias cannot satisfy source-pin
qualification. Library observation reports unprovisioned fixture skips explicitly.
Full `--required` fixtures,
KVM-only models, desktop/theme, benchmark and hardware/provider qualification
remain separately required before release; hosted build success cannot certify
them. There are no screenshot, log or receipt uploads and no self-hosted jobs
that would expose private runner identities.
