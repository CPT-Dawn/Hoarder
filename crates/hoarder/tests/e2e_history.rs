//! History (PLAN.md §5.3): the Daemon records every Normal Selection change
//! as a Clip, and `hoarder history` reads and restores them over the socket.
//! Observed only through the CLI, wl-clipboard and the files on disk.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Output, Stdio};
use std::time::{Duration, Instant};

use hoarder_e2e::{Selection, Session};
use serde_json::Value;

const HOARDER: &str = env!("CARGO_BIN_EXE_hoarder");

/// A running `hoarder daemon`, killed when dropped.
struct Daemon(Child);

impl Daemon {
    fn start(session: &Session) -> Daemon {
        let log = std::fs::File::create(session.runtime_dir().join("daemon.log")).unwrap();
        let child = session
            .command(HOARDER)
            .arg("daemon")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("start hoarder daemon");
        let mut daemon = Daemon(child);
        let socket = session.runtime_dir().join("hoarder/hoarder.sock");
        let deadline = Instant::now() + Duration::from_secs(10);
        while UnixStream::connect(&socket).is_err() {
            if let Ok(Some(status)) = daemon.0.try_wait() {
                panic!("daemon exited early ({status}): {}", daemon_log(session));
            }
            assert!(Instant::now() < deadline, "daemon socket never appeared");
            std::thread::sleep(Duration::from_millis(20));
        }
        daemon
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn daemon_log(session: &Session) -> String {
    std::fs::read_to_string(session.runtime_dir().join("daemon.log")).unwrap_or_default()
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// `hoarder ARGS` in the session.
fn hoarder(session: &Session, args: &[&str]) -> Output {
    session
        .command(HOARDER)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

/// `hoarder history list --json`, newest first.
fn history(session: &Session) -> Vec<Value> {
    let out = hoarder(session, &["history", "list", "--json"]);
    assert!(
        out.status.success(),
        "history list failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice::<Value>(&out.stdout)
        .unwrap_or_else(|e| panic!("bad JSON {:?}: {e}", String::from_utf8_lossy(&out.stdout)))
        .as_array()
        .expect("a JSON array")
        .clone()
}

/// Copy `offer` from a fresh app and wait until History has `clips` Clips
/// whose newest one has its last use after `after`.
fn copy(session: &Session, offer: &[(&str, &[u8])]) -> hoarder_e2e::Client {
    let app = session.client().unwrap();
    app.set_selection(Selection::Clipboard, offer).unwrap();
    app
}

fn data_dir(session: &Session) -> PathBuf {
    session.runtime_dir().join("home/.local/share/hoarder")
}

#[test]
fn copying_the_same_content_twice_yields_one_clip_with_an_updated_time() {
    let session = Session::start_default().unwrap();
    let _daemon = Daemon::start(&session);

    let _first = copy(&session, &[("text/plain", b"hoard me".as_slice())]);
    wait_until("the first Clip", || history(&session).len() == 1);
    let before = history(&session)[0].clone();
    assert_eq!(before["kind"], "text");
    assert_eq!(before["preview"], "hoard me");

    let _other = copy(&session, &[("text/plain", b"something else".as_slice())]);
    wait_until("the second Clip", || history(&session).len() == 2);
    let _again = copy(&session, &[("text/plain", b"hoard me".as_slice())]);
    wait_until("the repeat to move to the top", || {
        history(&session)[0]["id"] == before["id"]
    });

    let clips = history(&session);
    assert_eq!(clips.len(), 2, "{clips:?}");
    assert_eq!(clips[0]["preview"], "hoard me");
    assert_ne!(clips[0]["at"], before["at"], "last use was not updated");
    assert_eq!(clips[0]["created"], before["created"]);
}
