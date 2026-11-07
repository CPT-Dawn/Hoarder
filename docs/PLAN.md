# Hoarder — implementation plan

> Status: agreed design, pre-code (2026-11-07). Vocabulary follows [`GLOSSARY.md`](../GLOSSARY.md); decisions are recorded in [`docs/adr/`](adr/); prior-art research is in [`docs/research/`](research/).

Hoarder replaces grim, slurp, grimblast/Hyprshot, hyprpicker, wl-clipboard, wl-clip-persist and cliphist with one Rust-native, resource-light tool for Wayland: pixel-exact Captures that land on the clipboard, a clipboard that survives the app that set it, and a private, searchable History.

## 1. Principles

1. **Pixels are sacred.** A Capture is cut from the compositor's own buffer. No resampling, no re-capture of our own UI, no lossy step unless the user picks a lossy format for the saved file. ([ADR-0003](adr/0003-freeze-first-capture-never-resample.md))
2. **Never break the app you copied from.** Hoarder stays out of the way while the source app is alive and only takes over when it leaves. ([ADR-0005](adr/0005-takeover-only-when-selection-vanishes.md))
3. **Secrets never touch disk.** ([ADR-0006](adr/0006-three-sensitivity-tiers.md))
4. **Zero cost at rest.** The Daemon is event-driven, with no polling. The Overlay and Picker exist only while they are on screen.
5. **One gesture for the common case, precision one key away.**
6. **Everything is configurable from the terminal**, and every Front end speaks one public API. ([ADR-0004](adr/0004-json-lines-socket-public-api.md))

## 2. Scope

**Compositors** ([ADR-0002](adr/0002-compositor-scope-ext-protocols-no-portal.md)):

| | Capture | Clipboard | Window snapping |
|---|---|---|---|
| Hyprland | ext-image-copy-capture | ext-data-control | Hyprland IPC |
| Sway 1.11+ | ext-image-copy-capture | ext-data-control | Sway IPC |
| niri, river, labwc | ext, or wlr-screencopy as fallback | ext-data-control | none (region/output only) |
| COSMIC | ext-image-copy-capture | ext-data-control | none |
| KWin 6.5+ | **not supported** (KWin is portal-only) | ext-data-control | — |
| Wayfire | wlr-screencopy | wlr-data-control | none |
| GNOME | out of scope | out of scope | — |

**Non-goals** ([ADR-0008](adr/0008-non-goals.md)): screen recording, scrolling capture, sync, History import, pinning a Capture on screen, paste queue.

## 3. Architecture

```
                       ┌──────────────────────── hoarder (one binary) ────────────────────────┐
  compositor           │                                                                      │
  ──────────           │  hoarder daemon  (systemd --user, always on, ~0% CPU idle)           │
  ext-data-control ◄──►│   ├─ SelectionWatcher ─► Classifier ─► Reader ─► Store (SQLite)      │
  Hyprland/Sway IPC ◄─►│   ├─ Takeover (serves Offers after the source exits)                │
                       │   ├─ Retention / expiry timers                                       │
                       │   └─ IPC server  $XDG_RUNTIME_DIR/hoarder/hoarder.sock  (JSON-lines) │
                       │              ▲            ▲              ▲              ▲            │
                       │   hoarder shot     hoarder picker   hoarder CLI     Zenith module    │
  ext-image-copy ◄────►│   (Overlay, exits) (Picker, exits)  (one-shot)    (in zenith-bar)    │
  layer-shell    ◄────►│                                                                      │
                       └──────────────────────────────────────────────────────────────────────┘
```

- **Daemon**: a single-threaded `calloop` event loop. The Wayland connection, the IPC socket, inotify for config reload and timers are all event sources on it. Blocking work (pipe reads, PNG encoding, SQLite writes) runs on a small worker pool and returns results to the loop. ([ADR-0001](adr/0001-single-binary-daemon-on-demand-overlay.md))
- **Capture process** (`hoarder shot`): connects, freezes the outputs, runs the Overlay, encodes, and hands the PNG to the Daemon over the socket (`offer` request, fd passing for large payloads). It then exits. If the Daemon is down, it forks a minimal serving child, the way `wl-copy` does, and warns that History is off.
- **Picker process** (`hoarder picker`): a short-lived layer-shell window that reads History from the Daemon.

### Workspace

