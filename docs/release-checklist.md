# Release checklist

The manual pass from PLAN.md §13, for what the headless test session cannot cover. Copy this file for each release, fill it in on real hardware, and attach it to the GitHub release. Mark each row `pass`, `fail` (open an issue, link it) or `n/a`.

Release: `v_._._` · Date: · Tester: · `hoarder --version`: · Compositor and version:

## Hyprland

| Check | Result | Notes |
|---|---|---|
| Hover highlights the window under the pointer; click captures it | | |
| `W` cycles through stacked windows | | |
| `shot window --list` lists windows, including ones on hidden workspaces | | |
| `shot window --list TITLE` captures a window on a hidden workspace from its own buffer | | |
| `window_source = "toplevel"` captures a covered window without what covers it | | |
| `setup hyprland --apply` bindings work after a re-login, and the Daemon starts with the session | | |

## Takeover and Restore, per app

For each app: copy, close the app, paste elsewhere (Takeover). Then restore the Clip from the Picker and paste into the app (Restore, auto-paste). Note anything that breaks in the app while it is still open.

| App | Text: Takeover | Image or rich: Takeover | Restore and auto-paste | No breakage while open |
|---|---|---|---|---|
| Firefox | | | | |
| Chromium | | | | |
| Electron (Discord or VS Code) | | | | |
| GTK4 app | | | | |
| Qt6 app | | | | |
| IntelliJ (cut and paste) | | | | |
| LibreOffice | | | | |
| Inkscape | | | | |
| GIMP 3 | | | | |
| foot or kitty (Ctrl+Shift+V) | | | | |
| Password manager copy is Secret: not in History, not kept alive | | | | |

## Outputs

Verify with a known pattern (a 1px checkerboard or a pixel-grid image) that a Region Capture matches the screen pixel for pixel.

| Output | Region | Window | Output | All Outputs | Loupe colour exact |
|---|---|---|---|---|---|
| Scale 1.25 | | | | | |
| Scale 1.5 | | | | | |
| Scale 2 | | | | | |
| Rotated (90° or 270°) | | | | | |
| 10-bit (16-bit PNG out, `depth = "auto"`) | | | | | |
| HDR (badge shown) | | | | | |
| Two outputs of different scales: `upscale`, `clamp`, `split` | | | | | |

## Install

| Check | Result | Notes |
|---|---|---|
| `paru -S hoarder-bin` on a clean system, then `hoarder doctor` reports no problems | | |
| `hoarder` (source) builds and installs | | |
| `hoarder-wl-compat` replaces wl-clipboard; `wl-copy`/`wl-paste` work in Neovim and a shell | | |
| Man pages and zsh completions load | | |
