# Hoarder

[![CI](https://github.com/CPT-Dawn/Hoarder/actions/workflows/ci.yml/badge.svg)](https://github.com/CPT-Dawn/Hoarder/actions/workflows/ci.yml)
[![AUR](https://img.shields.io/aur/version/hoarder-bin)](https://aur.archlinux.org/packages/hoarder-bin)
[![License](https://img.shields.io/github/license/CPT-Dawn/Hoarder)](LICENSE)

Pixel-exact screenshots and a clipboard that outlives the app that set it, for Wayland.

One Rust binary replaces grim, slurp, grimblast/Hyprshot, hyprpicker, wl-clipboard, wl-clip-persist and cliphist:

- **Capture.** `hoarder shot` freezes every output at its native resolution and lets you pick a Region, a Window, an Output or All Outputs in one gesture. The image is cut from the compositor's own buffers, never resampled: crisp at fractional scaling, correct on 10-bit outputs, no cursor or selection border leaking in. It lands on the clipboard as lossless PNG, is saved in the format you choose, and is announced with a notification.
- **Persistence.** A small, event-driven Daemon keeps whatever you copied pasteable after the app that copied it closes. It takes over only once the app is gone, so it never breaks Firefox, GTK, IntelliJ, GIMP or Discord the way re-offering every copy does.
- **History.** Every copy is searchable from a fast, themed Picker (Super+V). Password-manager copies are never touched, and anything that looks like a secret stays in locked memory for two minutes and never reaches disk.

<!-- Demo GIFs: docs/media/shot.gif, docs/media/picker.gif -->

## Status

Hoarder is complete: v1.0 is the finished product and the project is in maintenance mode. Bugs are fixed as they are reported, and new features are considered when users ask for them. [Open an issue](https://github.com/CPT-Dawn/Hoarder/issues/new/choose) for either.

## Install

Arch Linux, from the AUR:

| Package | What |
|---|---|
| `hoarder-bin` | Prebuilt binary |
| `hoarder` | Built from the release source |
| `hoarder-git` | Built from the latest commit |
| `hoarder-wl-compat` | `wl-copy` and `wl-paste` as Hoarder's shims, in place of wl-clipboard |

```sh
paru -S hoarder-bin          # or yay, or makepkg
```

Every package ships the systemd user unit, man pages, bash/zsh/fish completions and the default config (`/usr/share/doc/hoarder/config.toml`).

Elsewhere: a tarball from [Releases](https://github.com/CPT-Dawn/Hoarder/releases), or

```sh
cargo install --locked --git https://github.com/CPT-Dawn/Hoarder --tag v1.0.1 hoarder
```

Nothing is published to crates.io.

### Compositors

| | Capture | Clipboard and History | Window picking |
|---|---|---|---|
| Hyprland | yes | yes | yes |
| Sway 1.11+ | yes | yes | yes |
| niri, river, labwc, COSMIC | yes | yes | Region and Output only |
| Wayfire | yes (wlr-screencopy) | yes | Region and Output only |
| KDE Plasma (KWin) | no: KWin only offers capture through the portal | yes | — |
| GNOME | no | no | — |

## Quickstart

```sh
systemctl --user enable --now hoarder   # the Daemon; or: hoarder service enable
hoarder setup hyprland --apply          # or sway, niri: keybindings and login-time start
hoarder doctor                          # checks protocols, paths, service and conflicting tools
```

`setup --apply` writes a file of Hoarder's own next to your compositor config (`hoarder.lua` for a Lua `hyprland.lua`, `hoarder.conf` for hyprlang, an include for Sway and niri) and prints the one line to add, such as `require("hoarder")`. It never edits your main config. The bindings:

| Key | Command | Does |
|---|---|---|
| `Print` | `hoarder shot` | Capture a Region, Window or Output |
| `Shift+Print` | `hoarder shot all` | Capture every output |
| `Alt+Print` | `hoarder pick` | Pick a colour |
| `Super+V` | `hoarder picker` | Search History |

### Taking a shot

Drag a rectangle and release: done. Or, on the Overlay:

| Input | Action |
|---|---|
| Click a window | Capture the Window |
| `O`, or click the desktop | Capture the Output |
| `A` | Capture All Outputs |
| `Enter` with no selection | Capture the last Region again |
| `Space` held while dragging | Move the selection instead of resizing it |
| Shift+release | Enter Refine to adjust the selection before capturing |
| Arrows / Shift+Arrows | Nudge an edge by 1 / 10 px |
| `Alt` held | Show the Loupe (it also appears when the pointer rests) |
| `W` | Cycle through the windows under the pointer |
| `C` | Toggle the cursor |
| `Esc`, right-click | Cancel |

```sh
hoarder shot window --list              # list windows; `--list TITLE` captures one from its own buffer
hoarder shot output -o - | swappy -f -  # to stdout
hoarder shot --delay 3 --format webp
```

### The Picker

`Enter` restores a Clip and pastes it into the window you came from; `Shift+Enter` restores it as plain text. `Ctrl+P` pins, `Del` deletes, `Ctrl+1`–`9` restores quickly, `Alt` reveals Concealed Clips. Filter with `img:`, `txt:`, `file:`, `pin:` and `app:<id>`.

### From the terminal

```sh
echo hi | hoarder copy                  # every wl-copy flag works, plus --sensitive
hoarder paste -t image > clip.png       # every wl-paste flag works
hoarder history list | head
hoarder history restore 42
hoarder pause 5m                        # stop recording into History; `hoarder resume`
hoarder clear                           # empty the clipboard and keep it empty
hoarder status
```

Install `hoarder-wl-compat` and existing `wl-copy`/`wl-paste` integrations (Neovim, tmux, shell scripts) use Hoarder without changes.

## Configuration

Everything lives in `~/.config/hoarder/config.toml`, every key is optional, and changes apply live.

```sh
hoarder config set screenshot.format avif
hoarder config set history.encrypt true   # age, with the key in your Secret Service keyring
hoarder shot --set screenshot.save=false  # any key, for one run
```

Every key is described in **[docs/config.md](docs/config.md)**. Themes follow the [Zenith](https://github.com/CPT-Dawn/Zenith) bar by default, or pick `midnight-bloom`, `tokyo-night`, `catppuccin-mocha` or `gruvbox-dark`.

## Privacy

Each copy is classified before anything is stored:

- **Secret**: marked by a password manager (`x-kde-passwordManagerHint`). Never read, stored or kept alive.
- **Concealed**: copied in an app on `privacy.exclude_apps` (KeePassXC, Bitwarden, 1Password, Proton Pass, Secrets by default), or matching a secret pattern (private keys, cloud and API tokens, JWTs, card numbers). Locked memory only, masked, gone after two minutes.
- **Normal**: stored in `$XDG_DATA_HOME/hoarder/`, readable only by you, optionally encrypted.

## For developers

Front ends talk to the Daemon over a versioned JSON-lines socket; see [docs/ipc.md](docs/ipc.md). Rust clients can depend on `hoarder-ipc` by git, pinned to a release tag. Design notes are in [docs/PLAN.md](docs/PLAN.md), decisions in [docs/adr/](docs/adr/), and vocabulary in [GLOSSARY.md](GLOSSARY.md).

```sh
cargo test --workspace   # needs sway, grim and wl-clipboard on PATH
```

The headless-Sway test harness and CI are described in [docs/testing.md](docs/testing.md). See [CONTRIBUTING.md](CONTRIBUTING.md) before opening a PR.

## License

[MIT](LICENSE)
