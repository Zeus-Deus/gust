
## Becoming the default file manager

    gust --default on     # claim inode/directory, remembering what was there
    gust --default off    # put the previous one back

Nothing about this happens at install time. A package that silently takes
over the file manager association is a package people uninstall.

`--default off` restores exactly what was found, including "nothing was set",
which is different from "set to some fallback". If the association was changed
by something else after Gust claimed it, that newer choice is reported and
left alone rather than being stamped over (plan §8). The same goes for a Gust
that was already the default before `--default on` was run: there is no
earlier app on record, so `off` says the default was not changed.

A `mimeapps.list` kept as a symbolic link by a dotfile manager stays a link;
the file it points to is updated in place. If
`~/.config/<desktop>-mimeapps.list` (for example `hyprland-mimeapps.list`)
sets `inode/directory` to another app, that file wins on this desktop:
`--default on` names it and leaves it to you to edit.

Omarchy's own `omarchy default` command has agent/browser/editor/terminal but
no file-manager verb yet. When one is added, it should call `gust --default`
rather than editing `mimeapps.list` itself, so the rollback record stays
accurate.
