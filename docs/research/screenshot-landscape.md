# Screenshot landscape on Wayland (researched 2026-11-07)

Verified against local binaries (grim 1.5.0, slurp 1.5.0, Hyprland 0.57.1, wayland-protocols 1.50), crates.io sources, GitHub issue trackers. **[unverified]** marks inference or single-source claims.

## Existing tools

| Tool | Shape | Capture path / notes |
|---|---|---|
| grim 1.5.0 | C one-shot CLI | wlr-screencopy + ext-image-copy-capture (output + toplevel `-T`), `-l` PNG level, `-s` scale, `-c` cursor. Moved to gitlab.freedesktop.org/emersion/grim |
| slurp 1.5.0 | C selector, prints geometry | layer-shell + cursor-shape; **no** fractional-scale/viewporter |
| grimblast / Hyprshot | shell pipelines (`hyprctl`+jq → slurp → grim → wl-copy → notify-send) | freeze = `hyprpicker -r -z & sleep 0.2`; Hyprshot uses `sleep 1` + `pgrep` loop |
| hyprpicker / wayfreeze | freeze overlays | wayfreeze needs `--inverse-transform` for rotated outputs |
| wayshot 1.6 / libwayshot 0.9 | Rust | screencopy + ext capture, optional dmabuf, libwaysip selector, wl-clipboard-rs; resamples to max scale |
| Satty / swappy | annotators (GTK windows, not layer-shell) | read PNG on stdin |
| Flameshot 14 | C++/Qt | portal Screenshot API; asks to pick a monitor |
| Newcomers (Rust) | wlr-utils (egui overlay, dmabuf), wayscriber (annotation overlay), bullseye, clipt, foamshot, hyprshot-rs | small/young |

## Protocol support

