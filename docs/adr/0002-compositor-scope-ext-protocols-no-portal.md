# Compositor scope: standard ext/wlr protocols, no xdg-desktop-portal

Hoarder targets compositors that expose the standard `ext-*` Wayland protocols (image capture, data control), falling back to their `wlr-*` predecessors, and adds Hyprland-specific extras (e.g. window geometry for snapping) through Hyprland IPC. GNOME, and portal-based capture/clipboard in general, are out of scope for v1: the portal path adds permission dialogs and an extra process hop, and gives up the direct, pixel-exact buffer access that is the product's core promise.

## Consequences

- **KWin (Plasma 6.5+)** gets the clipboard side (ext-data-control) but **not** Capture: KWin deliberately refuses ext-image-copy-capture and wlr-screencopy ("portals only", KDE bug 513785).
- **COSMIC** has no wlr-screencopy, so ext-image-copy-capture is the primary capture path and wlr-screencopy is only the fallback (niri stable, older Sway).
- **Wayfire** lacks ext-data-control, which is why the wlr-data-control fallback stays.
