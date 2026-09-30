# Decisions

One line each: **decision** — why — how to revert. Newest last. Written by the
autonomous v1 build (goal contract of 30 Sep 2026); the owner may reverse any.

- **D1 · Fixture skips go through a `gust-testkit` crate (`skip!`/`skip_unless!`) that writes a ledger; `tests/run-tests.sh` reports them as SKIP and subtracts them from the pass count** — libtest has no runtime skip, so an early return printed `ok` and inflated the pass count — revert: drop `crates/testkit`, restore the `eprintln!("SKIP…"); return;` blocks, point `just test` back at `cargo test`.
- **D2 · `clipboard_live` runs only with `GUST_LIVE_CLIPBOARD=1` (or `GUST_CLIPBOARD_REQUIRED=1`)** — it replaced the real clipboard of whoever ran `cargo test` — revert: remove `live_clipboard_opted_in()` from `need_wayland!`.
- **D3 · `GUST_ALL_REQUIRED=1` turns every fixture skip into a failure; `just test-required` sets it** — the goal asks for one recipe that sets every `GUST_*_REQUIRED`; a single switch cannot miss a newly added gate — revert: delete the recipe and the `all_required()` check.
- **D4 · Lint fixed for the Rust 1.98 toolchain (`is_multiple_of`, `&Path`, one `#[allow(too_many_arguments)]` on the job worker) and the tree `cargo fmt`-ed** — `just lint` was red at 4c2ac56 on this toolchain; the fmt pass is whitespace only — revert: `git revert` the commit.
- **D5 · `tests/lint-qml.sh` strips strings and comments in ONE pass, and allows `indexOf`/`includes`/`valueOf` in bindings** — the two-pass version treated the `//` in `"file://"` as a comment and reported 4 phantom unclosed braces; the three methods read notifying properties so the binding does re-evaluate. A planted missing brace still fails the gate — revert: restore the previous regex block.
