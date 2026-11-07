# Testing and CI

Hoarder is tested through as few seams as possible, as high up as possible ([PLAN.md §13](PLAN.md#13-testing)). The primary seam is the `hoarder` binary run as a black box inside a throwaway **headless Sway** session. The `hoarder-e2e` crate provides that session and a helper Wayland client to drive the desktop around it.

## Running the tests

```sh
cargo test --workspace
```

The end-to-end tests need these on `PATH` (Arch: `sway grim wl-clipboard libavif dbus zsh man-db`):

| Tool | Why |
|---|---|
| `sway` (1.11+) | The compositor, run with `WLR_BACKENDS=headless` and `WLR_RENDERER=pixman`. No GPU, display or seat needed. |
| `grim` | Independent oracle for "what the user sees" (`Session::screenshot`). |
| `wl-copy`, `wl-paste` | Independent oracle for Selections, and the reference the `hoarder copy`/`paste` parity suite (`crates/hoarder/tests/e2e_clipboard.rs`) runs every case against, in every copier/paster pairing. |
| `avifdec` | Independent AVIF decoder for the saved-format tests (`libavif`). |
| `socat`, `jq` | Optional: run the examples in docs/ipc.md (skipped without them). |
| `dbus-daemon` | A private session bus per notification test (`crates/hoarder/tests/bus/`, used by `e2e_notify.rs` and `e2e_pick.rs`), with a stand-in notification server on it, so no test ever reaches your real desktop. |
| `bash`, `zsh`, `man` | Oracles for what packages ship (`e2e_dist.rs`): bash runs the completion function, `zsh -n` parses its script, man(1) renders the man pages. |

Latency budgets (4K shot-to-clipboard under 150 ms) are `#[ignore]`d in debug builds, since timing unoptimised code proves nothing. Run them with `cargo test --release -p hoarder --test e2e_shot_clipboard`; see [Timings and budgets](#timings-and-budgets) for scaling them with `HOARDER_BUDGET_SCALE`.

Each test starts its own Sway in a private directory under `/tmp`, so tests run in parallel and never touch your real desktop, config or History. Set `HOARDER_E2E_KEEP=1` to keep that directory (it holds a `--debug` `sway.log`) and print its path.

## The harness (`crates/hoarder-e2e`)

```rust
use hoarder_e2e::{keys, BitDepth, OutputSpec, Pattern, Reply, Selection, Session, TextField, Transform};

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
app.set_selection_replying(Selection::Clipboard, vec![                // a slow or stuck source
    ("text/plain", Reply::After(Duration::from_millis(300), b"late".to_vec())),
    ("text/html", Reply::Never),
])?;
assert!(app.owns_selection(Selection::Clipboard)?);                    // not cancelled by anyone
app.open_window("org.example.app")?;                                  // focused probe window
app.open_text_field("foot", TextField::Terminal)?;                    // pastes on Ctrl+Shift+V
let frame = app.capture(session.output("HEADLESS-2"))?;               // raw buffer, native depth
app.exit();                                                           // like the app quitting

// Virtual pointer and keyboard, owned by the session.
let input = session.input();
input.move_to(session.output("HEADLESS-1"), 100.0, 50.0)?;
input.click(keys::BTN_LEFT)?;
input.chord(&[keys::KEY_LEFTCTRL, keys::KEY_V])?;
let events = app.take_input_events()?; // what the probe window received
let typed = app.text()?;               // what its text fields pasted

let image = session.screenshot("HEADLESS-1")?; // grim: what the user sees
```

What it guarantees:

- **Session**: outputs are named `HEADLESS-1`… in the order given and laid out left to right. `Session::start` returns once every output is active. `Session::swaymsg` runs IPC commands against it.
- **Painting**: a background layer-shell surface per output. The buffer is the output's physical size, mapped 1:1 through `wp_viewporter`, so the pattern lands on physical pixels at any scale or rotation. `paint_as(…, PixelFormat::Xrgb2101010)` paints 10-bit values; `Rgb10` colours with no 8-bit equivalent come back bit-exact from `capture` on a 10-bit output. `Pattern::Ramp` covers every 10-bit value of every channel; `Pattern::Checker { cell: 1 }` shows any resampling. Each call returns after the frame is presented.
- **Selections**: set over `ext-data-control-v1`, so no focus is needed; payloads of any size are written off-thread. `exit()` (or dropping the client) closes the connection, and the compositor clears the Selection exactly as it does when a real app quits. `Reply` scripts how each type answers a paste (`Now`, `After(delay)`, `Never`), which is how the Reader's time limits are tested. `owns_selection` says whether the compositor has cancelled the client's source: the independent check that the Daemon never takes a live app's Selection.
- **Input**: `wlr-virtual-pointer` with absolute motion in output-logical coordinates, and `zwp-virtual-keyboard` with a US keymap. The harness tracks modifiers itself, because the protocol leaves that to the client. Every call returns after the compositor has processed it.
- **Probe windows**: plain `xdg_toplevel`s that Sway tiles and focuses like any new app, filled with one colour (grey `#808080`, or any `Rgb8` through `open_window_coloured`, so stacked windows can be told apart in a Capture). They record `InputEvent`s (enter, motion, buttons, keys, modifiers). Float and place them with `Session::swaymsg`, e.g. `[app_id=…] floating enable, resize set 200 120, move absolute position 100 60`.
- **Text fields**: probe windows that paste like real apps, through `wl_data_device`, so they only see the clipboard while focused. A `TextField::Editor` pastes on Ctrl+V; a `TextField::Terminal` only on Ctrl+Shift+V (its Ctrl+V is a literal `^V`). `Client::text` returns everything they pasted.

Every helper call times out after 10 s with an error rather than hanging the test.

## Pretending a protocol is missing

`HOARDER_HIDE_GLOBALS` takes a comma-separated list of Wayland global interface names that `hoarder` then treats as absent. Headless Sway offers every protocol Hoarder uses, so this is how tests reach the other paths:

```rust
// Force the wlr-screencopy fallback.
cmd.env("HOARDER_HIDE_GLOBALS", "ext_image_copy_capture_manager_v1");
// Look like KWin: no capture protocol at all (exit code 3).
cmd.env("HOARDER_HIDE_GLOBALS", "ext_image_copy_capture_manager_v1,zwlr_screencopy_manager_v1");
```

It is also handy for bug reports: a user can check whether a capture problem follows one protocol. Hiding `ext_data_control_manager_v1` sends the Daemon, and `copy`/`paste` without one, to the `wlr-data-control` fallback.

## The Daemon (`crates/hoarder/tests/e2e_daemon.rs`)

The Daemon tests start `hoarder daemon` in the session and judge it only by what a user or Front end can see: `wl-paste` after the source app exits (Takeover and the Reader rules), the source still owning its Selection, socket responses, `/proc` context switches for idle wakeups, and exit codes. The `status` op's `reading` flag tells a test when the Daemon has finished reading an Offer, so a source is never quit mid-read by accident. `hoarder service` runs against a stand-in `systemctl` placed first on `PATH`, so no test touches your real user manager.

## The socket API (`crates/hoarder/tests/e2e_ipc.rs`)

The conformance suite for [docs/ipc.md](ipc.md): one test per op and per event, speaking raw JSON lines (and passing fds with `sendmsg`) to a Daemon in the session, so it checks the wire rather than the `hoarder-ipc` types. Tests wait on events from a subscribed connection instead of polling. Two tests need more than the usual tools:

- `the_documented_examples_run` runs every `sh` block in docs/ipc.md with `bash -euo pipefail`. It needs `socat` and `jq` and skips (with a note) without them.
- `connections_from_another_uid_are_refused` opens the socket's directory to everyone and has a second user try to connect, so only the `SO_PEERCRED` check stands in the way. Set `HOARDER_TEST_INTRUDER` to a user that `sudo -n -u USER` works for; it skips when unset. CI creates an `intruder` user for it.

The wire format of every type is pinned separately in `crates/hoarder-ipc/tests/wire.rs`.

## History (`crates/hoarder/tests/e2e_history.rs`)

History is judged through `hoarder history … --json`, `history get` bytes, `wl-paste` after a `history restore`, and the files under the session's private `$XDG_DATA_HOME/hoarder` (modes, and which representations became `blobs/` files). Each test polls `history list --json` until the Daemon has recorded what it copied, so nothing waits on a fixed sleep.

## Window geometry providers

`hoarder` picks its window-geometry provider from the environment: `HYPRLAND_INSTANCE_SIGNATURE` means Hyprland IPC, `SWAYSOCK` means Sway IPC, and neither means no provider (no hover highlighting). `Session::command` sets `SWAYSOCK`, so the end-to-end tests (`e2e_window.rs`) snap through Sway's `get_tree`; `cmd.env_remove("SWAYSOCK")` gives the no-provider path. Hyprland can't run headless in CI, so its parsing is covered by fixture tests (`crates/hoarder-wayland/tests/windows.rs`) and its live behaviour by the manual release checklist. The same sockets give the Daemon the focused app for `privacy.exclude_apps` (Sway `window` focus events, Hyprland `activewindowv2`); `e2e_sensitivity.rs` hides `zwlr_foreign_toplevel_manager_v1` to prove the Sway path on its own, and the event parsing has unit tests in `crates/hoarder-wayland/src/focus.rs`. `shot output` asks the same provider for the focused output.

## Driving the Overlay

`crates/hoarder/tests/common` has `Overlay::start`, which spawns `hoarder shot …` and returns once grim sees every listed output change (the Overlay is up), so input sent next reaches it. `finish()` waits for the exit and hands back the status, stdout and stderr. Overlay visuals are checked with `Session::screenshot`.

The hint bar retires after 20 uses, counted in `$XDG_STATE_HOME/hoarder/state.toml` under the session's fresh `HOME`, so every test starts with it showing. Tests that look at the bottom of an output should allow for it, or seed `hint_bar_uses = 20`.

**The cursor.** Once any client has asked headless Sway for a capture *with* cursors (`grim -c`, or `paint_cursors`), Sway keeps drawing a software cursor into every later capture of that session, including ones that ask for none. So never take a `grim -c` oracle in the session under test: `e2e_cursor.rs` takes it from a twin session set up the same way.

## Timings and budgets

With `HOARDER_TIMINGS` set, `hoarder shot` prints stage timings on stderr, measured from the start of `shot`: `frozen_ms`, `surfaces_created_ms` and `overlay_visible_ms` (the first frame callback of every Overlay surface). With `--live` there is no `frozen_ms`; `overlay_hidden_ms` marks the frame callback of the transparent Overlay, after which the desktop is captured. With `--delay`, timings start when the countdown ends.

`--delay` and `--live` are covered by `e2e_delay.rs` and `e2e_live.rs`; the `output` and `last` Targets, `--confirm` and `screenshot.freeze = false` by `e2e_shot_targets.rs`.

`budget_overlay_visible_on_4k` (in `e2e_overlay.rs`) checks the "key press → Overlay visible on 4K < 60 ms" budget (PLAN.md §12). The test only enforces it in optimised builds:

```sh
cargo test --release -p hoarder --test e2e_overlay budget -- --nocapture
```

Recorded results (headless Sway 1.12, pixman, one 3840×2160 output, release build, median of 5):

| Date | Change | Median | Breakdown |
|---|---|---|---|
| 2026-11-07 | #61, first cut: full Argb8888 chrome painted before the first frame | 59.5 ms | |
| 2026-11-07 | #61: idle chrome is one `wp_single_pixel_buffer_v1` pixel stretched by the viewporter | 39.9 ms | freeze 14, Overlay surfaces and frame upload 11, compositor's first frame 13 |
| 2026-11-07 | #63, merged with everything up to #64 and #66: the freeze also takes the cursor (cursor session); Loupe, hint bar and cursor layer are subsurfaces drawn after the first frame | 46.0 ms | freeze 15 |

This is within budget, so ADR-0007 stands. The next easy win is giving the freeze's memfd to the Overlay instead of copying the 33 MB frame (about 8 ms).

### The Picker's first frame

`hoarder picker` prints `picker_configured_ms`, `picker_history_ms` (History has arrived from the Daemon), `picker_painted_ms` and `picker_visible_ms` (the first frame callback), measured from the start of `picker`. `budget_picker_first_frame_with_1000_clips` (in `e2e_picker.rs`) seeds 1,000 Clips straight into the Store before starting the Daemon and checks the "Picker first frame < 50 ms" budget:

```sh
cargo test --release -p hoarder --test e2e_picker budget -- --nocapture
```

| Date | Change | Median | Breakdown |
|---|---|---|---|
| 2026-11-08 | #78, first cut: `FontSystem::new` on a thread, all ~2,500 system faces | 53.4 ms | History 10, fonts plus first line shaped 37 (matching a family opens every non-regular face's file) |
| 2026-11-08 | #78: fonts load first thing, and only regular upright faces are kept | 29.7 ms | History 11, fonts 20 (in parallel), paint 3, compositor 3 |

Font loading (a full fontconfig scan, about 20 ms) is now the floor. The Overlay's labels share the trimmed font set.

The Picker tests (`e2e_picker.rs`) start it with `Overlay::start(&session, &["picker"], …)`, which works for any `hoarder` command that changes what grim sees.

### The Daemon's memory

`budget_daemon_rss_idle_with_1000_clips` (in `e2e_daemon.rs`) seeds 1,000 Clips, lists them once, holds a Selection, lets the Daemon go idle and reads `/proc/PID/status`. It prints two `budget:` lines in MB: `VmRSS`, everything resident, and `RssAnon`, what the Daemon itself holds (heap, thread stacks, SQLite's cache). Only `RssAnon` is held to PLAN's 12 MB; `VmRSS` is reported for the budgets job's 20% rule, because most of it is the binary's own shared, clean text, which tracks the binary-size budget instead. Memory budgets are never scaled by `HOARDER_BUDGET_SCALE`.

```sh
cargo test --release -p hoarder --test e2e_daemon budget -- --nocapture
```

| Date | Change | VmRSS | RssAnon |
|---|---|---|---|
| 2026-11-08 | first measurement (release build; the `dist` binary measured 14.2 MB) | 14.5 MB | 3.0 MB |

### Scaling budgets on slow machines

Every budget test checks through one helper, `hoarder_e2e::budget::assert_within`, which prints the measurement next to the budget (`budget: …: measured 41.2 ms, budget 60.0 ms (PLAN 60.0 ms x 1)`) and fails if it is over. The budgets in the tests are PLAN's numbers. `HOARDER_BUDGET_SCALE` multiplies all of them (default `1.0`, so local runs enforce PLAN as written). CI sets it to `3`, because shared GitHub runners take 187–245 ms for the 150 ms shot-to-clipboard path that a desktop does well under budget. A regression that 3× would hide is caught by the `budgets` job instead (below). New budget tests must use the helper, not their own `assert!`, since that job reads their `budget:` lines.

```sh
HOARDER_BUDGET_SCALE=3 cargo test --release -p hoarder --test e2e_shot_clipboard --test e2e_overlay --test e2e_picker -- 150_ms budget --nocapture
```

### The budgets job

CI's `budgets` job runs `scripts/budgets.sh measure` on the change and on its base: the PR's merge base, or the commit a push moved the branch from. Both run on the same runner, so a slow runner slows both alike. Each run records every `budget:` line (milliseconds or MB), plus the size of the stripped release binary. `scripts/budgets.sh compare` then fails the job if any of them grew by more than 20% (`HOARDER_REGRESSION_PCT`). The table lands in the job summary. A brand-new branch has no base, so its job only reports. The absolute budgets (scaled 3×) still fail on their own.

```sh
scripts/budgets.sh measure . head.tsv                 # what CI measures, here
scripts/budgets.sh compare base.tsv head.tsv          # the 20% rule
```

The shipped binary (the `dist` profile: fat LTO, one codegen unit, stripped) measured 14.3 MB on 2026-11-08 against PLAN's 8 MB budget. The release workflow prints its size and warns when it is over.

## CI (`.github/workflows/ci.yml`)

CI runs on pushes to `main`, `integration/**`, `ticket/**` and `fix/**`, and on every pull request.

| Job | Command | Notes |
|---|---|---|
| rustfmt | `cargo fmt --all --check` | |
| clippy | `cargo clippy --workspace --all-targets --locked -- -D warnings` | Any warning fails the build. |
| MSRV (1.97) | `cargo check --workspace --all-targets --locked` | MSRV is current stable minus two; bump it with `rust-version` in `Cargo.toml`. |
| tests (headless Sway) | `cargo test --workspace --locked` | Runs in an `archlinux` container, because Ubuntu's Sway predates the protocols Hoarder targets. Runs as a non-root user. |
| budgets | `scripts/budgets.sh` on the change and its base, `--release`, `HOARDER_BUDGET_SCALE=3` | Fails on a latency budget over 3× PLAN, the Daemon's own memory over PLAN, or anything more than 20% worse than the base ([The budgets job](#the-budgets-job)). |
| cargo-deny | `cargo deny check` | Advisories, licences (allow-list in `deny.toml`), bans and sources. |

To run the same checks locally before pushing:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo deny check            # cargo install --locked cargo-deny
```
