# GUST

A Rust and Qt Quick file manager for Omarchy, with theme tokens adapted from
Omarchy and a fallback palette for other desktops. MIT licensed.

Gust is under active development. The source includes working application paths
for the features below; that is distinct from qualification of a particular
installed build, device or account. See [testing](docs/TESTING.md) for the gates
and their prerequisites.

## Current source scope

- Rows, Grid and Columns views; tabs, two panes, keyboard navigation,
  type-ahead, filtering and scoped filename/content search.
- Copy, move, duplicate, rename and bulk rename; conflict choices including
  directory merge; drag and drop and file clipboard integration.
- Trash/restore and identity-checked undo for supported operations. Permanent
  deletion is a separate confirmed action with no undo.
- Colour tags stored in extended attributes, permission/ownership controls,
  bookmarks, recent places and session preferences.
- Quick Look for text, Markdown, images, PDF pages, audio and video frames;
  sandboxed thumbnail helpers. Availability depends on installed tools/codecs.
- Explicit SSHFS, rclone and GVfs connections; Tailscale discovery; phone,
  camera and removable-volume actions; Open/Save portal integration.
- Archive listing/extraction and tar.zst creation. Archive containment and
  recovery are being strengthened; a displayed preview is not proof that
  extraction is complete or safe for every archive.

Phone fixtures are stand-ins. Physical phone browsing/transfers, physical USB
mount/eject and individual cloud accounts need their own qualification. A
successful mount does not establish upload, atomic rename or recovery support.
New backend contracts become user features only after app/UI consumption and
native acceptance.

## Build and run

Requires Rust 1.90 or newer, Qt 6 development packages, a C++ compiler, pkg-config
and libdbus development files. New native GIO/archive-helper integrations also
need GLib/GIO and libarchive development libraries. Qt model tests need Qt Test
and the relevant QML modules. The package recipe lists runtime helpers.

```sh
cargo build --locked -p gust-app
./target/debug/gust
```

The ops archive helper, when present in the chosen source revision, is a second
runtime executable. Build and stage release artifacts with the recorded build
workflow described in [release preparation](docs/release/README.md).

```sh
just test           # library/fixture results, including actual skips
just test-app       # Qt app Rust tests through the same accounting
just test-all       # workspace Rust tests through the same accounting
just check-release  # runner, containment and packaging contracts
just lint           # Rust and source lint checks (requires just/Qt)
```

`just test-required` rejects missing fixtures, ignored tests and empty test runs.
It does not provision servers, change credentials or enable the live clipboard.
Desktop/theme acceptance needs a separately selected synthetic session; see
[testing](docs/TESTING.md). Raw logs and screenshots stay local.

## Safety and qualification

Staged publication, no-clobber checks, target/subtree identity validation and
explicit confirmations protect specific operations. Tests exercise those
boundaries; they do not establish a general guarantee against data loss.
Thumbnail/audio helpers use bubblewrap and prlimit. Missing sandbox support must
remain an explicit unavailable capability. Filesystem work is being moved off
the UI thread; the design's threading rules are requirements, not a claim that
every current handler already satisfies them.

Hosted CI checks source contracts, app/model seams and native builds/staged
artifacts on x86_64 and aarch64. It does not establish an installed Arch package,
real desktop/theme matrix, benchmark result or hardware/provider pass. Historical
benchmark and desktop notes describe their recorded revisions only.

## Omarchy integration

Installing the prepared package does not select Gust as the default, mount
anything, pair a phone or enable services. `gust --default on` is an explicit
per-user action; `gust --default off` attempts the recorded rollback while
preserving later edits. Portal selection is separate and explicit.

The source recipe is prepared for review; this document does not claim a
published release or registry package. [Release preparation](docs/release/README.md)
describes immutable sources, checksums and installed-artifact checks.

## Layout

`crates/core`: listing/search/theme; `crates/ops`: operations/recovery;
`crates/remote`: connections; `crates/thumbs`: preview helpers;
`crates/app`: Qt bridge/models; `ui/`: QML;
[docs/DESIGN.md](docs/DESIGN.md): shared contracts; `tests/`: orchestration.
