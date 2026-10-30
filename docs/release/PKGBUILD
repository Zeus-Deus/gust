# Maintainer: Gust contributors
#
# Packaging notes
# ---------------
# - Nothing here changes a user's defaults. `gust --default on` is an
#   explicit, per-user, reversible action (docs/DESIGN.md §6.2); a package
#   install must never silently take over the file manager association.
# - Common file-operation and connection helpers are dependencies. Optional
#   preview capabilities are declared separately; tool/service availability
#   still needs runtime qualification.

pkgname=gust
_commit=62e1d2b15fe5f441fdff63101b5de211c119abd3
pkgver=0.1.0.r120.g62e1d2b15fe5
pkgrel=1
pkgdesc="A fast, calm file manager for Omarchy"
arch=('x86_64' 'aarch64')
url="https://github.com/Zeus-Deus/gust"
license=('MIT')
# LTO breaks the cxx-qt bridge: the generated C++ becomes LLVM bitcode and
# the linker drops symbols the Rust side needs, failing with
# "undefined symbol: rust$cxxqtlib1$cxxbridge1$...$qguiapplication_exec".
# Arch enables LTO by default, so this must be explicit.
options=(!lto)
# Connection and device helpers ship with Gust; connecting remains explicit.
depends=(
  'qt6-base>=6.11.2' 'qt6-declarative>=6.11.2' 'dbus' 'glibc' 'gcc-libs' 'glib2>=2.66'
  'openssh' 'sshfs' 'fuse3' 'rclone'
  'gvfs' 'gvfs-afc' 'gvfs-smb' 'gvfs-nfs' 'gvfs-mtp' 'gvfs-gphoto2' 'gvfs-dnssd'
  'usbmuxd' 'polkit'
  # Clipboard, Open with, terminal and folder jump.
  'wl-clipboard' 'xdg-utils' 'xdg-terminal-exec' 'zoxide'
  # Sandboxed thumbnails and Quick Look previews.
  'bubblewrap' 'util-linux' 'imagemagick' 'ffmpegthumbnailer' 'ffmpeg'
  'pipewire-audio' 'poppler'
  # Browse neighbouring images, audio and video in one player.
  'mpv'
  # Archives, USB drives and discs, Open/Save dialogs for other apps.
  'libarchive' 'tar' 'zstd' 'udisks2' 'xdg-desktop-portal'
  'shared-mime-info' 'curl' 'openssl' 'coreutils'
)
makedepends=('rust' 'cargo' 'python' 'pkgconf')
# Tailscale is a network service with its own account: Gust lists its devices
# when you already run it, but never installs it for you.
optdepends=(
  'tailscale: list Tailscale devices to connect to'
  'libraw: RAW preview helper (capability-gated)'
  'perl-image-exiftool: camera metadata (capability-gated)'
  'libreoffice-fresh: sandboxed Office/HTML conversion (capability-gated)'
)
source=("$pkgname-$_commit.tar.gz::https://codeload.github.com/Zeus-Deus/gust/tar.gz/$_commit")
sha256sums=('ab0b77575d330123a2a1fb4fed3a3868cf38bbb5a5bf1b50804cf4553881b6f8')

build() {
  cd "$pkgname-$_commit"
  # §8: no private paths in artifacts. Without this the build machine's
  # home directory is baked into panic messages from dependencies, so a
  # shipped binary carries strings like /home/<builder>/.cargo/registry/...
  export RUSTFLAGS="$RUSTFLAGS --remap-path-prefix=$SRCDEST=/usr/src \
    --remap-path-prefix=$srcdir=/usr/src \
    --remap-path-prefix=$HOME/.cargo/registry=/usr/src/cargo-registry"
  # Owns the build lease and records source/executable digests. Includes
  # cargo build -p gust-ops --bin gust-archive-helper when its source exists.
  python3 packaging/build-artifact.py
}

check() {
  cd "$pkgname-$_commit"
  # Library tests only: the desktop QA suite needs a Wayland session and is
  # run by `just qa` against the release artifact, not by makepkg.
  ./tests/run-tests.sh --workspace --package-unit --required --release
}

package() {
  cd "$pkgname-$_commit"
  install -Dm755 "target/release/gust" "$pkgdir/usr/bin/gust"
  # Archive listing and extraction run through this helper inside bwrap;
  # without it Gust disables archives. Older pinned snapshots predate it.
  if [[ -f "crates/ops/src/bin/gust-archive-helper.rs" ]]; then
    install -Dm755 "target/release/gust-archive-helper" "$pkgdir/usr/bin/gust-archive-helper"
  fi
  install -Dm644 "packaging/gust.desktop" \
    "$pkgdir/usr/share/applications/gust.desktop"
  install -Dm644 "packaging/gust-chooser.desktop" \
    "$pkgdir/usr/share/applications/gust-chooser.desktop"
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
