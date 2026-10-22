# Omarchy integration proposal — preparation only

This file is a local source proposal. It does not assert the state of an upstream
pull request or a published registry package, and it is not a request to submit,
deploy or change Omarchy configuration.

The proposed integration would add `gust` / `gust.desktop` to the file-manager
choices after release and installed-artifact qualification. Package installation
must remain inert; selecting the default and selecting the portal backend are
separate explicit user actions with their documented rollback behavior.

Use the actual dependency manifest and runtime-file list rather than describing
Gust as depending only on Qt or assuming a single executable once the archive
helper is integrated. Include only verified qualification for the exact shipped
revision. Historical benchmark/theme evidence does not establish that status.
