# Packaging contract changes

The two recipes now have one canonical body instead of divergent draft
instructions. Their real upstream URL, full source revision, snapshot version
and SHA-256 replace the local archive/placeholder URL/SKIP metadata. Source
pinning is reproducible and does not create an upstream release.

The package build uses a source-bound artifact receipt, includes the ops archive
helper when that entrypoint exists, and stages only the runtime file manifest.
Actual staging validates files, modes, architecture and linking; static metadata
alone is not installed qualification. Runtime helper providers are checked for
drift, including GLib/GIO, curl, tar/zstd, MIME data and the active media helper.
Optional preview prerequisites remain explicit capabilities.

`check()` invokes the shared skip-aware required workspace library runner.
Ignored tests, missing fixtures and empty success cannot qualify a package.
Desktop, KVM model, theme, benchmark and real-device/account checks are separate;
public hosted CI does not claim those passes. See [release preparation](README.md)
and [testing](../TESTING.md) for actual commands and limitations.
