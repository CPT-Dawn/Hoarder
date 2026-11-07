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

Linux only (Unix sockets and `SCM_RIGHTS` fd passing via `rustix`). [`Connection`](https://docs.rs/hoarder-ipc/latest/hoarder_ipc/struct.Connection.html) is blocking; an async Front end can use the types with its own socket and [`encode`](https://docs.rs/hoarder-ipc/latest/hoarder_ipc/fn.encode.html) for framing.

## Versioning

The wire protocol carries a version, `v`, now `1`. The crate's version follows it:

- **Additive protocol changes** (a new op, event, field, error code or enum value) keep `v: 1` and come out as a **minor** release. Older crates keep working against newer Daemons: unknown fields are ignored, and unknown events, error codes, states and owners decode as `Unknown`. The enums that grow (`Op`, `Event`, `ErrorCode`, `State`, `Owner`) are `#[non_exhaustive]`, so match them with a wildcard arm.
- **Breaking protocol changes** bump `v` and the crate's **major** version. `Op::Hello` tells you which versions a Daemon speaks.
- Response structs (`Status`, `ClipSummary`, …) may gain public fields in a minor release. Reading them is covered by this policy; building them with struct literals (to implement a Daemon) is not.
- While the crate is 0.x, Cargo treats each 0.minor as breaking, so additive releases bump 0.minor. 1.0.0 marks protocol v1 as frozen.

The minimum supported Rust version is in `Cargo.toml` (`rust-version`) and moves only in minor releases.

## License

MIT
