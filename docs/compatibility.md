# Compatibility

What works on which compositor. Hoarder captures through the `ext-image-copy-capture` and `wlr-screencopy` protocols and never through the portal ([ADR-0002](adr/0002-compositor-scope-ext-protocols-no-portal.md)); `hoarder doctor` lists the protocols your compositor offers.

| Compositor | Capture | Clipboard and History | Window picking | `setup --apply` |
| --- | :-: | :-: | :-: | :-: |
| Hyprland | ✓ | ✓ | ✓ | ✓ |
| Sway 1.11+ | ✓ | ✓ | ✓ | ✓ |
| niri | ✓ | ✓ | Region and Output only | ✓ |
| river, labwc, COSMIC | ✓ | ✓ | Region and Output only | bind manually |
| Wayfire | ✓ (wlr-screencopy) | ✓ | Region and Output only | bind manually |
| KDE Plasma (KWin) | ✗ KWin only offers capture through the portal | ✓ | — | bind manually |
| GNOME | ✗ | ✗ | — | — |

"Bind manually" means `hoarder setup <wm>` has no file format for that compositor: bind the four commands from [Keybindings](keybindings.md#compositor-bindings) in its config.

[Back to the README](../README.md)
