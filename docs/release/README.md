# Release preparation (no publication)

The root `PKGBUILD` is canonical; `docs/release/PKGBUILD` must be byte-for-byte
identical. These are reviewable source recipes, not a published registry entry.
No install hook changes defaults, mounts a volume, pairs a phone or starts a
service. Runtime prerequisites are not hardware or provider qualification.

## Immutable source metadata

`_commit` is a full source revision, `source` fetches its GitHub archive from the
actual upstream repository and `sha256sums` verifies those bytes. The snapshot
package version combines the workspace version, revision count and commit. It
does not invent an upstream release tag. Refresh metadata only after the source
commit is available remotely:

```sh
python3 packaging/pin-source.py FULL_SOURCE_COMMIT
python3 tests/check-setup-packaging.py
makepkg --printsrcinfo
```

The pinning tool downloads within a size budget, inspects Cargo metadata without
extracting or executing it, requires the skip-aware runner and updates both
recipes. Review the exact source revision/checksum diff. It does not tag,
publish a release/package, change repository settings or submit to a registry.
A metadata-only commit may pin its preceding source commit; qualification must
record both the source pin and the PR head. Lockfiles pin Rust dependencies;
reproducing a source snapshot is distinct from bit-identical output across
unrecorded compiler, Qt and system-library versions.

## Build, stage and inspect the real files

```sh
python3 packaging/build-artifact.py
python3 packaging/check-package.py --stage --output /tmp/gust-package-fresh
python3 packaging/check-dependencies.py --installed
```

The builder owns the nonblocking build lease, uses at most two Cargo jobs by
default, records the source-input digest and actual executable hashes, and remaps
checkout/registry paths. Raw build logs/receipts stay local under `target/release`.
Changes during a build invalidate the receipt. A separately coordinated build is
needed if the lease is busy; do not nest locks.

When `crates/ops/src/bin/gust-archive-helper.rs` exists in the selected snapshot,
the builder runs `cargo build --release --locked -p gust-ops --bin
gust-archive-helper` as well as the app build. `package()` installs that helper
beside Gust at `/usr/bin/gust-archive-helper`. A snapshot that predates the helper
cannot qualify the new archive contract. The parent still needs bwrap/prlimit;
shipping the helper alone does not establish sandbox containment.

The staging check executes the actual audited `package()` function into a fresh
private root, verifies its exact file set against `packaging/runtime-files.json`,
then checks modes, source/configuration bytes, executable build receipts, ELF
machine type and resolved dynamic libraries. `--root` verifies an existing staged
or installed root against the same build. Symlinked runtime files, missing
helpers, replaced binaries, stale source receipts and wrong architectures fail.
A development-tree launch or recipe text check is not this installed check.

`package()` stages the executable(s), desktop entry, inert portal/service files,
license and README. It does not copy audit logs, receipts or screenshots. A clean
Arch `makepkg` build/install and its dependency closure still need separate
qualification; Ubuntu hosted staging is useful compatibility evidence only.

## Dependency contracts

`packaging/helper-packages.json` maps literal subprocess helpers to required,
optional or desktop-session providers. The drift check scans production literal
`Command::new` and thumbnail-helper additions, and checks declared Arch packages.
Computed argv, configured applications and transitive GVfs/codec helpers still
require owner review; this is not an exhaustive static call graph.

The baseline package supplies Qt, D-Bus/GLib, SSHFS/rclone/GVfs backends, clipboard,
preview and archive tools. Curl/OpenSSL/GLib are included for the remote lane's native GIO
and TLS-sharing integration; installing them does not claim that integration is
already consumed in every app revision. GVfs FTP/WebDAV support is supplied by
the matching installed GVfs build and must be tested with its actual backend.
On Arch, FTP/FTPS come from GVfs; WebDAV needs
[`gvfs-dnssd`](https://archlinux.org/packages/extra/x86_64/gvfs-dnssd/), whose
[file list](https://archlinux.org/packages/extra/x86_64/gvfs-dnssd/files/) includes
`/usr/lib/gvfsd-dav` and `dav.mount`. `http.mount` alone does not establish WebDAV
support. Missing backend files must remain an explicit unavailable capability.
The mpv prerequisite matches the active neighboring-media integration; a package
manifest alone does not qualify that unpublished UI flow.

Optional preview contracts from the preview lane are `dcraw_emu` (LibRaw),
`exiftool` (perl-image-exiftool), and headless LibreOffice conversion with the
Writer/Impress/Calc filters (libreoffice-fresh). Capability/tool checks and real
sandboxed codec fixtures must run on the integrated revision; these entries do
not establish RAW/Office rendering, hardware decoding or a finished UI. Tailscale
is also optional and requires the user's separately configured service/account.
The seven existing setup capabilities remain owned by the remote lane.

## Qualification handoff

Use `tests/release-gates.py --required` with explicitly provisioned fixtures,
installed root and the UI-owned native runner. Keep exact source/PR head, invoked
checks, skips/failures, architectures and genuine runner limits in a private
handoff. Theme/native captures stay local and outside Git. Public CI uploads no
raw artifacts. Record fresh benchmark/hardware/provider results only when those
checks actually ran; historical notes never substitute for them. No release,
package publication, deployment, merge or shared-history rewrite is part of this
preparation workflow.