| Crate | Responsibility |
|---|---|
| `hoarder` | Binary, CLI (clap), subcommand wiring, notifications (zbus) |
| `hoarder-ipc` | Request/response/event types plus a framing codec. Versioned and **published to crates.io** for Zenith and third parties. |
| `hoarder-wayland` | Protocol bindings (`wayland-protocols` `staging`, `-wlr`, vendored Hyprland XML), capture, data-control, the virtual keyboard, and the window-geometry providers (Hyprland/Sway IPC) |
| `hoarder-store` | SQLite schema and migrations, blobs, dedup, retention, FTS search |
| `hoarder-ui` | Shared SCTK layer-shell plumbing, tiny-skia drawing, cosmic-text, widgets (list, text field, loupe, toast), theme tokens ([ADR-0007](adr/0007-custom-ui-stack-no-toolkit.md)) |
| `hoarder-config` | Typed schema (serde), defaults, `toml_edit` writes, validation with line numbers, live reload |

Rust 2024 edition; MSRV is the current stable minus two releases.

## 4. Capture

### 4.1 Pipeline

1. **Bind** the best protocols available, in this order:
   - `ext_image_copy_capture_manager_v1` with `ext_output_image_capture_source_manager_v1`, falling back to `zwlr_screencopy_manager_v1`;
   - `ext_foreign_toplevel_list_v1` and `ext_foreign_toplevel_image_capture_source_manager_v1`;
   - `hyprland_toplevel_mapping_v1` if it is present.
