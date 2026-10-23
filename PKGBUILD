# Maintainer: Gust contributors
#
# Packaging notes
# ---------------
# - Nothing here changes a user's defaults. `gust --default on` is an
#   explicit, per-user, reversible action (docs/DESIGN.md §6.2); a package
#   install must never silently take over the file manager association.
# - Every program Gust calls is a dependency, so a fresh install works with
#   nothing else to set up. Gust still degrades gracefully and says why if
#   one is removed later (plan §8).

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
# Connection and device helpers ship with Gust; connecting remains explicit.
depends=(
  'qt6-base' 'qt6-declarative' 'dbus'
  'openssh' 'sshfs' 'fuse3' 'rclone'
  'gvfs' 'gvfs-afc' 'gvfs-smb' 'gvfs-nfs' 'gvfs-mtp' 'gvfs-gphoto2'
  'usbmuxd' 'polkit'
  # Clipboard, Open with, terminal and folder jump.
  'wl-clipboard' 'xdg-utils' 'xdg-terminal-exec' 'zoxide'
  # Sandboxed thumbnails and Quick Look previews.
  'bubblewrap' 'util-linux' 'imagemagick' 'ffmpegthumbnailer' 'ffmpeg'
  'pipewire-audio' 'poppler'
  # Archives, USB drives and discs, Open/Save dialogs for other apps.
  'libarchive' 'udisks2' 'xdg-desktop-portal'
)
makedepends=('rust' 'cargo' 'cmake' 'just')
# Tailscale is a network service with its own account: Gust lists its devices
# when you already run it, but never installs it for you.
optdepends=(
  'tailscale: list Tailscale devices to connect to'
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
  # Archive listing and extraction run through this helper inside bwrap;
  # without it Gust disables archives.
  install -Dm755 "target/release/gust-archive-helper" \
    "$pkgdir/usr/bin/gust-archive-helper"
  install -Dm644 "packaging/gust.desktop" \
    "$pkgdir/usr/share/applications/gust.desktop"
  install -Dm644 "LICENSE" "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
  # No FileManager1 activation file here: that name belongs to whichever
  # file manager the user chose. `gust --default on` writes a per-user one.
  # The file-chooser backend is installed but inert: xdg-desktop-portal uses
  # it only when portals.conf selects it (`gust --file-dialogs on`).
  install -Dm644 "packaging/gust.portal" \
    "$pkgdir/usr/share/xdg-desktop-portal/portals/gust.portal"
  install -Dm644 "packaging/org.freedesktop.impl.portal.desktop.gust.service" \
    "$pkgdir/usr/share/dbus-1/services/org.freedesktop.impl.portal.desktop.gust.service"
  install -Dm644 "README.md" "$pkgdir/usr/share/doc/$pkgname/README.md"
}
