# GUST

A fast, calm file manager for Omarchy. One Rust binary, Qt Quick UI (cxx-qt), MIT.
Your files wherever they are — local, USB, phone, other computers, NAS, cloud — in one window
that looks like a native Quattro surface and never loses data.

## Layout
- `crates/core`     listing, watch, sort, MIME, search      (no UI, no Qt)
- `crates/ops`      copy/move/trash/rename, jobs, undo journal
- `crates/remote`   mount manager: sshfs, gio, rclone, discovery
- `crates/thumbs`   freedesktop cache + sandboxed thumbnailers
- `crates/app`      cxx-qt bridge, models, image provider, main
- `ui/`             QML views; `ui/theme/` maps Omarchy tokens
- `docs/DESIGN.md`  types and protocols shared between crates — change it first
- `bench/`          field bench; `tests/` per-crate + desktop QA scripts

## Rules
- No colour, font, radius or spacing literal in QML. Tokens only, from
  `~/.local/state/omarchy/current/theme/{colors,shell}.toml`; fallback palette off-Omarchy.
- Destructive paths ship with a test for the confirmation and the undo. Undo verifies
  target identity (dev+inode+mtime) and refuses if it changed.
- Untrusted bytes (thumbnailers, archive listers) run under bwrap+prlimit or not at all.
- No secret in argv, env, logs or config files. Keyring or rclone config only.
- The UI thread never touches the filesystem. Every remote call has a cancel token.
- Bench (100k settle, first window, PSS) and the 22-theme screenshot matrix are CI gates.
- One lane owns one crate or directory; shared changes go through docs/DESIGN.md.
- Done means proven on the real desktop with a screenshot, not described.

## Commands
`just build` · `just test` · `just bench` · `just qa` (drives the real window) · `just lint`

## Omarchy integration
`--default` flips inode/directory + keybinds (reversible with `--default off`).
Ships via OPR; `omarchy default file-manager` selects it. See docs/omarchy.md.
