# Testing and CI

Hoarder is tested through as few seams as possible, as high up as possible ([PLAN.md §13](PLAN.md#13-testing)). The primary seam is the `hoarder` binary run as a black box inside a throwaway **headless Sway** session. The `hoarder-e2e` crate provides that session and a helper Wayland client to drive the desktop around it.

## Running the tests

```sh
cargo test --workspace
```

The end-to-end tests need these on `PATH` (Arch: `sway grim wl-clipboard libavif dbus`):

| Tool | Why |
|---|---|
| `sway` (1.11+) | The compositor, run with `WLR_BACKENDS=headless` and `WLR_RENDERER=pixman`. No GPU, display or seat needed. |
| `grim` | Independent oracle for "what the user sees" (`Session::screenshot`). |
| `wl-copy`, `wl-paste` | Independent oracle for Selections, and the reference the `hoarder copy`/`paste` parity suite (`crates/hoarder/tests/e2e_clipboard.rs`) runs every case against, in every copier/paster pairing. |
| `avifdec` | Independent AVIF decoder for the saved-format tests (`libavif`). |
| `dbus-daemon` | A private session bus per notification test (`crates/hoarder/tests/e2e_notify.rs`), with a stand-in notification server on it, so no test ever reaches your real desktop. |

Latency budgets (4K shot-to-clipboard under 150 ms) are `#[ignore]`d in debug builds, since timing unoptimised code proves nothing. Run them with `cargo test --release -p hoarder --test e2e_shot_clipboard`; see [Timings and budgets](#timings-and-budgets) for scaling them with `HOARDER_BUDGET_SCALE`.

Each test starts its own Sway in a private directory under `/tmp`, so tests run in parallel and never touch your real desktop, config or History. Set `HOARDER_E2E_KEEP=1` to keep that directory (it holds a `--debug` `sway.log`) and print its path.

## The harness (`crates/hoarder-e2e`)

```rust
use hoarder_e2e::{keys, BitDepth, OutputSpec, Pattern, Selection, Session, Transform};

let session = Session::start(&[
    OutputSpec::new(1920, 1080),                               // HEADLESS-1
    OutputSpec::new(1280, 720).scale(2.0).depth(BitDepth::Ten), // HEADLESS-2
    OutputSpec::new(640, 480).transform(Transform::Rotate90),   // HEADLESS-3
])?;

// Run hoarder as a black box. The environment points only at this session,
// with a fresh HOME and XDG directories.
let out = session.command(env!("CARGO_BIN_EXE_hoarder")).arg("--version").output()?;

// The helper client: one Wayland connection per Client, served by its own thread.
let app = session.client()?;
app.paint(session.output("HEADLESS-1"), &Pattern::Quadrants)?;        // 1:1 physical pixels
app.set_selection(Selection::Clipboard, &[("text/plain", b"hi".as_slice())])?; // served until exit
app.open_window("org.example.app")?;                                  // focused probe window
let frame = app.capture(session.output("HEADLESS-2"))?;               // raw buffer, native depth
app.exit();                                                           // like the app quitting

// Virtual pointer and keyboard, owned by the session.
let input = session.input();
input.move_to(session.output("HEADLESS-1"), 100.0, 50.0)?;
input.click(keys::BTN_LEFT)?;
input.chord(&[keys::KEY_LEFTCTRL, keys::KEY_V])?;
let events = app.take_input_events()?; // what the probe window received

let image = session.screenshot("HEADLESS-1")?; // grim: what the user sees
```

What it guarantees:

- **Session**: outputs are named `HEADLESS-1`… in the order given and laid out left to right. `Session::start` returns once every output is active. `Session::swaymsg` runs IPC commands against it.
- **Painting**: a background layer-shell surface per output. The buffer is the output's physical size, mapped 1:1 through `wp_viewporter`, so the pattern lands on physical pixels at any scale or rotation. `paint_as(…, PixelFormat::Xrgb2101010)` paints 10-bit values; `Rgb10` colours with no 8-bit equivalent come back bit-exact from `capture` on a 10-bit output. `Pattern::Ramp` covers every 10-bit value of every channel; `Pattern::Checker { cell: 1 }` shows any resampling. Each call returns after the frame is presented.
- **Selections**: set over `ext-data-control-v1`, so no focus is needed; payloads of any size are written off-thread. `exit()` (or dropping the client) closes the connection, and the compositor clears the Selection exactly as it does when a real app quits.
- **Input**: `wlr-virtual-pointer` with absolute motion in output-logical coordinates, and `zwp-virtual-keyboard` with a US keymap. The harness tracks modifiers itself, because the protocol leaves that to the client. Every call returns after the compositor has processed it.
- **Probe windows**: plain `xdg_toplevel`s that Sway tiles and focuses like any new app. They record `InputEvent`s (enter, motion, buttons, keys, modifiers).

Every helper call times out after 10 s with an error rather than hanging the test.

## Pretending a protocol is missing

`HOARDER_HIDE_GLOBALS` takes a comma-separated list of Wayland global interface names that `hoarder` then treats as absent. Headless Sway offers every protocol Hoarder uses, so this is how tests reach the other paths:

```rust
// Force the wlr-screencopy fallback.
cmd.env("HOARDER_HIDE_GLOBALS", "ext_image_copy_capture_manager_v1");
// Look like KWin: no capture protocol at all (exit code 3).
cmd.env("HOARDER_HIDE_GLOBALS", "ext_image_copy_capture_manager_v1,zwlr_screencopy_manager_v1");
```

It is also handy for bug reports: a user can check whether a capture problem follows one protocol.

## Driving the Overlay

`crates/hoarder/tests/common` has `Overlay::start`, which spawns `hoarder shot …` and returns once grim sees every listed output change (the Overlay is up), so input sent next reaches it. `finish()` waits for the exit and hands back the status, stdout and stderr. Overlay visuals are checked with `Session::screenshot`.

## Timings and budgets

With `HOARDER_TIMINGS` set, `hoarder shot` prints stage timings on stderr, measured from the start of `shot`: `frozen_ms`, `surfaces_created_ms` and `overlay_visible_ms` (the first frame callback of every Overlay surface).

`budget_overlay_visible_on_4k` (in `e2e_overlay.rs`) checks the "key press → Overlay visible on 4K < 60 ms" budget (PLAN.md §12). The test only enforces it in optimised builds:

```sh
cargo test --release -p hoarder --test e2e_overlay budget -- --nocapture
```

Recorded results (headless Sway 1.12, pixman, one 3840×2160 output, release build, median of 5):

| Date | Change | Median | Breakdown |
|---|---|---|---|
| 2026-11-07 | #61, first cut: full Argb8888 chrome painted before the first frame | 59.5 ms | |
| 2026-11-07 | #61: idle chrome is one `wp_single_pixel_buffer_v1` pixel stretched by the viewporter | 39.9 ms | freeze 14, Overlay surfaces and frame upload 11, compositor's first frame 13 |

This is within budget, so ADR-0007 stands. The next easy win is giving the freeze's memfd to the Overlay instead of copying the 33 MB frame (about 8 ms).

### Scaling budgets on slow machines

Every budget test checks through one helper, `hoarder_e2e::budget::assert_within`, which prints the measurement next to the budget (`budget: …: measured 41.2 ms, budget 60.0 ms (PLAN 60.0 ms x 1)`) and fails if it is over. The budgets in the tests are PLAN's numbers. `HOARDER_BUDGET_SCALE` multiplies all of them (default `1.0`, so local runs enforce PLAN as written). CI sets it to `3`, because shared GitHub runners take 187–245 ms for the 150 ms shot-to-clipboard path that a desktop does well under budget. Read the printed measurements in the CI log to spot a regression that 3× would hide. New budget tests must use the helper, not their own `assert!`.

```sh
HOARDER_BUDGET_SCALE=3 cargo test --release -p hoarder --test e2e_shot_clipboard --test e2e_overlay -- 150_ms budget --nocapture
```

## CI (`.github/workflows/ci.yml`)

CI runs on pushes to `main`, `integration/**`, `ticket/**` and `fix/**`, and on every pull request.

| Job | Command | Notes |
|---|---|---|
| rustfmt | `cargo fmt --all --check` | |
| clippy | `cargo clippy --workspace --all-targets --locked -- -D warnings` | Any warning fails the build. |
| MSRV (1.97) | `cargo check --workspace --all-targets --locked` | MSRV is current stable minus two; bump it with `rust-version` in `Cargo.toml`. |
| tests (headless Sway) | `cargo test --workspace --locked`, then the timing budgets with `--release` and `HOARDER_BUDGET_SCALE=3` | Runs in an `archlinux` container, because Ubuntu's Sway predates the protocols Hoarder targets. Runs as a non-root user. |
| cargo-deny | `cargo deny check` | Advisories, licences (allow-list in `deny.toml`), bans and sources. |

To run the same checks locally before pushing:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo deny check            # cargo install --locked cargo-deny
```
