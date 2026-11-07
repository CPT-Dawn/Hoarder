//! The `on_capture` hook (PLAN.md §4.1 step 7) and the Daemon's `on_clip`
//! hook (PLAN.md §9).

use std::io::Write as _;
use std::os::fd::AsFd as _;
use std::os::unix::process::CommandExt as _;
use std::process::{Command, Stdio};
use std::sync::Arc;

use crate::after::Delivered;

/// Watches the hook from a tiny shell so `hoarder shot` can exit at once and a
/// failure still gets logged to our stderr. `$1` is the user's hook.
const WATCHER: &str = r#"sh -c "$1"; s=$?; [ "$s" -eq 0 ] || echo "hoarder: on_capture hook failed (exit $s): $1" >&2"#;

/// Start `hook` with the Capture in its environment and return without
/// waiting. Its stdout goes to our stderr, so it can never corrupt a PNG
/// written to stdout. Failures are logged; none is an error for the Capture.
pub fn spawn(hook: &str, capture: &Delivered) {
    if hook.trim().is_empty() {
        return;
    }
    let stdout = match std::io::stderr().as_fd().try_clone_to_owned() {
        Ok(fd) => Stdio::from(fd),
        Err(_) => Stdio::null(),
    };
    let result = Command::new("sh")
        .args(["-c", WATCHER, "hoarder-hook", hook])
        .env(
            "HOARDER_PATH",
            capture
                .path
                .as_deref()
                .map(|p| p.as_os_str())
                .unwrap_or_default(),
        )
        .env("HOARDER_WIDTH", capture.width.to_string())
        .env("HOARDER_HEIGHT", capture.height.to_string())
        .env("HOARDER_TARGET", capture.target)
        .stdin(Stdio::null())
        .stdout(stdout)
        // Its own process group, so a Ctrl+C meant for the shot spares it.
        .process_group(0)
        .spawn();
    if let Err(e) = result {
        eprintln!("hoarder: on_capture hook failed: cannot start sh: {e}");
    }
}

/// Like [`WATCHER`], for `on_clip`.
const CLIP_WATCHER: &str =
    r#"sh -c "$1"; s=$?; [ "$s" -eq 0 ] || echo "hoarder: on_clip hook failed (exit $s): $1" >&2"#;

/// Run the `on_clip` `hook` for one Normal copy, as `wl-paste --watch CMD`
/// would: `data` (of type `mime`) on its stdin, `HOARDER_MIME`, every type
/// offered in `HOARDER_TYPES` (one per line) and the app it was copied in, if
/// known, in `HOARDER_APP`. Off the caller's thread: a slow hook never holds
/// up the Daemon, and its exit is reaped there.
pub fn spawn_on_clip(hook: &str, mime: &str, data: Arc<[u8]>, types: &[String], app: Option<&str>) {
    if hook.trim().is_empty() {
        return;
    }
    let mut command = Command::new("sh");
    command
        .args(["-c", CLIP_WATCHER, "hoarder-on-clip", hook])
        .env("HOARDER_MIME", mime)
        .env("HOARDER_TYPES", types.join("\n"))
        .env("HOARDER_APP", app.unwrap_or_default())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .process_group(0);
    let spawned = std::thread::Builder::new()
        .name("on_clip".into())
        .spawn(move || {
            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(e) => {
                    eprintln!("hoarder: on_clip hook failed: cannot start sh: {e}");
                    return;
                }
            };
            if let Some(mut stdin) = child.stdin.take() {
                // A hook that reads nothing is its own business.
                let _ = stdin.write_all(&data);
            }
            let _ = child.wait();
        });
    if let Err(e) = spawned {
        eprintln!("hoarder: on_clip hook failed: {e}");
    }
}
