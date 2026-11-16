# Keybindings

The keys `hoarder setup <wm>` binds, and every key inside the Overlay, Refine and the Picker. The key handling lives in `crates/hoarder-ui/src/overlay.rs`, `select.rs` and `picker/mod.rs`.

## Compositor bindings

`hoarder setup hyprland|sway|niri --apply` writes these to a file of Hoarder's own and prints the one line that includes it. On other compositors, run `hoarder setup <wm>` without `--apply` to print them, or bind the commands yourself.

| Key | Command | Does |
| --- | --- | --- |
| <kbd>Print</kbd> | `hoarder shot` | Capture a Region, Window or Output |
| <kbd>Shift</kbd>+<kbd>Print</kbd> | `hoarder shot all` | Capture every output |
| <kbd>Alt</kbd>+<kbd>Print</kbd> | `hoarder pick` | Pick a colour |
| <kbd>Super</kbd>+<kbd>V</kbd> | `hoarder picker` | Search History |

## The Overlay

Drag a rectangle and let go: that is a Region. The rest is optional.

| Input | Action |
| --- | --- |
| Click a window | Capture the Window |
| <kbd>O</kbd>, or click the desktop | Capture the Output |
| <kbd>A</kbd> | Capture All Outputs |
| <kbd>Enter</kbd> with no selection | Capture the last Region again |
| <kbd>Space</kbd> held while dragging | Move the selection instead of resizing it |
| <kbd>Shift</kbd>+release | Enter Refine to adjust before capturing |
| Arrows / <kbd>Shift</kbd>+Arrows | Nudge an edge by 1 / 10 px |
| <kbd>Alt</kbd> held | Show the Loupe (it also appears when the pointer rests) |
| <kbd>W</kbd> | Cycle through the windows under the pointer |
| <kbd>C</kbd> | Toggle the cursor in the Capture |
| <kbd>Esc</kbd>, right-click | Cancel |

## Refine

Refine keeps the selection on screen so you can adjust it. `--confirm` (or `screenshot.confirm = true`) enters it after every drag.

| Input | Action |
| --- | --- |
| Drag an edge or corner | Resize |
| Arrows / <kbd>Shift</kbd>+Arrows | Nudge the active edge by 1 / 10 px |
| <kbd>Tab</kbd> / <kbd>Shift</kbd>+<kbd>Tab</kbd> | Move between the Capture, Cursor and Cancel tools |
| <kbd>Enter</kbd> | Run the chosen tool |
| <kbd>Esc</kbd>, right-click | Cancel |

## The Picker

| Key | Action |
| --- | --- |
| Type | Search |
| <kbd>Enter</kbd> | Restore the Clip and paste it into the window you came from |
| <kbd>Shift</kbd>+<kbd>Enter</kbd> | Restore it as plain text |
| <kbd>Ctrl</kbd>+<kbd>1</kbd>–<kbd>9</kbd> | Restore the Clip in that row |
| <kbd>Up</kbd> / <kbd>Down</kbd>, <kbd>Page Up</kbd> / <kbd>Page Down</kbd> | Move the selection |
| <kbd>Shift</kbd>+Arrows | Scroll the preview |
| <kbd>Ctrl</kbd>+<kbd>P</kbd> | Pin or unpin |
| <kbd>Del</kbd> | Delete |
| <kbd>Backspace</kbd> / <kbd>Ctrl</kbd>+<kbd>Backspace</kbd> / <kbd>Ctrl</kbd>+<kbd>U</kbd> | Delete a character / a word / the whole query |
| <kbd>Alt</kbd> held | Reveal Concealed Clips |
| <kbd>Esc</kbd> | Close |

Filter the list by starting the query with `img:`, `txt:`, `file:`, `pin:` or `app:<id>`. Pinned Clips sit at the top until you search, when the list ranks by match. Held arrows, <kbd>Page Up</kbd>/<kbd>Page Down</kbd>, <kbd>Del</kbd> and <kbd>Backspace</kbd> repeat at your keyboard's repeat rate.

The paste after <kbd>Enter</kbd> sends <kbd>Ctrl</kbd>+<kbd>V</kbd>, or <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>V</kbd> in terminals; `picker.auto_paste` and `picker.terminal_apps` in [Configuration](config.md) change that.

[Back to the README](../README.md)
