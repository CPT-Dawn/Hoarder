# Clipboard landscape on Wayland (researched 2026-11-07)

Sources: cloned repos, issue trackers via `gh`, crates.io, wayland.app, KDE/Mozilla Bugzilla, GNOME GitLab. Unverified items listed at the end.

## Existing tools

| Tool | Lang | Model | Storage | Notes |
|---|---|---|---|---|
| wl-clipboard 2.3.0 | C | per-invocation; `wl-copy` forks to serve; `wl-paste --watch` fork+execs per change with `CLIPBOARD_STATE` | — | ext-data-control + `--sensitive` since 2.3.0; focus-stealing fallback without data-control |
| wl-clipboard-rs 0.9.5 | Rust | lib + CLI, in-memory sources, `Watcher` API | — | ext + wlr data-control |
| cliphist 0.7 | Go | via `--watch` | bbolt, 750 items / 5 MB each, byte dedup over last 100 | honors `CLIPBOARD_STATE=sensitive` |
| clipse 1.2 | Go | TUI + `--watch` | JSON + image files | excludes apps via `hyprctl activewindow` |
| clipvault 1.3 | Rust | via `--watch` | SQLite | regex ignore, foreign-toplevel app ignore |
| stash 0.5 | Rust | own watch daemon + wl-copy/wl-paste drop-ins | SQLite, FNV-1a dedup, optional age encryption | KDE hint, regex filter, app exclude, TTL that also clears live clipboard |
| Ringboard 0.17 | Rust | server + watchers + GUIs | custom mmap/io_uring format (~1M entries) | |
| wl-clip-persist 0.5 | Rust | daemon | memory | see Persistence |
| CopyQ 17, GPaste, Klipper, COSMIC applet | various | daemons | own / SQLite | GPaste and Klipper honor the KDE hint |

## Protocol support

| Compositor | ext-data-control-v1 | zwlr-data-control v2 |
|---|---|---|
| Hyprland 0.57.1, Sway 1.11, niri 26.04, COSMIC, labwc, river | Y | Y |
| KWin 6.5+ | Y | **removed** ([KDE 513822](https://bugs.kde.org/show_bug.cgi?id=513822)) |
| Wayfire 0.9 | N | Y |
| Mutter/GNOME | N | N |

ext-data-control-v1 is in wayland-protocols staging; wlr-data-control is deprecated. Core `wl_data_device` needs focus, hence fallback focus stealing ([wl-clipboard #90](https://github.com/bugaevc/wl-clipboard/issues/90)). Mutter persists natively by eagerly copying **one** preferred type (images ≤200 MB, text ≤4 MB) — a good conservative model.

## Persistence mechanics

The selection is an offer; bytes flow only on paste. When the source exits the selection is cleared and it is too late to read, so persistence must read **eagerly** at copy time. A "selection substitute" protocol was rejected ([wl-clipboard #65](https://github.com/bugaevc/wl-clipboard/issues/65)).

wl-clip-persist reads every MIME type and re-offers; its breakage list is the spec of what not to do: Firefox every-other copy fails ([#17](https://github.com/Linus789/wl-clip-persist/issues/17)), GTK/Emacs paste hangs ([#22](https://github.com/Linus789/wl-clip-persist/issues/22)), IntelliJ cut ([#15](https://github.com/Linus789/wl-clip-persist/issues/15)), Inkscape crash ([#7](https://github.com/Linus789/wl-clip-persist/issues/7)), GIMP 3 images ([#18](https://github.com/Linus789/wl-clip-persist/issues/18)), Discord ([#20](https://github.com/Linus789/wl-clip-persist/issues/20)), primary selection breaks GTK selection ([#3](https://github.com/Linus789/wl-clip-persist/issues/3)), encoding/offer-order bugs ([#9](https://github.com/Linus789/wl-clip-persist/issues/9)).

Takeaways: preserve MIME order; cap size and time; skip app-private types (`SAVE_TARGETS`, `x-moz-*`, app SVG); allow/deny lists per type; never re-offer sensitive data; clipboard only (not primary) by default; ignore self-caused events.

## Privacy

- `x-kde-passwordManagerHint: secret` is set by KeePassXC, Bitwarden desktop, 1Password (forum-reported), Firefox `about:logins`, `wl-copy --sensitive`, GPaste. **Browser extensions cannot set it.**
- Honored natively by Klipper, CopyQ, GPaste, stash; cliphist/clipse/clipman only via `CLIPBOARD_STATE`.
- Source app is **not** knowable from data-control; heuristic is focused window at copy time (foreign-toplevel or Hyprland IPC), which misfires for CLI copies and focus races.
- Prior art: age/libsodium encryption at rest, regex secret filters, TTL expiry that also clears the live clipboard. Nobody documents mlock/zeroize.

## Pain points

| # | Pain point | Evidence |
|---|---|---|
| 1 | Clipboard lost when source app closes | reason wl-clip-persist exists |
| 2 | Persistence daemons break apps | wl-clip-persist issues above |
| 3 | Passwords land in history | [cliphist #58](https://github.com/sentriz/cliphist/issues/58) |
| 4 | Focus stealing without data-control | [wl-clipboard #90](https://github.com/bugaevc/wl-clipboard/issues/90), [#268](https://github.com/bugaevc/wl-clipboard/issues/268) |
| 5 | Protocol churn (KWin dropped wlr) | KDE 513822 |
| 6 | Browser image copies stored as HTML/junk | [cliphist #28](https://github.com/sentriz/cliphist/issues/28), [#53](https://github.com/sentriz/cliphist/issues/53) |
| 7 | `--watch` process-per-change CPU | [wl-clipboard #137](https://github.com/bugaevc/wl-clipboard/issues/137) |
| 8 | Paste-once / auto-clear defeated by watchers | [cliphist #25](https://github.com/sentriz/cliphist/issues/25) |
| 9 | File copies (`text/uri-list`, gnome-copied-files) | [wl-clipboard #244](https://github.com/bugaevc/wl-clipboard/issues/244) |
| 10 | Primary selection side effects, charset issues | wl-clip-persist #3, #9 |

## Rust crates (2026-11-07)

`wayland-protocols` 0.32.13 (`staging` → `ext::data_control::v1`); `wayland-protocols-wlr` 0.3.12; `wl-clipboard-rs` 0.9.5; SCTK 0.21.1 has no data-control. Storage: `rusqlite` 0.41, `redb` 4.5, `fjall` 3.1; **avoid `sled`** (stale). Crypto/memory: `age` 0.12, `chacha20poly1305` 0.11, `zeroize`, `secrecy`, `memsec`.

## Unverified

Proton Pass hint support; current 1Password release state; CopyQ hint handling on Wayland; full primary-selection matrix; Reddit not mined.
