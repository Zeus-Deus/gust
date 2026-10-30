
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

## Answering other apps' Open and Save dialogs

    gust --file-dialogs on    # or Settings > Open/Save
    gust --file-dialogs off

`on` changes one key in `~/.config/xdg-desktop-portal/<desktop>-portals.conf`
and adds one marked block to `~/.config/hypr/windows.lua`:

    -- >>> gust: Open/Save dialogs float (remove with: gust --file-dialogs off)
    o.window("gust-chooser", { tag = "+floating-window" })
    -- <<< gust

That is Omarchy's own dialog treatment: floating, centred, 875x600. `off` puts
the portal key back, removes exactly that block and reloads Hyprland. Without
`windows.lua` nothing is added there and dialogs tile. Running `on` again is
safe, and adds the block for anyone who enabled dialogs before it existed.

The portal backend has to be installed for the portal to find it: the package
ships `gust.portal` and its D-Bus service. A build run from a checkout does not
register them.
