//! The `on_capture` hook (PLAN.md §4.1 step 7).

use std::os::fd::AsFd as _;
use std::os::unix::process::CommandExt as _;
use std::process::{Command, Stdio};

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
