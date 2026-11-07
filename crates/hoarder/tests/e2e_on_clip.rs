//! `clipboard.on_clip` (PLAN.md §9): the Daemon runs a command for each
//! Normal copy, the copy on its stdin, straight from its event loop: no
//! watcher process per change. Concealed and Secret copies never reach it.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use hoarder_e2e::{Selection, Session};

const HOARDER: &str = env!("CARGO_BIN_EXE_hoarder");

/// A token the built-in GitHub pattern conceals.
const TOKEN: &[u8] = b"ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789";

struct Daemon(Child);

impl Daemon {
    fn start(session: &Session, hook: &str) -> Daemon {
        let log = std::fs::File::create(session.runtime_dir().join("daemon.log")).unwrap();
        let child = session
            .command(HOARDER)
            .args(["--set", &format!("clipboard.on_clip={hook}"), "daemon"])
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap();
        let socket = session.runtime_dir().join("hoarder/hoarder.sock");
        wait_until("the Daemon's socket", || {
            UnixStream::connect(&socket).is_ok()
        });
        Daemon(child)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// A hook that appends `MIME|stdin` and a newline to `log`.
fn logging_hook(log: &Path) -> (String, PathBuf) {
    let hook = format!(
        r#"{{ printf '%s|' "$HOARDER_MIME"; cat; echo; }} >> '{}'"#,
        log.display()
    );
    (hook, log.to_owned())
}

#[test]
fn on_clip_gets_every_normal_copy_on_stdin_and_never_a_concealed_one() {
    let session = Session::start_default().unwrap();
    let (hook, log) = logging_hook(&session.runtime_dir().join("clips.log"));
    let _daemon = Daemon::start(&session, &hook);

    let first = session.client().unwrap();
    first
        .set_selection(Selection::Clipboard, &[("text/plain", b"hello".as_slice())])
        .unwrap();
    wait_until("the hook for the first copy", || {
        read(&log) == "text/plain|hello\n"
    });

    let secret = session.client().unwrap();
    secret
        .set_selection(Selection::Clipboard, &[("text/plain", TOKEN)])
        .unwrap();
    wait_until("the token to be read and Concealed", || {
        let out = session
            .command(HOARDER)
            .args(["status", "--json"])
            .output()
            .unwrap();
        let status: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
        status["clipboard"]["sensitivity"] == "concealed" && status["clipboard"]["reading"] == false
    });
    let copy = session
        .command(HOARDER)
        .args(["copy", "world"])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(copy.success());

    // Hooks run in order of copies, so the token would be logged by now.
    wait_until("the hook for `hoarder copy`", || {
        read(&log).contains("world")
    });
    let logged = read(&log);
    assert!(
        !logged.contains(std::str::from_utf8(TOKEN).unwrap()),
        "{logged}"
    );
    assert!(logged.starts_with("text/plain|hello\n"), "{logged}");
    assert!(logged.ends_with("|world\n"), "{logged}");
}
