# Maintainer: Gust contributors
#
# Packaging notes
# ---------------
# - Nothing here changes a user's defaults. `gust --default on` is an
#   explicit, per-user, reversible action (docs/DESIGN.md §6.2); a package
#   install must never silently take over the file manager association.
# - The optional dependencies are genuinely optional: Gust degrades
#   gracefully and says why when they are missing (plan §8).

pkgname=gust
pkgver=0.1.0
pkgrel=1
pkgdesc="A fast, calm file manager for Omarchy"
arch=('x86_64' 'aarch64')
url="https://github.com/omarchy/gust"
license=('MIT')
# LTO breaks the cxx-qt bridge: the generated C++ becomes LLVM bitcode and
# the linker drops symbols the Rust side needs, failing with
# "undefined symbol: rust$cxxqtlib1$cxxbridge1$...$qguiapplication_exec".
# Arch enables LTO by default, so this must be explicit.
options=(!lto)
depends=('qt6-base' 'qt6-declarative' 'dbus')
makedepends=('rust' 'cargo' 'cmake' 'just')
optdepends=(
  'bubblewrap: sandboxed thumbnail rendering'
  'util-linux: resource limits for thumbnail helpers'
  'imagemagick: image thumbnails'
  'sshfs: browse other computers over SSH'
  'rclone: cloud storage accounts'
  'gvfs: network shares and system trash integration'
)
source=("$pkgname-$pkgver.tar.gz")
sha256sums=('SKIP')

build() {
  cd "$pkgname-$pkgver"
  # §8: no private paths in artifacts. Without this the build machine's
  # home directory is baked into panic messages from dependencies, so a
  # shipped binary carries strings like /home/<builder>/.cargo/registry/...
  export RUSTFLAGS="$RUSTFLAGS --remap-path-prefix=$SRCDEST=/usr/src \
    --remap-path-prefix=$srcdir=/usr/src \
    --remap-path-prefix=$HOME/.cargo/registry=/usr/src/cargo-registry"
  cargo build --release --locked
}

check() {
  cd "$pkgname-$pkgver"
  # Library tests only: the desktop QA suite needs a Wayland session and is
  # run by `just qa` against the release artifact, not by makepkg.
  cargo test --release --locked --workspace --lib
}

package() {
  cd "$pkgname-$pkgver"
  install -Dm755 "target/release/gust" "$pkgdir/usr/bin/gust"
  install -Dm644 "packaging/gust.desktop" \
    "$pkgdir/usr/share/applications/gust.desktop"
  install -Dm644 "LICENSE" "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
  install -Dm644 "README.md" "$pkgdir/usr/share/doc/$pkgname/README.md"
}
