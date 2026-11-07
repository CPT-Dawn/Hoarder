# Hoarder

A Wayland-native tool that captures screenshots and keeps the clipboard alive, with a searchable history, for a single desktop user.

## Language

### Clipboard

**Selection**:
One of the two Wayland clipboard slots: the **Clipboard Selection** (Ctrl+C / Ctrl+V) or the **Primary Selection** (highlight / middle-click).
_Avoid_: using "clipboard" to mean both slots

**Offer**:
The set of MIME representations a source app advertises for a Selection; the data only exists while someone serves it.
_Avoid_: copy, payload

**Persistence**:
Hoarder keeping the current Selection pasteable after the app that set it has exited.
_Avoid_: clip-persist, keep-alive

**Clip**:
One recorded Selection change: all of its stored representations plus metadata, as kept in the History.
_Avoid_: item, entry, record

**History**:
The ordered, bounded collection of Clips the user can browse and restore.
_Avoid_: log, ring, database

### Capture

**Capture**:
One screenshot operation that yields a single image of a region, window or output.
_Avoid_: snap, screengrab, shot (except as the CLI verb)

**Overlay**:
The full-screen selection surface shown while the user chooses what to Capture.
_Avoid_: selector, slurp UI

**Frozen Frame**:
The native-resolution copy of one output taken before the Overlay appears; the Overlay shows it and the Capture is cut from it.
_Avoid_: freeze, snapshot, background

**Target**:
What a Capture covers: a **Region** (user-drawn rectangle), a **Window**, an **Output** (one monitor), or **All Outputs**.
_Avoid_: mode, area, screen (for Output)

### System

**Daemon**:
The single long-running Hoarder process that provides Persistence and owns the History.
_Avoid_: server, service, backend

**Front end**:
Anything that talks to the Daemon to show or act on Clips: the CLI, the Picker, the Zenith module, third-party scripts.
_Avoid_: client (ambiguous with Wayland clients)

**Picker**:
Hoarder's own popup for browsing, searching and restoring Clips.
_Avoid_: menu, launcher, clipboard manager window

**Zenith module**:
The `clipboard` module inside the Zenith bar: a Front end showing Daemon state and recent Clips in a popover.
_Avoid_: applet, widget, bar plugin

**Compat shims**:
Optional `wl-copy` / `wl-paste` commands that accept wl-clipboard's flags and forward to Hoarder.
_Avoid_: aliases, wrappers
