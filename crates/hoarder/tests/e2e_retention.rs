//! Retention, pins and Pause (PLAN.md §5.1, §5.3): observed through
//! `hoarder history --json`, `hoarder status --json` and wl-clipboard.

use std::os::unix::net::UnixStream;
use std::process::{Child, Output, Stdio};
use std::time::{Duration, Instant};

use hoarder_e2e::{Selection, Session};
use serde_json::Value;

const HOARDER: &str = env!("CARGO_BIN_EXE_hoarder");

/// A running `hoarder daemon`, killed when dropped.
struct Daemon(Child);

impl Daemon {
    /// Start the Daemon with `args` before the subcommand (`--set k=v`).
    fn start(session: &Session, args: &[&str]) -> Daemon {
        let log_path = session.runtime_dir().join("daemon.log");
        let log = std::fs::File::create(&log_path).unwrap();
        let child = session
            .command(HOARDER)
            .args(args)
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
                let log = std::fs::read_to_string(&log_path).unwrap_or_default();
                panic!("daemon exited early ({status}): {log}");
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

/// `hoarder ARGS`, which must succeed; its stdout.
fn ok(session: &Session, args: &[&str]) -> String {
    let out = hoarder(session, args);
    assert!(
        out.status.success(),
        "hoarder {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn json(session: &Session, args: &[&str]) -> Value {
    let out = ok(session, args);
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("bad JSON {out:?}: {e}"))
}

/// `hoarder history list --json`, newest first.
fn history(session: &Session) -> Vec<Value> {
    json(session, &["history", "list", "--json"])
        .as_array()
        .expect("a JSON array")
        .clone()
}

fn previews(session: &Session) -> Vec<String> {
    history(session)
        .iter()
        .map(|c| c["preview"].as_str().unwrap().to_owned())
        .collect()
}

/// Copy `text` from a fresh app, which serves it until dropped.
fn copy(session: &Session, text: &str) -> hoarder_e2e::Client {
    let app = session.client().unwrap();
    app.set_selection(Selection::Clipboard, &[("text/plain", text.as_bytes())])
        .unwrap();
    app
}

/// Copy `text` and wait until it is the newest Clip.
fn record(session: &Session, text: &str) -> (hoarder_e2e::Client, String) {
    let app = copy(session, text);
    wait_until(&format!("{text:?} to be recorded"), || {
        history(session)
            .first()
            .is_some_and(|c| c["preview"] == text)
    });
    let id = history(session)[0]["id"].to_string();
    (app, id)
}

#[test]
fn a_pinned_clip_survives_max_clips_while_the_oldest_unpinned_goes() {
    let session = Session::start_default().unwrap();
    let _daemon = Daemon::start(&session, &["--set", "history.max_clips=2"]);
    let (_a, a) = record(&session, "pinned first");

    let pinned = json(&session, &["history", "pin", &a, "--json"]);
    assert_eq!(pinned["pinned"], true);
    let _b = record(&session, "second");
    let _c = record(&session, "third");

    wait_until("eviction", || history(&session).len() == 2);
    assert_eq!(previews(&session), ["third", "pinned first"]);
    assert_eq!(history(&session)[1]["pinned"], true);
}

#[test]
fn unpinning_makes_a_clip_evictable_again() {
    let session = Session::start_default().unwrap();
    let _daemon = Daemon::start(&session, &["--set", "history.max_clips=2"]);
    let (_a, a) = record(&session, "first");
    ok(&session, &["history", "pin", &a]);
    let _b = record(&session, "second");
    let _c = record(&session, "third");
    wait_until("eviction", || history(&session).len() == 2);
    assert_eq!(previews(&session), ["third", "first"]);

    let unpinned = json(&session, &["history", "unpin", &a, "--json"]);
    assert_eq!(unpinned["pinned"], false);
    let _d = record(&session, "fourth");

    wait_until("eviction", || history(&session).len() == 2);
    assert_eq!(previews(&session), ["fourth", "third"]);
}

#[test]
fn a_clip_over_max_clip_mb_is_not_kept_in_history_but_still_persists() {
    let session = Session::start_default().unwrap();
    let _daemon = Daemon::start(&session, &["--set", "history.max_clip_mb=1"]);
    let big = "x".repeat(2 * 1024 * 1024);
    let app = copy(&session, &big);
    settled(&session);
    app.exit();
    wait_until("Takeover", || {
        paste(&session).is_some_and(|p| p == big.as_bytes())
    });

    // History works in order: once this is in, the big one was handled.
    let _small = record(&session, "small");
    assert_eq!(previews(&session), ["small"]);
}

#[test]
fn history_clear_keeps_pins_and_all_takes_them_too() {
    let session = Session::start_default().unwrap();
    let _daemon = Daemon::start(&session, &[]);
    let (_a, a) = record(&session, "pinned");
    ok(&session, &["history", "pin", &a]);
    let _b = record(&session, "plain");

    let cleared = json(&session, &["history", "clear", "--json"]);
    assert_eq!(cleared["deleted"], 1);
    assert_eq!(previews(&session), ["pinned"]);

    ok(&session, &["history", "clear", "--all"]);
    assert!(history(&session).is_empty());
}

#[test]
fn pin_of_a_missing_clip_fails() {
    let session = Session::start_default().unwrap();
    let _daemon = Daemon::start(&session, &[]);

    let out = hoarder(&session, &["history", "pin", "999"]);

    assert_eq!(out.status.code(), Some(4));
}

/// `wl-paste --no-newline`, or `None` if nothing is copied.
fn paste(session: &Session) -> Option<Vec<u8>> {
    let out = session
        .command("wl-paste")
        .arg("--no-newline")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    out.status.success().then_some(out.stdout)
}

/// Wait until the Daemon has read an app's Offer on the clipboard.
fn settled(session: &Session) {
    wait_until("the Daemon to read the copy", || {
        let status = json(session, &["status", "--json"]);
        status["clipboard"]["owner"] == "app" && status["clipboard"]["reading"] == false
    });
}

#[test]
fn nothing_is_recorded_while_paused_but_persistence_still_works() {
    let session = Session::start_default().unwrap();
    let _daemon = Daemon::start(&session, &[]);
    let _before = record(&session, "before");

    ok(&session, &["pause"]);
    let status = json(&session, &["status", "--json"]);
    assert_eq!(status["state"], "paused");
    assert!(status.get("until").is_none(), "{status}");
    let app = copy(&session, "while paused");
    settled(&session);
    // The copy outlives its app: Takeover is not paused.
    app.exit();
    wait_until("Takeover", || {
        paste(&session).is_some_and(|p| p == b"while paused")
    });

    let resumed = json(&session, &["resume", "--json"]);
    assert_eq!(resumed["state"], "active");
    let _after = record(&session, "after");
    assert_eq!(previews(&session), ["after", "before"]);
}

#[test]
fn a_timed_pause_ends_by_itself() {
    let session = Session::start_default().unwrap();
    let _daemon = Daemon::start(&session, &[]);

    let paused = json(&session, &["pause", "1s", "--json"]);

    assert_eq!(paused["state"], "paused");
    assert!(paused["until"].is_string(), "{paused}");
    let status = ok(&session, &["status"]);
    assert!(status.contains("paused until"), "{status}");
    wait_until("the Pause to end", || {
        json(&session, &["status", "--json"])["state"] == "active"
    });
    let _clip = record(&session, "recorded again");
}

#[test]
fn a_bad_pause_duration_is_a_usage_error() {
    let session = Session::start_default().unwrap();

    let out = hoarder(&session, &["pause", "soon"]);

    assert_eq!(out.status.code(), Some(2));
}
