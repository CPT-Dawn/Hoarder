# Daemon API: versioned JSON-lines over a Unix socket, public and stable

All Front ends, including Hoarder's own CLI and Picker and the Zenith `clipboard` module, talk to the Daemon through one versioned JSON-lines protocol on `$XDG_RUNTIME_DIR/hoarder/hoarder.sock` (request/response plus a `subscribe` event stream). It is documented as a public contract so third-party bars and launchers can integrate without patching Hoarder. JSON-lines was chosen over D-Bus (extra dependency, heavier tooling) and over a binary protocol (not scriptable with `socat`/`jq`); throughput is irrelevant at clipboard event rates.

## Consequences

- Zenith gains a native `clipboard` module that is just another Front end; Zenith has no IPC of its own, so the dependency points one way (Zenith → Hoarder socket) and Zenith must tolerate the Daemon being absent.
