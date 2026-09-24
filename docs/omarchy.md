
## Becoming the default file manager

    gust --default on     # claim inode/directory, remembering what was there
    gust --default off    # put the previous one back

Nothing about this happens at install time. A package that silently takes
over the file manager association is a package people uninstall.

`--default off` restores exactly what was found, including "nothing was set",
which is different from "set to some fallback". If the association was changed
by something else after Gust claimed it, that newer choice is reported and
left alone rather than being stamped over (plan §8).

Omarchy's own `omarchy default` command has agent/browser/editor/terminal but
no file-manager verb yet. When one is added, it should call `gust --default`
rather than editing `mimeapps.list` itself, so the rollback record stays
accurate.