2. **Freeze.** Capture every output in parallel into SHM at its native buffer size, in whatever format the compositor advertises: `XRGB8888`, `ARGB8888`, `XBGR2101010`, `XRGB2101010`, and so on. Capture the cursor through a separate cursor session. Record the transform and y-invert for each output.
3. **Normalise without loss.** Apply the 90° transforms and y-invert by moving pixels, never by resampling. 10-bit data is kept at 16 bits per channel internally.
4. **Overlay** (§4.2) yields a **Target**: a Region in logical coordinates mapped to buffer pixels per output, or a Window, an Output, or All Outputs.
5. **Cut.** Crop in buffer pixels. A Target that spans outputs follows `capture.mixed_scale` ([§4.4](#44-mixed-scale-and-depth)).
6. **Composite the cursor** only if it is toggled on.
7. **Deliver**, in parallel:
   - Clipboard: encode PNG with `Compression::Fast` and hand it to the Daemon. Target: on the clipboard within 150 ms for 4K.
   - File: encode in the configured format and level on a worker thread, then write the file using the name template.
   - Notification: a thumbnail plus Open / Edit / Show in folder / Delete.
   - Hook: run `on_capture` with `HOARDER_PATH`, `HOARDER_WIDTH`, `HOARDER_HEIGHT` and `HOARDER_TARGET` set.
   - History: the Daemon records the Capture as an image Clip.

With `--live`, Hoarder skips the freeze: the Overlay draws over the live desktop, then hides, and capture happens after one frame callback.

**Window Targets**: a click crops the Frozen Frame (what you saw). `Shift+click`, or `screenshot.window_source = "toplevel"`, captures the window's own buffer through the ext toplevel source instead. `hoarder shot window --list` always uses the toplevel source.

### 4.2 Overlay

**Surfaces.** One `zwlr_layer_surface_v1` per output: Overlay layer, anchored on all edges, exclusive zone -1, keyboard Exclusive. On each one, the Frozen Frame is a `wl_subsurface` sized with `wp_viewporter`, so one buffer pixel maps to one physical pixel. The UI chrome is a separate ARGB surface on top, redrawn only in damaged rectangles, so the 4K frame is never re-blended on the CPU. The surfaces use `wp_fractional_scale_v1` for crisp chrome and `wp_cursor_shape_v1` for the crosshair. Selection state is global, in logical coordinates, so a drag can cross monitors.

**State machine.**

```
Idle ──press──► Dragging ──release──► Capture
 │  ╲              │  Space held: move instead of resize
 │   ╲             └─ Shift+release / confirm=true ──► Refine ──Enter──► Capture
 │    click on window ──────────────────────────────────────────────────► Capture(Window)
 │    O / click on empty desktop ───────────────────────────────────────► Capture(Output)
 │    A ────────────────────────────────────────────────────────────────► Capture(All Outputs)
 │    Enter (no selection) ─────────────────────────────────────────────► Capture(last Region)
 └─ Esc / right-click ──► Cancel (exit 1, nothing copied)
```

**Keys.**

| Key | Action |
|---|---|
| Arrows / Shift+Arrows | Nudge the selection edge by 1 / 10 px |
| `Alt` (hold) | Show the Loupe immediately |
| `C` | Toggle the cursor layer |
| `O`, `A` | Capture the Output / All Outputs |
| `W` | Cycle through the windows under the pointer |
| `Tab` (in Refine) | Move between the Refine toolbar actions |

**Visual spec.**
- Outside the selection, the frame is dimmed to 45%.
- The selection has a 1px border in the accent colour and a size pill (`1280 × 720`, in physical pixels).
- The **Loupe** is an 8× grid of 15×15 px with a crosshair, coordinates and the hex colour. It appears after 300 ms of stillness or while `Alt` is held.
- Hovering a window gives it a tinted fill and a label with its icon and title.
- A hint bar (`drag · click window · O output · A all · C cursor · Esc`) fades out after 1.5 s. It is hidden after 20 uses, a count kept in `$XDG_STATE_HOME/hoarder/state.toml`.
- The Overlay fades in over 100 ms. There is no shutter sound and no flash unless they are enabled in the config.

**Snapping and window geometry.** A `WindowGeometry` provider trait has three implementations:
- `HyprlandIpc`: `j/clients` and `j/monitors`, with `hyprland_toplevel_mapping_v1` linking a toplevel to its address;
- `SwayIpc`: `get_tree`;
- `None`: hover highlighting is disabled. Region and Output capture still work, and `window --list` works through the toplevel source.

**Delay.** `--delay N` shows a small countdown ring on the output under the pointer and freezes at zero.

### 4.3 Colour picker (`hoarder pick`)

Same freeze and the same Overlay, with the Loupe always shown. A click copies the colour as `hex`, `rgb` or `hsl` (`--format`, or `pick.format`) and shows a swatch in a toast. 10-bit sources report the exact value.

### 4.4 Mixed scale and depth

- `capture.mixed_scale`:
  - `upscale` (default): render at the highest scale; lower-scale parts are upscaled by an integer or nearest-neighbour factor; the notification shows a "mixed scale" badge.
  - `clamp`: the Region cannot leave the output where the drag started.
  - `split`: one file per output.
- `capture.depth`:
  - `auto` (default): 8-bit outputs give 8-bit PNG, 10-bit outputs give 16-bit PNG.
  - `8`: always 8-bit.
  - HDR outputs get an "HDR" badge. Tone mapping is out of scope for v1.

### 4.5 Output formats

- **Clipboard:** always `image/png`, lossless, fast encoder.
- **File:** set by `screenshot.format`:

| Format | Options | Default |
|---|---|---|
| `png` | `level` 0–9 / `fast`/`best` | `best`, lossless |
| `webp` | `lossless`, `quality` | lossless |
| `jpeg` | `quality` | 92 |
| `avif` | `quality`, `speed` | 80 / 6 |
| `qoi` | — | lossless |

Files are named from `screenshot.dir` (default `$XDG_PICTURES_DIR/Screenshots`) and `screenshot.name` (default `Hoarder_{%Y-%m-%d_%H-%M-%S}`). Templates can also use `{target}`, `{app}`, `{w}` and `{h}`.

## 5. Clipboard

### 5.1 Selection lifecycle (Daemon)

```
selection(offer) ─► ignore if the offer is our own source
                 ─► Classifier ─► Secret?    ─► record nothing; don't Take over; mark the Selection Secret
                               ─► Concealed? ─► read into locked memory (Concealed Clip, 2 min TTL)
                               ─► Normal     ─► Reader ─► Store
selection(null)  ─► the previous Selection was Normal or Concealed, the Daemon is not Paused,
                    and the user did not ask for `hoarder clear`
                    ─► Takeover: set our own source, offering the stored representations
```

**Reader rules** ([ADR-0005](adr/0005-takeover-only-when-selection-vanishes.md)):
- Keep the source's MIME order.
- Skip the deny list: `SAVE_TARGETS`, `TARGETS`, `MULTIPLE`, `TIMESTAMP`, `x-moz-*`, `chromium/x-*`, `application/x-kde-*`, `image/x-inkscape-svg`, `application/x-qt-*`, plus anything in `clipboard.deny_types`.
- When several image types are offered, read only `image/png`, plus the source's first image type if it is different.
- Limits: 500 ms per type, 2 s per Offer, 64 MB per Offer. A partial read keeps the types that succeeded.
- Read `text/uri-list` and `x-special/gnome-copied-files`, and display them as file chips.
- Primary selection is ignored unless `clipboard.primary.persist` or `clipboard.primary.history` is set.

**Classifier** ([ADR-0006](adr/0006-three-sensitivity-tiers.md)):
- **Secret:** the Offer advertises `x-kde-passwordManagerHint`. This is checked from the MIME list alone, before any data is read; the hint's value is read only to confirm `secret`.
- **Concealed:** either the focused app_id at copy time is on `privacy.exclude_apps`, or the text matches a built-in or user pattern.
  - The focused app comes from the Hyprland `activewindowv2` event stream, the Sway focus event, or `ext-foreign-toplevel` activation state.
  - Built-in patterns: PEM private keys, AWS `AKIA…`, GCP service-account JSON, GitHub `gh[pousr]_…`, Slack `xox[abpr]-`, Stripe `sk_live_`, JWT, and Luhn-valid 13–19-digit card numbers.
- **Pause** (`hoarder pause [duration]`): nothing is recorded. Persistence and Takeover still work.

**Concealed handling:** buffers are allocated with `memsec` mlock and wrapped in `secrecy`, and zeroized when dropped. They are never written to SQLite. At TTL expiry, if Hoarder still owns the Selection, it clears it.

### 5.2 `hoarder copy` / `hoarder paste` and the Compat shims

`copy` accepts every wl-copy flag: `-p`, `-t TYPE`, `-n`, `-o`, `-f`, `-c`, `--sensitive`. With the Daemon running, `copy` sends the data over the socket and exits immediately; there is no lingering fork. `paste` accepts every wl-paste flag (`-p`, `-t`, `-n`, `-l`, `-w CMD`). `-w` runs inside the Daemon's event stream, so no extra process spins per change.

`hoarder-wl-compat` installs `/usr/bin/wl-copy` and `wl-paste`. They are argv[0]-dispatching symlinks to `hoarder`, and the package declares `conflicts=(wl-clipboard)` and `provides=(wl-clipboard)`.

### 5.3 Store

`$XDG_DATA_HOME/hoarder/`, with the directory at mode 0700 and files at 0600:

```sql
clips(id INTEGER PK, created_at, last_used_at, pinned BOOL, source_app TEXT NULL,
      kind TEXT /* text|image|files|other */, preview TEXT, size INTEGER, hash BLOB UNIQUE /* blake3 of canonical repr */)
reprs(clip_id FK, ord INTEGER, mime TEXT, inline BLOB NULL, blob_hash BLOB NULL)   -- inline ≤ 256 KB
thumbs(clip_id FK, png BLOB)                                                        -- 256 px, made on insert
clips_fts(preview)                                                                  -- FTS5 trigram
```

- Blobs larger than 256 KB go to `blobs/<blake3>`, content-addressed and reference-counted.
- **Dedup:** a repeat hash bumps `last_used_at`.
- **Retention:** 1,000 Clips, 30 days for unpinned Clips, 64 MB per Clip, 1 GB in total, evicting the oldest unpinned first. It runs on insert and on an hourly timer.
- **Encryption:** off by default. `history.encrypt = true` encrypts with age, using a key held in the Secret Service keyring (`oo7`).
- **Migrations:** the database stores a `user_version` and is upgraded forward only.

### 5.4 Restore and auto-paste

A Restore makes the Daemon own the Selection with that Clip's representations. If `picker.auto_paste` is on, the Picker then:
1. waits for its surface to unmap and focus to return to the previous window;
2. sends `Ctrl+V` through `zwp_virtual_keyboard_v1`, or `Ctrl+Shift+V` when the focused app_id is in `picker.terminal_apps` (kitty, foot, Alacritty, WezTerm, Ghostty, Konsole, and others).

The Zenith module only restores; it never auto-pastes.

## 6. Picker

```
┌───────────────────────────────────────────────────────────────────────┐
│ 🔍  search…                                       img: pin: app:      │
├──────────────────────────────┬────────────────────────────────────────┤
│ ▍ T  fn main() { let x = …   │  fn main() {                           │
│   ▣  [screenshot 1920×1080]  │      let x = hoard();                  │
│   T  https://github.com/…    │  }                                     │
│   ⧉  report.pdf, notes.md    │                                        │
│ 📌 T  ssh-ed25519 AAAA…        │                         firefox · 2m   │
│   •  ••••••••  (concealed)   │                                        │
├──────────────────────────────┴────────────────────────────────────────┤
│ ⏎ restore  ⇧⏎ plain  ^P pin  Del delete  ^1-9 quick           Esc     │
└───────────────────────────────────────────────────────────────────────┘
```

- **Layout:** a centered layer-shell surface on the focused output, about 720×480 logical px, keyboard Exclusive. It shows its first frame within 50 ms.
- **Search:** fuzzy match (nucleo) over previews, combined with FTS for long text. Filter prefixes `img:`, `txt:`, `file:`, `pin:` and `app:<id>`.
- **Preview pane:** text shown with a monospace font for code-like content; images at real pixel size, scrollable; files as chips with their paths.
- **Keys:**
  - `Enter`: restore.
  - `Shift+Enter`: restore as plain text.
  - `Ctrl+P`: pin or unpin.
  - `Del`: delete.
  - `Ctrl+1–9`: quick restore.
  - `Alt`: reveal Concealed Clips.
  - `Esc`: close.
- **v1.1 transforms** (`Ctrl+T`): trim, join lines, change case, encode or decode base64/URL/JSON.

## 7. Zenith module

This lives in the Zenith repo as a separate PR. It is a new `ModuleId::Clipboard` with the standard five files (`config.rs`, `model.rs`, `source.rs`, `view.rs`, `mod.rs`), and depends on the `hoarder-ipc` crate.

- **source.rs:** subscribes to the socket and publishes `{state: Active|Paused|Offline, recent: [ClipSummary; 10]}`. It reconnects with backoff and shows `Offline` without logging errors.
- **view.rs:**
  - The bar icon changes with state.
  - Left-click opens a popover (same pattern as `todo/view.rs`): the 10 most recent Clips with thumbnails; clicking one restores it.
  - The footer row reads "Search all…" and runs `hoarder picker`.
  - Right-click toggles Pause.
- **Styling:** `zb-clipboard*` classes and the `--zb-*` tokens. Theme docs are updated.

## 8. Theming

`[theme] name = "zenith"` (the default) or one of `midnight-bloom`, `tokyo-night`, `catppuccin-mocha`, `gruvbox-dark`, or `custom` (Midnight Bloom as a base for overrides). This mirrors Zenith's own `[theme]` table: a name plus override keys in one table.

- `zenith`: Hoarder rebuilds Zenith's cascade: the built-in palette named by Zenith's `[theme] name`, then Zenith's `[theme]` overrides (`accent`, `surface_opacity`, `module_radius`), then the `:root` `--zb-*` variables in `style.css` (simple `var()` references resolve; anything unparsable such as `color-mix()` is skipped). It reads them afresh for every Overlay and Picker, so a Zenith theme change shows on the next one. With no (or a broken) Zenith config it falls back to Midnight Bloom, Zenith's default.
- The other keys of `[theme]` override any token and beat every theme value: `accent`, `surface`, `text`, `muted`, `dim` (CSS colours: `#rgb[a]`, `#rrggbb[aa]`, `rgb()`/`rgba()`), `radius` (0–64 logical px), `font`, `mono_font` (font stacks, first installed family wins). `""` keeps the theme's value.

Token sources in Zenith: `accent` ← `--zb-accent`, `surface` ← `--zb-surface-color` at `--zb-surface-opacity`, `text`/`muted`/`dim` ← `--zb-text`/`--zb-text-muted`/`--zb-text-dim`, `radius` ← `--zb-module-radius`, `font` ← `--zb-font-family`, `mono_font` ← `--zb-mono`.

## 9. Configuration

`~/.config/hoarder/config.toml` reloads live. `hoarder config get|set|edit|reset|check|path`, and any setting can be overridden for one run with a flag.

```toml
[theme]
name = "zenith"
accent = ""               # "" keeps the theme's; or "#rrggbb", "rgb(r g b / a)"

[screenshot]
dir = "~/Pictures/Screenshots"
name = "Hoarder_{%Y-%m-%d_%H-%M-%S}"
format = "png"            # png | webp | jpeg | avif | qoi
save = true
copy = true
notify = true
confirm = false           # always enter Refine
freeze = true             # false = --live
cursor = false
window_source = "frame"   # frame | toplevel
mixed_scale = "upscale"   # upscale | clamp | split
depth = "auto"            # auto | 8
sound = false
flash = false
on_capture = ""           # shell hook

[screenshot.png]
level = "best"
[screenshot.webp]
lossless = true
quality = 90
[screenshot.jpeg]
quality = 92
[screenshot.avif]
quality = 80
speed = 6

[pick]
format = "hex"

[clipboard]
persist = true
deny_types = []
max_offer_mb = 64
no_takeover_apps = []     # escape hatch: app_ids whose copies are never taken over
on_clip = ""
[clipboard.primary]
persist = false
history = false

[history]
enabled = true
max_clips = 1000
max_age_days = 30
max_clip_mb = 64
max_total_mb = 1024
encrypt = false

[privacy]
exclude_apps = ["org.keepassxc.KeePassXC", "Bitwarden", "1Password", "proton-pass", "org.gnome.World.Secrets"]
patterns = true           # built-in secret patterns
extra_patterns = []
concealed_ttl_secs = 120

[picker]
auto_paste = true
terminal_apps = ["kitty", "foot", "Alacritty", "org.wezfurlong.wezterm", "com.mitchellh.ghostty", "org.kde.konsole"]
```

## 10. CLI

```
hoarder                         status, plus an offer to enable the service if it isn't running
hoarder shot [region|window|output|all|last] [--list [TITLE]] [--delay N] [--cursor] [--live]
             [--confirm] [--format F] [-o PATH|-] [--no-save] [--no-copy] [--no-notify]
hoarder pick [--format hex|rgb|hsl]
hoarder copy [wl-copy flags]    hoarder paste [wl-paste flags]
hoarder picker
hoarder history list|get|restore|delete|pin|unpin|clear [--json]
hoarder pause [DURATION]        hoarder resume       hoarder clear
hoarder status [--json]
hoarder daemon                  hoarder service enable|disable|status
hoarder config get|set|edit|reset|check|path
hoarder setup hyprland|sway|niri [--apply]
hoarder doctor
hoarder completions bash|zsh|fish
```

Exit codes: 0 on success, 1 when cancelled by the user, 2 for usage errors, 3 for unsupported compositor or protocol, 4 for I/O errors.

## 11. IPC API (v1)

One JSON object per line, sent over `$XDG_RUNTIME_DIR/hoarder/hoarder.sock`. The directory has mode 0700, and the peer's uid is checked with `SO_PEERCRED`.

```jsonc
→ {"v":1,"id":7,"op":"list","limit":10,"filter":"img:"}
← {"v":1,"id":7,"ok":[{"id":412,"kind":"image","preview":"1920×1080","app":"firefox","at":"…","pinned":false,"sensitivity":"normal"}]}
→ {"v":1,"id":8,"op":"subscribe"}
← {"v":1,"event":"clip_added","clip":{…}}
← {"v":1,"event":"state","state":"paused","until":"…"}
```

**Ops:** `hello` (version negotiation), `status`, `list`, `get` (the representation is streamed over a passed fd), `thumb`, `restore`, `offer` (Capture or `copy` data; fd passing), `delete`, `pin`, `unpin`, `clear`, `pause`, `resume`, `subscribe`, `reload`.

**Compatibility:** additive changes keep `v:1`; breaking changes bump `v` and the `hoarder-ipc` major version.

## 12. Budgets

| Metric | Budget |
|---|---|
| Daemon idle CPU | 0 wakeups/s (no timers except the hourly retention run) |
| Daemon RSS (idle, 1,000 Clips) | < 12 MB |
| Key press → Overlay visible (4K) | < 60 ms |
| Release → PNG on clipboard (4K) | < 150 ms |
| Picker first frame | < 50 ms |
| Stripped binary | < 8 MB |

The budgets are checked in CI with a benchmark job on the headless Sway session. A regression of more than 20% fails the job.

## 13. Testing

Tests go through as few seams as possible, and as high up as possible.

- **Primary seam: the `hoarder` binary as a black box in a headless Sway session** (`WLR_BACKENDS=headless`, `WLR_RENDERER=pixman`). A helper Wayland client:
  - paints known patterns on 8-bit, 10-bit, scaled and rotated outputs;
  - sets and serves Selections (including Secret ones), then exits on cue;
  - drives the Overlay and Picker through virtual pointer and virtual keyboard.

  The tests cover byte-exact Captures, Takeover, Sensitivity tiers, History, `copy`/`paste` flag parity with wl-clipboard, and IPC conformance. Overlay and Picker visuals are checked by capturing the headless screen through this same seam.
- **Secondary seams, three deep modules tested directly:**
  - **Classifier:** Selection metadata in, Sensitivity out.
  - **Retention policy:** Store state plus limits in, Clips to evict out.
  - **Config editing:** file plus `set` in, file out with comments preserved.
- **Manual release checklist:** Hyprland features (snapping, toplevel mapping, hidden workspaces), and the app matrix for Takeover and Restore (Firefox, Chromium, an Electron app such as Discord or VS Code, GTK4, Qt6, IntelliJ, LibreOffice, Inkscape, GIMP 3, foot/kitty), fractional scales 1.25/1.5/2, rotated output, 10-bit/HDR.

## 14. Milestones

> Work is tracked as vertical-slice tickets #57–#86 under spec #56, grouped into the GitHub milestones below; v1.1 stays in epic #50. The lists here describe scope, not ticket boundaries. M0's spikes were folded into #58 (fidelity), #61 (rendering budgets) and #72 (Takeover app matrix).

### M0: De-risking prototypes
1. **Takeover prototype:** a minimal data-control daemon implementing ADR-0005, run against the app matrix. It produces a compatibility report and confirms or revises the reader deny list.
2. **Fidelity spike:** freeze outputs at native size, shown 1:1 through a viewporter subsurface, on fractional, rotated and 10-bit outputs, with byte-exact crop verification.
3. **Rendering spike:** tiny-skia and cosmic-text on a layer surface, measuring the time to first frame and RSS against the budgets.

### M1: Capture (v0.1)
Workspace scaffold and CI · `hoarder-config` · protocol bindings and capture (ext plus wlr fallback) · Overlay surfaces and the state machine · Loupe, hints and visual spec · window geometry providers (Hyprland, Sway) · window toplevel capture and `--list` · mixed scale and depth · encoders and file naming · notifications and hooks · `hoarder pick` · `--delay` · Daemon-less clipboard fallback.

### M2: Clipboard and History (v0.2)
Daemon skeleton (calloop, service) · IPC server and `hoarder-ipc` crate · SelectionWatcher and Reader · Takeover · Classifier and Concealed memory · Pause/clear · Store (schema, blobs, dedup, FTS, retention) · encryption at rest (opt-in) · `copy`/`paste` with wl-clipboard flags · Compat shims · `hoarder history` CLI · Capture → History integration.

### M3: Picker and Zenith (v0.3)
hoarder-ui widgets (list, text field) · Picker layout and search · preview pane · keys and quick restore · auto-paste through the virtual keyboard · theming (`zenith` follower plus built-ins) · Zenith `clipboard` module PR.

### M4: Ship v1.0
`hoarder doctor` · `hoarder setup <wm>` · completions and man pages (clap_mangen) · systemd unit · README, docs site and GIF demos · CONTRIBUTING and issue templates · release workflow (git-cliff, crates.io, GitHub release artefacts) · AUR packages `hoarder`, `hoarder-bin`, `hoarder-git`, `hoarder-wl-compat` · release checklist pass.

### v1.1
In-place annotation in Refine (arrow, box, text, highlighter, numbered steps, blur/pixelate) · Picker transforms · OCR (optional `tesseract`) · QR decode · `text-input-v3` IME in the Picker.

## 15. Risks

| Risk | Mitigation |
|---|---|
| Takeover-on-vanish still breaks some app, or restores something an app deliberately cleared | M0 prototype with the app matrix; per-app `clipboard.no_takeover_apps` escape hatch; Secret/Concealed never restored |
| ext-image-copy-capture quirks differ between compositors | Fidelity spike; wlr fallback; `hoarder doctor` reports which path is used |
| Focused-app heuristic misfires (CLI copies, focus races) | Patterns and the Secret hint are the primary signals; app exclusion is defence in depth |
| Hidden-workspace windows come out stale on Hyprland | Documented; the window list shows a "may be stale" badge |
| Custom text input lacks IME | `text-input-v3` scheduled for v1.1; CJK users can still filter with ASCII prefixes |
| HDR captures look washed out | "HDR" badge; tone mapping tracked as future work |