| | wlr-screencopy | ext-image-copy-capture (output) | ext toplevel source | ext-foreign-toplevel-list | layer-shell |
|---|---|---|---|---|---|
| Hyprland 0.57.1 | Y | Y | Y | Y | Y |
| Sway 1.12 | Y | Y | Y | Y | Y |
| niri 26.04 | Y | main only ([#4554](https://github.com/niri-wm/niri/pull/4554)) | main only | Y | Y |
| river 0.5 | Y | Y | Y | Y | Y |
| COSMIC 1.0 | N | Y | Y | Y | Y |
| KWin 6.x | N | **N, rejected** ([bug 513785](https://bugs.kde.org/show_bug.cgi?id=513785), "portals only") | N | N | Y |
| Mutter | N | N | N | N | N |

Hyprland extras: `hyprland-toplevel-export-v1`, `hyprland_workspace_image_capture_source_v1`, `hyprland_toplevel_mapping_v1` (toplevel handle → hyprctl address). wlr-screencopy is deprecated in favour of ext-image-copy-capture.

Occluded windows: Hyprland toplevel capture renders the window itself, so occlusion is fine; hidden-workspace windows may be **stale** due to `misc:render_unfocused_fps` [partially verified]. Other compositors [unverified].

## Pixel fidelity

- **SHM** is enough for stills (4K XRGB8888 ≈ 33 MB, 8K ≈ 133 MB); dmabuf only pays for live preview/recording.
- **Formats**: must handle 10-bit `XRGB2101010`/`XBGR2101010`; 8-bit-only tools break ([wayshot #181](https://github.com/waycrate/wayshot/issues/181), [hyprpicker #16](https://github.com/hyprwm/hyprpicker/issues/16)). HDR captures look washed out ([Hyprshot #100](https://github.com/Gustash/Hyprshot/issues/100)); true fidelity needs 16-bit PNG + cICP [png crate cICP support unverified].
- **Scaling**: grim upsamples low-scale outputs and has an unmerged geometry-space fix ([contrib #69](https://github.com/hyprwm/contrib/issues/69)). Rule: capture each output at native buffer size, crop in buffer pixels, **never resample**. Cross-output regions with mixed scales need an explicit policy.
- **Transforms**: frames arrive in buffer orientation (+ possible y-invert); apply lossless 90° rotations ([grim #73](https://github.com/emersion/grim/issues/73)).
- **Cursor**: ext capture offers `paint_cursors` and a separate cursor session → cursor can be kept as a separate layer and toggled after the fact.
- **Encoding**: `png` 0.18 with fdeflate (`Compression::Fast`) is very fast and lossless; QOI only as an internal cache; `image` crate is heavier than needed.
- **Freeze done wrong**: existing tools freeze by showing a screencopy, then re-capture the overlay → double resample, cursor/border/animation artifacts, races. Done right: capture once, display those buffers 1:1 via viewporter, crop from the originals.

## Selection UX mechanics

- One layer-shell surface per output (Overlay layer, all anchors, exclusive zone -1, keyboard Exclusive); `wp_fractional_scale_v1` + `wp_viewporter` for crispness; `wp_cursor_shape_v1`.
- Frozen frame on a subsurface; UI chrome on a small damage-tracked ARGB surface above it, so the compositor does the GPU blend.
- Global selection state in logical coordinates so drags cross monitors ([slurp #178](https://github.com/emersion/slurp/issues/178), [#185](https://github.com/emersion/slurp/issues/185)).
- Window geometry is not in any generic protocol: Hyprland `j/clients` IPC, Sway `get_tree`.

## Pain points (by GitHub comment volume)

| # | Pain point | Evidence |
|---|---|---|
| 1 | Blur at fractional/mixed scale, worse with freeze | [contrib #69](https://github.com/hyprwm/contrib/issues/69), [flameshot #564](https://github.com/flameshot-org/flameshot/issues/564), [slurp #137](https://github.com/emersion/slurp/issues/137) |
| 2 | Overlay artifacts in the shot (cursor, border, black edge) | [contrib #60](https://github.com/hyprwm/contrib/issues/60), [#115](https://github.com/hyprwm/contrib/issues/115), [Hyprshot #64](https://github.com/Gustash/Hyprshot/issues/64) |
| 3 | Multi-monitor geometry errors | [flameshot #2364](https://github.com/flameshot-org/flameshot/issues/2364), [contrib #137](https://github.com/hyprwm/contrib/issues/137) |
| 4 | Clipboard lost when the screenshot process exits | [flameshot #635](https://github.com/flameshot-org/flameshot/issues/635), [Satty #19](https://github.com/Satty-org/Satty/issues/19), [wayshot #109](https://github.com/waycrate/wayshot/issues/109) |
| 5 | Slow start / high CPU | [Satty #132](https://github.com/Satty-org/Satty/issues/132), [contrib #134](https://github.com/hyprwm/contrib/issues/134) |
| 6 | Freeze broken or crashy | [contrib #118](https://github.com/hyprwm/contrib/issues/118), [hyprpicker #17](https://github.com/hyprwm/hyprpicker/issues/17) |
| 7 | HDR / 10-bit wrong | [Hyprland #7379](https://github.com/hyprwm/Hyprland/issues/7379), Hyprshot #100 |
| 8 | Annotation needs a separate window | [Satty #118](https://github.com/Satty-org/Satty/issues/118) |
| 9 | Window capture missing/buggy | [wayshot #379](https://github.com/waycrate/wayshot/issues/379), [Hyprshot #55](https://github.com/Gustash/Hyprshot/issues/55) |

## Rust stack (crates.io, 2026-11-07)

`wayland-client` 0.31.15; `wayland-protocols` 0.32.13 (`staging` feature has ext image capture + foreign-toplevel-list); `wayland-protocols-wlr` 0.3.12; `smithay-client-toolkit` 0.21.1 (layer-shell, seat, shm, output; no fractional-scale helper); Hyprland protocols must be vendored + generated with `wayland-scanner`. Renderers: `tiny-skia` 0.12 (CPU, draws into SHM) recommended; `vello`/`wgpu`/`femtovg`/`iced_layershell` heavier. Encoding: `png` 0.18.
