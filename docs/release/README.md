# Gust

A fast, calm file manager for Omarchy. One Rust binary, a Qt Quick window that
takes its colours, fonts and borders from your Omarchy theme, and a file engine
that never loses data.

> Draft for v0.1.0. Not published.

<p align="center">
  <img src="docs/screens/readme/gust-dark-grid.png" alt="Gust in Grid view, dark theme: drawn folders and labelled file pages" width="49%">
  <img src="docs/screens/readme/gust-light-grid.png" alt="Gust in Grid view, light theme" width="49%">
</p>
<p align="center">
  <img src="docs/screens/readme/gust-dark-rows.png" alt="Gust in Rows view: name, kind, size, modified, with read-only, no-access, mount and link badges" width="80%">
</p>

## Why Gust

- **Fast where it shows.** A 100,000-file folder settles in about 0.4 s on a
  plain VM, about half the time of the next fastest file manager in Flea's own
  field bench. Details are read only for the rows on screen, and folder sizes
  only for the folders you can see. Full table: `bench/RESULTS.md`.
- **Looks like Omarchy.** Every colour, font and spacing value comes from the
  current theme, and switching themes recolours the open window. All 22 bundled
  themes pass a contrast test (4.5:1 for primary text, 3:1 for secondary).
- **Never loses data.** Copies are staged and renamed into place atomically.
  Every destructive action asks first, and Ctrl+Z undoes it. Undo checks that
  the file is still the one it changed (device, inode, mtime) and refuses if
  it is not.
- **Keyboard first, mouse friendly.** Type-ahead, `/` filter, `:` command
  palette, Ctrl+L folder jump, four key presets (Standard, Vim, Mac, Windows),
  and `?` for the full key map. Right-click, drag and drop, rubber-band
  selection and the mouse side buttons all work.

## What it does

- Rows, Grid and Columns views, tabs, and a split view (F3).
- Copy, move, rename, duplicate, trash and restore, with conflict prompts
  (replace / keep both / skip).
- Pause and resume long transfers from the transfer card.
- Right-button drag offers Copy here, Move here or Link here.
- Paste an image from the clipboard as a file.
- Make links. Change permissions and owner/group, for one item or many.
- Quick Look (Space) for text and images, with SHA-256 on request.
- Archives: browse, extract and compress, behind traversal and bomb checks.
- Other computers (SSH), Windows and NFS shares, and rclone cloud remotes.
- Custom actions from `~/.config/gust/actions/*.toml`. These run as argv,
  never through a shell.
- Open as administrator: a separate window, clearly marked, started through
  polkit. It manages files but never opens or runs them.
- Settings: reopen last session's tabs, density, hidden files, key preset.

## Install

Arch / Omarchy (from the AUR, once published):

    yay -S gust

From source:

    git clone <repository> gust && cd gust
    cargo build --release --locked
    install -Dm755 target/release/gust ~/.local/bin/gust

Needs Rust 1.90+, Qt 6 (qt6-base, qt6-declarative) and a Wayland session.
Optional helpers are listed in `PKGBUILD` (`optdepends`). Gust works without
each one and says what is missing when you reach for it.

## Make it the default file manager

    gust --default on      # take over inode/directory, remembering the old one
    gust --default off     # put the previous one back exactly

Nothing changes at install time.

## Safety rules the code follows

- No colour, font, radius or spacing literal in QML. `just lint` fails the
  build if one appears.
- Untrusted bytes (thumbnailers, archive listers) run under bubblewrap and
  prlimit, or not at all.
- No secret is ever put in argv, the environment, logs or config files.
- The UI thread never touches the filesystem, and every remote call can be
  cancelled.
- Activating a file never runs it. Programs run only through **Run…**, behind
  a confirmation where Cancel has focus.

## Development

    just build          # cargo build --workspace
    just test           # library tests
    just lint           # fmt, clippy, QML token rules
    just qa             # drives the real window with real keys and pointer
    just theme-matrix   # screenshots every installed Omarchy theme
    just bench-field    # Flea's field bench, Gust as an entrant

Layout: `crates/core` (listing, sort, search, theme), `crates/ops` (copy, move,
trash, undo journal), `crates/remote` (mounts), `crates/thumbs` (sandboxed
thumbnails), `crates/app` (Qt bridge), `ui/` (QML). Design notes are in
`docs/DESIGN.md`, and every judgement call is in `docs/DECISIONS.md` with
how to revert it.

## License

MIT.
