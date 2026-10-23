# What the draft PKGBUILD corrects (against the repo's `PKGBUILD`)

| Before | After | Why |
|---|---|---|
| `url="https://github.com/omarchy/gust"` | placeholder `<owner>/gust` | That repository does not exist; the owner sets the real one when publishing. |
| `source=("$pkgname-$pkgver.tar.gz")`, a local file | the tag tarball from `$url` | A package that cannot fetch its source does not build in a clean chroot. |
| linked-library dependencies: `qt6-base qt6-declarative dbus` | `qt6-base qt6-declarative gcc-libs glibc` (plus the shared connection helpers below) | The draft retains its existing linked-library correction; Qt6 Core/Gui/Qml/Network/DBus are from qt6-base/declarative. |
| `makedepends=('rust' 'cargo' 'cmake' 'just')` | `rust cargo` | cxx-qt builds its C++ through the `cc` crate; no `cmake` in Cargo.lock; `just` is not called by the PKGBUILD. |
| `cargo build --release --locked` with no `prepare()` | `cargo fetch --locked` in `prepare()`, `--frozen` after | Arch Rust packaging guidelines: network only in `prepare()`. |
| feature helper differences | + `xdg-terminal-exec`, `zoxide`, `ffmpeg`, `pipewire-audio`, `poppler`, optional `tailscale` | The repo `PKGBUILD` now matches: both manifests require the same helpers. |
| `--remap-path-prefix=$SRCDEST=…` | dropped (`$srcdir` and the cargo registry remain) | `$SRCDEST` is the download cache, not where the code is built; `$srcdir` is. |

## Shared connection and device dependency policy

Both manifests now require `openssh`, `sshfs`, `fuse3`, `rclone`, `gvfs`,
`gvfs-afc`, `gvfs-smb`, `gvfs-nfs`, `gvfs-mtp`, `gvfs-gphoto2` and `polkit`.
Both also explicitly require `usbmuxd` for Apple-device support.

## Shared feature dependency policy

Every other program Gust calls is required too, so installing the package is
the whole setup: `wl-clipboard`, `xdg-utils`, `xdg-terminal-exec`, `zoxide`,
`bubblewrap`, `util-linux`, `imagemagick`, `ffmpegthumbnailer`, `ffmpeg`,
`pipewire-audio`, `poppler`, `mpv`, `libarchive`, `udisks2` and
`xdg-desktop-portal`. `tailscale` is the only `optdepends` entry: it is a
network service tied to the user's own account, so Gust uses it when present
and never installs it. Every required package is in Arch's official
repositories. The existing versions, source URLs, build steps and
`options=(!lto)` are unchanged.

This ships helpers, not automatic connections. There are no Gust install
hooks to set defaults, mount devices, pair phones, accept Trust or enable/start
services. Source builds and older installations offer **Install support**
with a package preview and system authorization, then require an explicit
**Connect/Open** after installation. Apple devices expose only media and app
file-sharing areas, not arbitrary private app/iCloud data or music-library sync.

`python3 tests/check-setup-packaging.py` checks both manifests without sourcing
them, building packages or accessing devices. It requires the connection and
feature helper sets, allows only Tailscale as optional, rejects
required/optional overlap, and guards against install hooks and
runtime actions in packaging. Regression-first evidence retains the original
failure, a corrected-fixture dependency failure, and an identical-test pass.

Metadata checks: `bash -n PKGBUILD`, `bash -n docs/release/PKGBUILD`, and
`makepkg --printsrcinfo` for each manifest. A real package build still needs
the published tag and is on the owner's list; it is not part of these checks.
