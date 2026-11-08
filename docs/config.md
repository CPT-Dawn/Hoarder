# Configuration reference

Hoarder reads `~/.config/hoarder/config.toml` (`$XDG_CONFIG_HOME/hoarder/config.toml`; `hoarder config path` prints it). Every key is optional: a missing key, table or file keeps the default. Unknown keys are errors, so a typo never fails silently.

Changes apply live: the Daemon reloads the file when it is saved, and a half-saved or invalid file keeps the last good configuration until it is fixed.

```sh
hoarder config get                      # the effective configuration
hoarder config get history.max_clips    # one key, or a table: `hoarder config get history`
hoarder config set screenshot.format webp   # edits the file, keeping every comment
hoarder config reset screenshot.format  # back to the default; no key rewrites the whole file
hoarder config check                    # PATH:LINE:COLUMN: message, exit 2 on problems
hoarder config edit                     # $VISUAL / $EDITOR, then check
hoarder shot --set screenshot.save=false    # override any key for one run
```

The commented default file ships at `/usr/share/doc/hoarder/config.toml`.

## `[theme]`

The colours and fonts of the Overlay and the Picker.

| Key | Default | Values | Meaning |
|---|---|---|---|
| `theme.name` | `"zenith"` | `zenith`, `midnight-bloom`, `tokyo-night`, `catppuccin-mocha`, `gruvbox-dark`, `custom` | `zenith` follows the Zenith bar's active theme and `style.css` live, and falls back to Midnight Bloom without Zenith. `custom` is Midnight Bloom, meant as a base for the overrides below. |
| `theme.accent` | `""` | colour | Selection border, highlights. |
| `theme.surface` | `""` | colour | Panel and pill backgrounds. |
| `theme.text` | `""` | colour | Primary text. |
| `theme.muted` | `""` | colour | Secondary text. |
| `theme.dim` | `""` | colour | Hints and disabled text. |
| `theme.radius` | `""` | `0`–`64` | Corner radius, in logical pixels. |
| `theme.font` | `""` | font stack | UI font, like CSS `font-family`: `"Inter, sans-serif"`. The first installed family wins. |
| `theme.mono_font` | `""` | font stack | Font for code-like Clips in the Picker. |

Every override beats every theme value, and `""` keeps the theme's. Colours are `"#rgb"`, `"#rgba"`, `"#rrggbb"`, `"#rrggbbaa"` `"rgb(r g b / a)"` or `"rgba(r, g, b, a)"`.

## `[screenshot]`

What `hoarder shot` does with a Capture.

