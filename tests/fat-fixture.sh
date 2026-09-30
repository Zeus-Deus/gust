#!/usr/bin/env bash
# A real FAT32 filesystem (a 64 MiB loop image), mounted for the current user,
# so copy tests hit the kernel's vfat rules rather than a simulation.
#   tests/fat-fixture.sh start   → prints GUST_FAT_FIXTURE=<mountpoint>
#   tests/fat-fixture.sh stop
# Needs root for losetup/mount; refuses cleanly without sudo -n.
set -uo pipefail
ROOT="${GUST_FAT_ROOT:-/tmp/gust-fat-fixture}"
IMG="$ROOT/fat32.img"; MNT="$ROOT/mnt"
start() {
  sudo -n true 2>/dev/null || { echo "a FAT mount needs root; sudo -n is unavailable" >&2; return 1; }
  mkdir -p "$MNT"
  mountpoint -q "$MNT" && { echo "GUST_FAT_FIXTURE=$MNT"; return 0; }
  truncate -s 64M "$IMG"
  mkfs.fat -F 32 -n GUSTFAT "$IMG" >/dev/null
  sudo -n mount -o loop,uid="$(id -u)",gid="$(id -g)",umask=022 "$IMG" "$MNT"
  echo "GUST_FAT_FIXTURE=$MNT"
}
stop() {
  mountpoint -q "$MNT" && sudo -n umount "$MNT"
  rm -rf "$ROOT"
}
"${1:-start}"
