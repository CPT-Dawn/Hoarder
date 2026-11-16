# Command line

Everything Hoarder does is a `hoarder` subcommand, so scripts and keybindings can drive it. `hoarder --help` lists them and `hoarder <command> --help` documents each flag; `man hoarder` comes with every package.

| Command | Does |
| --- | --- |
| `shot [region\|window\|output\|all\|last]` | Take a Capture |
| `pick` | Pick a colour off the screen and copy it |
| `copy` / `paste` | The clipboard, with every `wl-copy` / `wl-paste` flag |
| `picker` | Open the Picker |
| `history` | List, read, restore, delete, pin and clear Clips |
| `pause [DURATION]` / `resume` | Stop and restart recording into History |
| `clear` | Empty the clipboard and keep the Daemon from restoring it |
| `status` | The Daemon's state and the clipboard owner |
| `config` | Read, change and check `config.toml` |
| `service enable\|disable\|status` | Manage the systemd user service |
| `setup hyprland\|sway\|niri [--apply]` | Print or write keybindings |
| `doctor` | Check the setup; exit 1 with a fix for every problem |
| `completions <shell>` | Print a completion script |

Every command takes `--set KEY=VALUE` to override one setting for that run.

## Screenshots

```sh
hoarder shot window --list              # list windows; `--list TITLE` captures one from its own buffer
hoarder shot output -o - | swappy -f -  # pipe to an editor
hoarder shot --delay 3 --format webp    # count down, then capture as WebP
hoarder shot last                       # the last Region again, no Overlay
hoarder shot --no-save --no-copy -o shot.png
```

`--live` skips the freeze, `--confirm` always enters Refine, and `--cursor` includes the pointer. Formats are `png`, `webp`, `jpeg`, `avif` and `qoi`.

## Colours

```sh
hoarder pick                # copy the colour under the pointer as hex
hoarder pick --format rgb   # or hsl
```

## Clipboard

```sh
echo hi | hoarder copy                  # every wl-copy flag works
hoarder copy --sensitive < token.txt    # mark it as a password: never stored
hoarder paste -t image > clip.png       # every wl-paste flag works
hoarder paste --watch notify-send Copied
```

Install `hoarder-wl-compat` and everything that already calls `wl-copy`/`wl-paste` (Neovim, tmux, shell scripts) uses Hoarder unchanged.

## History

```sh
hoarder history list | head             # id, a tab, a preview; newest first
hoarder history get 42 > clip.bin
hoarder history restore 42
hoarder history pin 42                  # exempt from expiry, limits and `clear`
hoarder pause 5m                        # stop recording; `hoarder resume` to undo
hoarder clear                           # empty the clipboard and keep it empty
```

`pause` takes `90`, `30s`, `5m` or `1h30m`, or no duration for "until `resume`". Copies still outlive their apps while paused.

[Back to the README](../README.md)