| Key | Default | Values | Meaning |
|---|---|---|---|
| `screenshot.dir` | `"$XDG_PICTURES_DIR/Screenshots"` | path | Where Captures are saved. `~`, `$VAR` and `${VAR}` expand; `$XDG_PICTURES_DIR` falls back to `user-dirs.dirs`, then `~/Pictures`. |
| `screenshot.name` | `"Hoarder_{%Y-%m-%d_%H-%M-%S}"` | template | File name, without extension. Tokens: `{%…}` (strftime of the Capture time), `{target}` (`region`, `window`, `output`, `all`), `{app}` (the window's app, for window Captures), `{w}`, `{h}` (size in physical pixels). |
| `screenshot.format` | `"png"` | `png`, `webp`, `jpeg`, `avif`, `qoi` | Format of the saved file. The clipboard always gets lossless PNG. |
| `screenshot.save` | `true` | bool | Save to `dir`. `--no-save` for one run. |
| `screenshot.copy` | `true` | bool | Put the Capture on the clipboard. `--no-copy` for one run. |
| `screenshot.notify` | `true` | bool | Show a notification with actions. `--no-notify` for one run. |
| `screenshot.confirm` | `false` | bool | Enter Refine after every drag, not only after Shift+release. `--confirm` for one run. |
| `screenshot.freeze` | `true` | bool | Freeze the screen before selecting. `false` selects over the live desktop (`--live`). |
| `screenshot.cursor` | `false` | bool | Include the pointer. `C` toggles it in the Overlay; `--cursor` for one run. |
| `screenshot.window_source` | `"frame"` | `frame`, `toplevel` | A window Capture is cut from the Frozen Frame (`frame`: what you saw, popups and overlapping windows included) or taken from the window's own buffer (`toplevel`: only the window, even if covered). |
| `screenshot.sound` | `false` | bool | Play the freedesktop `screen-capture` sound. |
| `screenshot.flash` | `false` | bool | Flash the screen after the freeze; never part of the Capture. |
| `screenshot.on_capture` | `""` | shell command | Run with `sh -c` after each saved or copied image, without waiting for it. Environment: `HOARDER_PATH` (empty when not saved), `HOARDER_WIDTH`, `HOARDER_HEIGHT`, `HOARDER_TARGET`. Its output goes to stderr; a failure is logged, never fatal. |

### Format options

| Key | Default | Values | Meaning |
|---|---|---|---|
| `screenshot.png.level` | `"best"` | `fast`, `best`, `0`–`9` | PNG compression for saved files. Always lossless. |
| `screenshot.webp.lossless` | `true` | bool | Lossless WebP (byte-exact). `false` encodes lossy WebP at `quality`. |
| `screenshot.webp.quality` | `90` | `0`–`100` | Lossy WebP quality; ignored when `lossless = true`. |
| `screenshot.jpeg.quality` | `92` | `1`–`100` | JPEG quality. |
| `screenshot.avif.quality` | `80` | `0`–`100` | AVIF quality. |
| `screenshot.avif.speed` | `6` | `1`–`10` | AVIF encoder speed: higher is faster and larger. |

QOI has no options: it is always lossless.

## `[capture]`

How a Target spanning outputs of different scales or bit depths is cut. Hoarder never resamples otherwise.

| Key | Default | Values | Meaning |
|---|---|---|---|
| `capture.mixed_scale` | `"upscale"` | `upscale`, `clamp`, `split` | `upscale`: one image at the highest scale, lower-scale parts upscaled by nearest neighbour, with a "mixed scale" badge on the notification. `clamp`: a Region cannot leave the output where the drag started. `split`: one file per output. |
| `capture.depth` | `"auto"` | `"auto"`, `8` | `auto`: 8-bit outputs give 8-bit PNG, 10-bit outputs give 16-bit PNG. `8`: always 8-bit. HDR outputs get an "HDR" badge; there is no tone mapping. |

## `[pick]`

| Key | Default | Values | Meaning |
|---|---|---|---|
| `pick.format` | `"hex"` | `hex`, `rgb`, `hsl` | How `hoarder pick` writes the colour. `--format` for one run. |

## `[clipboard]`

Persistence: the Daemon reads every copy at once but takes ownership of the Selection (Takeover) only after the source app is gone, so live apps are never disturbed.

| Key | Default | Values | Meaning |
|---|---|---|---|
| `clipboard.persist` | `true` | bool | Keep the clipboard pasteable after the app that set it exits. |
| `clipboard.deny_types` | `[]` | MIME types | Extra MIME types never read or stored, on top of the built-in deny list. A trailing `*` matches a prefix: `"application/x-foo-*"`. |
| `clipboard.max_offer_mb` | `64` | integer | The most the Daemon reads from one copy; larger representations are skipped. |
| `clipboard.no_takeover_apps` | `[]` | app ids | Apps (focused when the copy is made) whose copies are never kept alive after they exit: the escape hatch for an app that misbehaves when Hoarder serves its Selection. They are still recorded in History. |
| `clipboard.on_clip` | `""` | shell command | Run with `sh -c` for each Normal copy, with the copy on stdin, like `wl-paste --watch`. Environment: `HOARDER_MIME` (the type on stdin), `HOARDER_TYPES` (every offered type, one per line), `HOARDER_APP` (the app it was copied in, when known). Never run for Secret or Concealed copies. |
| `clipboard.primary.persist` | `false` | bool | Persistence for the primary selection (highlight and middle-click). |
| `clipboard.primary.history` | `false` | bool | Record the primary selection in History. |

## `[history]`

The private, searchable store of Clips in `$XDG_DATA_HOME/hoarder/` (directory mode 0700, files 0600). Retention runs on every new Clip and hourly, evicting the oldest unpinned Clips first. For every limit, `0` means none. Pinned Clips count toward the limits but are never evicted.

| Key | Default | Values | Meaning |
|---|---|---|---|
| `history.enabled` | `true` | bool | Record Normal copies. With `false`, Persistence still works. |
| `history.max_clips` | `1000` | integer | The most Clips kept. |
| `history.max_age_days` | `30` | integer | Unpinned Clips older than this are removed. |
| `history.max_clip_mb` | `64` | integer | Clips larger than this are not stored. |
| `history.max_total_mb` | `1024` | integer | The most disk space History uses. |
| `history.encrypt` | `false` | bool | Encrypt History at rest with age, under a key kept in the Secret Service keyring (GNOME Keyring, KeePassXC, …). Toggling it migrates History both ways and scrubs the old form. Times, pin state, source app, kind, MIME types and sizes stay readable. Without an unlocked keyring, History is unavailable and nothing is recorded in plain text. |

## `[privacy]`

Every copy gets a Sensitivity:

- **Secret**: the source marked it with `x-kde-passwordManagerHint` (as `hoarder copy --sensitive` does). Nothing is read, stored or kept alive.
- **Concealed**: copied in an excluded app, or matching a secret pattern. Kept in locked memory only, masked in the Picker (hold `Alt` to reveal), and cleared after `concealed_ttl_secs`. Never written to disk.
- **Normal**: everything else, stored in History.

| Key | Default | Values | Meaning |
|---|---|---|---|
| `privacy.exclude_apps` | `["org.keepassxc.KeePassXC", "Bitwarden", "1Password", "proton-pass", "org.gnome.World.Secrets"]` | app ids | Copies made while one of these apps is focused are Concealed. |
| `privacy.patterns` | `true` | bool | The built-in secret patterns: PEM private keys, AWS access keys, GCP service-account JSON, GitHub tokens, Slack tokens, Stripe live keys, JWTs and Luhn-valid card numbers. |
| `privacy.extra_patterns` | `[]` | regexes | More patterns that make text Concealed, in Rust regex syntax. An invalid one is skipped with a warning. |
| `privacy.concealed_ttl_secs` | `120` | integer | How long a Concealed Clip lives. If Hoarder still owns the clipboard then, it clears it. |

## `[picker]`

The History panel opened by `hoarder picker` (usually bound to Super+V).

| Key | Default | Values | Meaning |
|---|---|---|---|
| `picker.auto_paste` | `true` | bool | After a Restore, paste into the window that was focused before, by sending Ctrl+V. |
| `picker.terminal_apps` | `["kitty", "foot", "Alacritty", "org.wezfurlong.wezterm", "com.mitchellh.ghostty", "org.kde.konsole"]` | app ids | Apps that get Ctrl+Shift+V instead. |
