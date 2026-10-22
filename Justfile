# GUST — a fast, calm file manager for Omarchy.

# Everything, the way CI runs it.
default: build test lint

build:
    cargo build --workspace

release:
    cargo build --workspace --release

# Library tests. These need no display and are the fast inner loop.
# Fixture tests that cannot run are reported as SKIP, never counted as passed.
test:
    ./tests/run-tests.sh

# Release gate: every fixture must be present. A skip is a failure here.
# Start the fixtures first (tests/remote-fixture.sh, smb-, nfs-, rclone-,
# docker-peers.sh) and run inside the desktop session for the clipboard.
test-required:
    GUST_ALL_REQUIRED=1 GUST_LIVE_CLIPBOARD=1 ./tests/run-tests.sh

# Every test including the app crate (needs Qt).
test-all:
    cargo test --workspace

# VM-only Qt model/signal regressions. Use a fresh owned evidence directory:
# GUST_MODEL_QA_OUT="$PWD/target/model-qa-run" just qa-model-routing
# These replace external model I/O, not native CXX/visual/clipboard acceptance.
qa-model-routing:
    python3 crates/app/tests/tab_model_regressions.py --source . --output "${GUST_MODEL_QA_OUT:?Set a fresh evidence directory}/tabs"
    python3 crates/app/tests/pane_focus_regressions.py --source . --output "${GUST_MODEL_QA_OUT:?Set a fresh evidence directory}/panes"

# VM-only exact methods with actual Qt ListModel/Instantiator insertion.
# Directory I/O, CXX transport and visual/focus hooks are still seams.
qa-tab-insertion:
    python3 crates/app/tests/tab_model_regressions.py --insertion --source . --output "${GUST_MODEL_QA_OUT:?Set a fresh evidence directory}/insertion"

# Actual ConnectPanel route/keyboard tests; model I/O and theme are fixtures.
qa-connections:
    python3 crates/app/tests/connection_regressions.py --source . --output "${GUST_CONNECTION_QA_OUT:?Set a fresh evidence directory}"
    python3 crates/app/tests/phone_state_regressions.py --source . --output "${GUST_CONNECTION_QA_OUT}/workers"

lint:
    cargo fmt --all -- --check
    # Rust lints, denied. Scoped to Gust's own crates: gust-app compiles
    # generated C++ against Qt headers, whose warnings are not ours to fix
    # and cannot be denied without failing on someone else's code.
    cargo clippy -p gust-core -p gust-ops -p gust-remote -p gust-thumbs \
        --all-targets -- -D warnings
    # gust-app still has to compile cleanly, just without -D on Qt's headers.
    cargo clippy -p gust-app
    ./tests/lint-qml.sh
    ./tests/lint-recipes.sh
    ./tests/lint-strings.sh
    python3 tests/check-setup-packaging.py

# Accessibility and scaling (plan §7): runs the window at three font sizes
# and reads the live AT-SPI tree a screen reader would read.
a11y: release
    GUST_BIN=target/release/gust ./tests/a11y.sh

# Desktop QA: drives the REAL window with REAL key events and asserts on the
# app's own state. Requires a Wayland session (see tests/vmsession).
qa: build
    ./tests/qa.sh

# Desktop QA against the RELEASE artifact, which is what users install.
# The plan requires testing the artifact, not just development source.
qa-release: release
    GUST_BIN="$PWD/target/release/gust" ./tests/qa.sh /tmp/gust-qa-release

# Screenshot every installed Omarchy theme.
theme-matrix: build
    ./tests/theme-matrix.sh

bench: release
    ./tests/bench.sh

# Flea's field bench with Gust added: 100k + media fixtures, 3 cold runs each.
bench-field: release
    ./bench/field-bench.sh

# Rebuild the throwaway fixture the QA suite runs against.
fixture:
    ./tests/make-fixture.sh /tmp/gust-fixture

clean:
    cargo clean
    rm -rf /tmp/gust-fixture /tmp/gust-qa*
