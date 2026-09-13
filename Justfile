# GUST — a fast, calm file manager for Omarchy.

# Everything, the way CI runs it.
default: build test lint

build:
    cargo build --workspace

release:
    cargo build --workspace --release

# Library tests. These need no display and are the fast inner loop.
test:
    cargo test -p gust-core -p gust-ops -p gust-remote -p gust-thumbs

# Every test including the app crate (needs Qt).
test-all:
    cargo test --workspace

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

# Rebuild the throwaway fixture the QA suite runs against.
fixture:
    ./tests/make-fixture.sh /tmp/gust-fixture

clean:
    cargo clean
    rm -rf /tmp/gust-fixture /tmp/gust-qa*
