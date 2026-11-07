# hoarder-ipc

The contract between [Hoarder](https://github.com/CPT-Dawn/Hoarder), a Wayland clipboard daemon, and its Front ends: request, response and event types for its JSON-lines socket API, a framing codec and a blocking client. Use it to build a status-bar module, a launcher integration or any other tool that lists, restores or watches clipboard History.

The protocol is documented, with `socat`/`jq` examples, in [docs/ipc.md](https://github.com/CPT-Dawn/Hoarder/blob/main/docs/ipc.md). You can speak it without this crate; this crate saves you copying the types.

```rust,no_run
use hoarder_ipc::{Connection, Event};

fn main() -> std::io::Result<()> {
    // $XDG_RUNTIME_DIR/hoarder/hoarder.sock; fails if no Daemon is running.
    let mut daemon = Connection::connect()?;
    let status = daemon.status()?.map_err(std::io::Error::other)?;
    println!("hoarder {} is {:?}", status.daemon, status.recording.state);

    daemon.subscribe()?.map_err(std::io::Error::other)?;
    loop {
        match daemon.next_event()? {
            Event::ClipAdded { clip } => println!("{}: {}", clip.id, clip.preview),
            Event::ClipRemoved { clip } => println!("{clip} is gone"),
            Event::State(recording) => println!("now {:?}", recording.state),
            _ => {} // newer events, and selection changes
        }
    }
}
```

Linux only (Unix sockets and `SCM_RIGHTS` fd passing via `rustix`). `Connection` is blocking; an async Front end can use the types with its own socket and `encode` for framing. `cargo doc -p hoarder-ipc --open` in your project shows the API.

## Getting it

`hoarder-ipc` is not on crates.io. Depend on it by git, pinned to a Hoarder release tag:

```toml
[dependencies]
hoarder-ipc = { git = "https://github.com/CPT-Dawn/Hoarder", tag = "v1.0.0" }
```

The crate is versioned with Hoarder: tag `vX.Y.Z` holds `hoarder-ipc` X.Y.Z. Move the tag forward when you want newer ops or fields.

## Compatibility

The wire protocol carries a version, `v`, now `1`. That is the promise, not the crate's version:

- **Additive protocol changes** (a new op, event, field, error code or enum value) keep `v: 1`. A client pinned to an older tag keeps working against a newer Daemon: unknown fields are ignored, and unknown events, error codes, states and owners decode as `Unknown`. The enums that grow (`Op`, `Event`, `ErrorCode`, `State`, `Owner`) are `#[non_exhaustive]`, so match them with a wildcard arm.
- **Breaking protocol changes** bump `v`. `Op::Hello` tells you which versions a Daemon speaks.
- Response structs (`Status`, `ClipSummary`, …) may gain public fields in any release. Reading them is covered by this policy; building them with struct literals (to implement a Daemon) is not.

The minimum supported Rust version is in `Cargo.toml` (`rust-version`).

## License

MIT
