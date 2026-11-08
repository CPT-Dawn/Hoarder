<div align="center">

# Hoarder

**Pixel-exact screenshots and a clipboard that never forgets, for Wayland.**

One fast Rust binary replaces `grim`, `slurp`, `grimblast`/`Hyprshot`, `hyprpicker`,<br>
`wl-clipboard`, `wl-clip-persist` and `cliphist`.

[![CI](https://github.com/CPT-Dawn/Hoarder/actions/workflows/ci.yml/badge.svg)](https://github.com/CPT-Dawn/Hoarder/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/CPT-Dawn/Hoarder?color=8b5cf6)](https://github.com/CPT-Dawn/Hoarder/releases/latest)
[![AUR](https://img.shields.io/aur/version/hoarder-bin?label=AUR&color=1793d1)](https://aur.archlinux.org/packages/hoarder-bin)
[![License: MIT](https://img.shields.io/github/license/CPT-Dawn/Hoarder?color=22c55e)](LICENSE)
[![Stars](https://img.shields.io/github/stars/CPT-Dawn/Hoarder?style=flat&color=f59e0b)](https://github.com/CPT-Dawn/Hoarder/stargazers)

[Install](#-install) · [Quickstart](#-quickstart) · [Features](#-features) · [Docs](docs/config.md) · [Set up with an agent](#-set-up-with-an-ai-agent)

</div>

<!-- Demo GIFs: docs/media/shot.gif, docs/media/picker.gif -->

```sh
paru -S hoarder-bin && systemctl --user enable --now hoarder && hoarder setup hyprland --apply
```

Press <kbd>Print</kbd> to take a shot. Press <kbd>Super</kbd>+<kbd>V</kbd> to get back anything you copied.

---

## Contents

- [Why Hoarder](#-why-hoarder)
- [Features](#-features)
  - [Capture](#capture)
  - [Persistence](#persistence)
  - [History and the Picker](#history-and-the-picker)
  - [Colour picker](#colour-picker)
- [Install](#-install)
- [Quickstart](#-quickstart)
- [Usage](#-usage)
  - [Keybindings](#keybindings)
  - [Taking a shot](#taking-a-shot)
  - [The Picker](#the-picker)
  - [From the terminal](#from-the-terminal)
- [Configuration](#-configuration)
- [Privacy](#-privacy)
- [Compatibility](#-compatibility)
- [Set up with an AI agent](#-set-up-with-an-ai-agent)
- [FAQ](#-faq)
- [For developers](#-for-developers)
- [Status](#-status)
- [License](#-license)

---

## ✦ Why Hoarder

A typical Wayland setup glues together seven tools to do two jobs. The screenshots come out blurry at fractional scaling, the clipboard empties when an app closes, and clipboard managers that re-offer every copy break pasting in Firefox, GTK apps and IntelliJ.

Hoarder does both jobs in one binary, and does them properly:

| | The usual stack | Hoarder |
|---|---|---|
| Screenshot tools | `grim` + `slurp` + `grimblast` | `hoarder shot` |
| Colour picker | `hyprpicker` | `hoarder pick` |
| Clipboard CLI | `wl-copy` / `wl-paste` | `hoarder copy` / `paste`, flag-compatible |
| Keep copies alive | `wl-clip-persist` | the Daemon, without breaking apps |
| History | `cliphist` + a launcher | a built-in, themed Picker |
| Fractional scaling | resampled, soft | native buffers, pixel-exact |
| Passwords | stored like anything else | never stored |

---

## ✦ Features

### Capture

- **Freeze, then select.** Every output freezes at its native resolution, so you pick a **Region**, **Window**, **Output** or **All Outputs** in one gesture, without the screen moving under you.
- **Pixel-exact.** Images are cut straight from the compositor's buffers and never resampled: crisp at fractional scaling, correct on 10-bit outputs, no cursor or selection border leaking in.
- **Precise.** A Loupe for pixel-level aim, arrow-key nudging, a Refine step to adjust before capturing, and one key to repeat the last Region.
- **Done when you let go.** The shot lands on the clipboard as lossless PNG, is saved as PNG, WebP, JPEG, AVIF or QOI, and is announced with a notification.

### Persistence

Close the app you copied from and the copy is still there. A small, event-driven Daemon takes over the clipboard **only after the source app is gone**, so it never breaks Firefox, GTK, IntelliJ, GIMP or Discord the way tools that re-offer every copy do.

### History and the Picker

Every copy, text, image or file, goes into a searchable History. Press <kbd>Super</kbd>+<kbd>V</kbd> for a fast, themed Picker: type to search, <kbd>Enter</kbd> to paste it straight into the window you came from. Pin favourites, filter by type or app, and optionally encrypt History at rest.

### Colour picker

`hoarder pick` freezes the screen, shows the Loupe, and copies the colour under the pointer.

---

## ✦ Install

**Arch Linux** (AUR):

```sh
paru -S hoarder-bin          # or yay, or makepkg
```

| Package | What you get |
|---|---|
| `hoarder-bin` | Prebuilt binary (recommended) |
| `hoarder` | Built from the release source |
| `hoarder-git` | Built from the latest commit |
| `hoarder-wl-compat` | `wl-copy` / `wl-paste` provided by Hoarder, in place of wl-clipboard |

Every package ships the systemd user unit, man pages, bash/zsh/fish completions and the default config (`/usr/share/doc/hoarder/config.toml`).

**Any other distro:** grab a tarball from [Releases](https://github.com/CPT-Dawn/Hoarder/releases/latest), or build it:

```sh
cargo install --locked --git https://github.com/CPT-Dawn/Hoarder --tag v1.0.1 hoarder
```

> [!NOTE]
> Hoarder is not published to crates.io.

---

## ✦ Quickstart

Three commands, under a minute:

```sh
systemctl --user enable --now hoarder   # 1. start the Daemon (or: hoarder service enable)
hoarder setup hyprland --apply          # 2. keybindings + start at login (or: sway, niri)
hoarder doctor                          # 3. check everything; each problem comes with its fix
```

`setup --apply` writes a file of Hoarder's own next to your compositor config and prints the **one line** to add to it, such as `require("hoarder")` for a Lua `hyprland.lua` or a `source` line for hyprlang. It **never edits your main config**.

Running niri, river, labwc or another compositor? Run `hoarder setup <wm>` without `--apply` to print the bindings, or bind the commands below yourself.

---

## ✦ Usage

### Keybindings

| Key | Command | Does |
|---|---|---|
| <kbd>Print</kbd> | `hoarder shot` | Capture a Region, Window or Output |
| <kbd>Shift</kbd>+<kbd>Print</kbd> | `hoarder shot all` | Capture every output |
| <kbd>Alt</kbd>+<kbd>Print</kbd> | `hoarder pick` | Pick a colour |
| <kbd>Super</kbd>+<kbd>V</kbd> | `hoarder picker` | Search History |

### Taking a shot

Drag a rectangle and release. That's it. On the Overlay you can also:

| Input | Action |
|---|---|
| Click a window | Capture the Window |
| <kbd>O</kbd>, or click the desktop | Capture the Output |
| <kbd>A</kbd> | Capture All Outputs |
| <kbd>Enter</kbd> with no selection | Capture the last Region again |
| <kbd>Space</kbd> held while dragging | Move the selection instead of resizing it |
| <kbd>Shift</kbd>+release | Enter Refine to adjust before capturing |
| Arrows / <kbd>Shift</kbd>+Arrows | Nudge an edge by 1 / 10 px |
| <kbd>Alt</kbd> held | Show the Loupe (it also appears when the pointer rests) |
| <kbd>W</kbd> | Cycle through the windows under the pointer |
| <kbd>C</kbd> | Toggle the cursor |
| <kbd>Esc</kbd>, right-click | Cancel |

From scripts:

```sh
hoarder shot window --list              # list windows; `--list TITLE` captures one from its own buffer
hoarder shot output -o - | swappy -f -  # pipe to an editor
hoarder shot --delay 3 --format webp    # countdown, then capture as WebP
hoarder shot last                       # the last Region again, no Overlay
```

### The Picker

| Key | Action |
|---|---|
| Type | Search |
| <kbd>Enter</kbd> | Restore the Clip and paste it into the window you came from |
| <kbd>Shift</kbd>+<kbd>Enter</kbd> | Restore it as plain text |
| <kbd>Ctrl</kbd>+<kbd>1</kbd>–<kbd>9</kbd> | Restore quickly |
| <kbd>Ctrl</kbd>+<kbd>P</kbd> | Pin / unpin |
| <kbd>Del</kbd> | Delete |
| <kbd>Alt</kbd> | Reveal Concealed Clips |
| <kbd>Esc</kbd> | Close |

Filter with `img:`, `txt:`, `file:`, `pin:` and `app:<id>`.

### From the terminal

```sh
echo hi | hoarder copy                  # every wl-copy flag works, plus --sensitive
hoarder paste -t image > clip.png       # every wl-paste flag works
hoarder history list | head
hoarder history restore 42
hoarder pause 5m                        # stop recording into History; `hoarder resume` to undo
hoarder clear                           # empty the clipboard and keep it empty
hoarder status
```

Install `hoarder-wl-compat` and everything that already calls `wl-copy`/`wl-paste` (Neovim, tmux, shell scripts) uses Hoarder with no changes.

---

## ✦ Configuration

Everything lives in `~/.config/hoarder/config.toml`. Every key is optional and changes apply live, no restart needed.

```sh
hoarder config set screenshot.format avif
hoarder config set history.encrypt true   # age, with the key in your Secret Service keyring
hoarder shot --set screenshot.save=false  # override any key for one run
```

Every key is documented in **[docs/config.md](docs/config.md)**.

**Themes:** Hoarder follows the [Zenith](https://github.com/CPT-Dawn/Zenith) bar by default, or choose `midnight-bloom`, `tokyo-night`, `catppuccin-mocha` or `gruvbox-dark`.

---

## ✦ Privacy

Each copy is classified **before** anything is stored:

| Class | What it is | What Hoarder does |
|---|---|---|
| **Secret** | Marked by a password manager (`x-kde-passwordManagerHint`) | Never read, stored or kept alive |
| **Concealed** | Copied in an app on `privacy.exclude_apps` (KeePassXC, Bitwarden, 1Password, Proton Pass, Secrets by default), or matching a secret pattern: private keys, cloud and API tokens, JWTs, card numbers | Locked memory only, masked in the Picker, gone after two minutes, never written to disk |
| **Normal** | Everything else | Stored in `$XDG_DATA_HOME/hoarder/`, readable only by you, optionally encrypted |

---

## ✦ Compatibility

| Compositor | Capture | Clipboard and History | Window picking | `setup --apply` |
|---|:-:|:-:|:-:|:-:|
| Hyprland | ✓ | ✓ | ✓ | ✓ |
| Sway 1.11+ | ✓ | ✓ | ✓ | ✓ |
| niri | ✓ | ✓ | Region and Output only | ✓ |
| river, labwc, COSMIC | ✓ | ✓ | Region and Output only | bind manually |
| Wayfire | ✓ (wlr-screencopy) | ✓ | Region and Output only | bind manually |
| KDE Plasma (KWin) | ✗ KWin only offers capture through the portal | ✓ | — | bind manually |
| GNOME | ✗ | ✗ | — | — |

---

## ✦ Set up with an AI agent

Hoarder is designed to be set up non-interactively. Point your coding agent at this section, or paste it in.

<details>
<summary><b>Instructions for agents</b></summary>

Goal: install Hoarder, start its Daemon, add keybindings, and verify. Every step is idempotent.

1. **Check the environment.** `echo $XDG_SESSION_TYPE` must print `wayland`. Detect the compositor from `$XDG_CURRENT_DESKTOP` (or `$HYPRLAND_INSTANCE_SIGNATURE` / `$SWAYSOCK` / `$NIRI_SOCKET`). Stop on GNOME: it is unsupported. On KDE Plasma only the clipboard features work.
2. **Install.**
   - Arch-based: `paru -S --needed hoarder-bin` (or `yay`). Add `hoarder-wl-compat` only if the user wants Hoarder to replace `wl-clipboard`; it conflicts with that package.
   - Other distros: `cargo install --locked --git https://github.com/CPT-Dawn/Hoarder --tag v1.0.1 hoarder`, or the tarball from the latest GitHub release.
   - Confirm with `hoarder --version`.
3. **Start the Daemon.** `systemctl --user enable --now hoarder` (equivalent: `hoarder service enable`).
4. **Keybindings.** For `hyprland`, `sway` or `niri`: run `hoarder setup <wm> --apply`. It writes Hoarder's own file next to the compositor config and **prints one include line**. Add that exact line to the user's main compositor config if it is not already there; do not write the bindings into the main config yourself. For other compositors, bind the four commands from [Keybindings](#keybindings) in their config.
5. **Remove conflicts.** If `wl-clip-persist` or a `cliphist` watcher (`wl-paste --watch cliphist store`) is started from the compositor config, ask the user before removing those lines: they duplicate Hoarder and `wl-clip-persist` breaks pasting in some apps.
6. **Verify.** Run `hoarder doctor`. Exit code 0 means done. On exit code 1, each reported problem includes its fix; apply it and re-run.
7. **Reload** the compositor config (`hyprctl reload`, `swaymsg reload`, or `niri msg action load-config-file`) and tell the user: <kbd>Print</kbd> for a screenshot, <kbd>Super</kbd>+<kbd>V</kbd> for History.

Do not edit `~/.config/hoarder/config.toml` unless the user asks; every key has a sensible default. Use `hoarder config set <key> <value>` for changes, and see [docs/config.md](docs/config.md) for the keys.

</details>

---

## ✦ FAQ

<details>
<summary><b>Does it work with my existing <code>wl-copy</code> / <code>wl-paste</code> scripts?</b></summary>

Yes. `hoarder copy` and `hoarder paste` accept every wl-clipboard flag. Install `hoarder-wl-compat` and the `wl-copy` / `wl-paste` commands themselves come from Hoarder.
</details>

<details>
<summary><b>Why not just keep wl-clip-persist?</b></summary>

It re-offers every copy the moment it happens, which breaks pasting in Firefox, GTK apps, IntelliJ, GIMP and Discord. Hoarder steps in only after the source app has gone. `hoarder doctor` will tell you if it is still running.
</details>

<details>
<summary><b>Will my passwords end up in History?</b></summary>

No. Copies marked by a password manager are never read. Copies from password-manager apps, or that look like keys or tokens, stay in locked memory for two minutes and never touch disk. See [Privacy](#-privacy).
</details>

<details>
<summary><b>Something isn't working.</b></summary>

Run `hoarder doctor`: it checks the compositor, protocols, paths, service and conflicting tools, and prints a fix for every problem. If that doesn't solve it, [open an issue](https://github.com/CPT-Dawn/Hoarder/issues/new/choose) with its output.
</details>

---

## ✦ For developers

Front ends talk to the Daemon over a versioned JSON-lines socket, documented in [docs/ipc.md](docs/ipc.md). Rust clients can depend on the `hoarder-ipc` crate by git, pinned to a release tag; the [Zenith](https://github.com/CPT-Dawn/Zenith) bar's clipboard module is built on it.

```sh
cargo test --workspace   # needs sway, grim and wl-clipboard on PATH
```

| Read | For |
|---|---|
| [docs/testing.md](docs/testing.md) | The headless-Sway test harness and CI |
| [docs/PLAN.md](docs/PLAN.md) | Design notes |
| [docs/adr/](docs/adr/) | Architecture decisions |
| [GLOSSARY.md](GLOSSARY.md) | Vocabulary (Clip, Daemon, Overlay, Refine, …) |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Before you open a PR |

---

## ✦ Status

Hoarder v1.0 is the finished product and the project is in maintenance mode: bugs are fixed as they are reported, and new features are considered when users ask for them. [Open an issue](https://github.com/CPT-Dawn/Hoarder/issues/new/choose) for either.

## ✦ License

[MIT](LICENSE). If Hoarder saves you a few tools, a ⭐ helps others find it.
