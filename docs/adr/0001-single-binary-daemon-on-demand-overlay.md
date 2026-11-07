# One binary: clipboard Daemon plus on-demand capture Overlay

Hoarder ships as a single `hoarder` binary. Only the clipboard side runs persistently (the Daemon, a systemd user service), because Persistence and History need something alive to serve Offers after source apps exit. Captures run as short-lived processes that exit when done and hand the image to the Daemon to serve, so the screenshot UI costs zero memory while idle. Front ends reach the Daemon over a Unix socket in `$XDG_RUNTIME_DIR`.

## Considered Options

- **Always-warm single process (Overlay preloaded)**: rejected; Rust cold start is a few milliseconds, so the warm Overlay buys nothing while costing resident GPU/SHM memory all day.
- **Separate binaries per feature (grim/slurp/wl-copy style)**: rejected; it reproduces the process-pipeline races and inconsistent config that Hoarder exists to remove.
