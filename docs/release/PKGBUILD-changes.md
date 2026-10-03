# What the draft PKGBUILD corrects (against the repo's `PKGBUILD`)

| Before | After | Why |
|---|---|---|
| `url="https://github.com/omarchy/gust"` | placeholder `<owner>/gust` | That repository does not exist; the owner sets the real one when publishing. |
| `source=("$pkgname-$pkgver.tar.gz")`, a local file | the tag tarball from `$url` | A package that cannot fetch its source does not build in a clean chroot. |
| `depends=('qt6-base' 'qt6-declarative' 'dbus')` | `qt6-base qt6-declarative gcc-libs glibc` | `ldd` on the built binary: Qt6 Core/Gui/Qml/Network/DBus only (all in qt6-base/declarative); D-Bus is reached through QtDBus, not the `dbus` package's library. |
| `makedepends=('rust' 'cargo' 'cmake' 'just')` | `rust cargo` | cxx-qt builds its C++ through the `cc` crate; no `cmake` in Cargo.lock; `just` is not called by the PKGBUILD. |
| `cargo build --release --locked` with no `prepare()` | `cargo fetch --locked` in `prepare()`, `--frozen` after | Arch Rust packaging guidelines: network only in `prepare()`. |
| optdepends missed four programs Gust runs | + `polkit`, `xdg-terminal-exec`, `zoxide`, `openssh` | Open as administrator (pkexec), Run…/Open terminal, folder jump, SSH host keys. |
| `--remap-path-prefix=$SRCDEST=…` | dropped (`$srcdir` and the cargo registry remain) | `$SRCDEST` is the download cache, not where the code is built; `$srcdir` is. |

Checked with `bash -n PKGBUILD` and `makepkg --printsrcinfo` only; a real
`makepkg` needs the published tag and is on the owner's list.
