# Set up with an AI agent

Hoarder sets up non-interactively, so a coding agent can install and configure it for you. Point your agent at this page, or paste the instructions below.

## Instructions for agents

Goal: install Hoarder, start its Daemon, add keybindings, and verify. Every step is idempotent.

1. **Check the environment.** `echo $XDG_SESSION_TYPE` must print `wayland`. Detect the compositor from `$XDG_CURRENT_DESKTOP` (or `$HYPRLAND_INSTANCE_SIGNATURE` / `$SWAYSOCK` / `$NIRI_SOCKET`). Stop on GNOME: it is unsupported. On KDE Plasma only the clipboard features work.
2. **Install.**
   - Arch-based: `paru -S --needed hoarder-bin` (or `yay`). Add `hoarder-wl-compat` only if the user wants Hoarder to replace `wl-clipboard`; it conflicts with that package.
   - Other distros: `cargo install --locked --git https://github.com/CPT-Dawn/Hoarder --tag v1.0.4 hoarder`, or the tarball from the latest GitHub release.
   - Confirm with `hoarder --version`.
3. **Start the Daemon.** `systemctl --user enable --now hoarder` (equivalent: `hoarder service enable`).
4. **Keybindings.** For `hyprland`, `sway` or `niri`: run `hoarder setup <wm> --apply`. It writes Hoarder's own file next to the compositor config and **prints one include line**. Add that exact line to the user's main compositor config if it is not already there; do not write the bindings into the main config yourself. For other compositors, bind the four commands from [Keybindings](keybindings.md#compositor-bindings) in their config.
5. **Remove conflicts.** If `wl-clip-persist` or a `cliphist` watcher (`wl-paste --watch cliphist store`) is started from the compositor config, ask the user before removing those lines: they duplicate Hoarder and `wl-clip-persist` breaks pasting in some apps.
6. **Verify.** Run `hoarder doctor`. Exit code 0 means done. On exit code 1, each reported problem includes its fix; apply it and re-run.
7. **Reload** the compositor config (`hyprctl reload`, `swaymsg reload`, or `niri msg action load-config-file`) and tell the user: <kbd>Print</kbd> for a screenshot, <kbd>Super</kbd>+<kbd>V</kbd> for History.

Do not edit `~/.config/hoarder/config.toml` unless the user asks; every key has a sensible default. Use `hoarder config set <key> <value>` for changes, and see [Configuration](config.md) for the keys.

[Back to the README](../README.md)
