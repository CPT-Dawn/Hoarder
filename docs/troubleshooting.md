# Troubleshooting

Start with `hoarder doctor`: it checks the compositor, protocols, paths, service and conflicting tools, prints a fix for every problem, and exits 1 while any remain. Daemon logs are in `journalctl --user -u hoarder`.

## Does it work with my existing `wl-copy` / `wl-paste` scripts?

Yes. `hoarder copy` and `hoarder paste` accept every wl-clipboard flag. Install `hoarder-wl-compat` and the `wl-copy` / `wl-paste` commands themselves come from Hoarder.

## Why not keep wl-clip-persist?

It re-offers every copy the moment it happens, which breaks pasting in Firefox, GTK apps, IntelliJ, GIMP and Discord. Hoarder steps in only after the source app has gone ([ADR-0005](adr/0005-takeover-only-when-selection-vanishes.md)). `hoarder doctor` tells you if `wl-clip-persist` or a `cliphist` watcher is still running; remove them from your compositor config.

## Will my passwords end up in History?

No. Copies marked by a password manager are never read. Copies from password-manager apps, or that look like keys or tokens, stay in locked memory for two minutes and never touch disk. See [Privacy](privacy.md).

## <kbd>Super</kbd>+<kbd>V</kbd> does something else on Hyprland

Hyprland's example config binds <kbd>Super</kbd>+<kbd>V</kbd> to `togglefloating`. Remove that bind, or rebind `hoarder picker` in the file `hoarder setup hyprland --apply` wrote.

## "The Daemon is not running"

`hoarder picker` and `hoarder history` need the Daemon. Start it with `systemctl --user enable --now hoarder` (or `hoarder service enable`), then check `hoarder status`.

## Still stuck

[Open an issue](https://github.com/CPT-Dawn/Hoarder/issues/new/choose) with the output of `hoarder doctor` and `hoarder --version`. Never paste clipboard contents or History that might hold secrets.

[Back to the README](../README.md)
