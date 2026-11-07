//! Auto-paste after a Restore (PLAN.md §5.4): once the Picker is off the
//! screen and focus is back on the window it covered, `hoarder picker` types
//! the paste chord through the virtual keyboard. Judged by what a helper
//! text field, pasting through its own `wl_data_device`, ends up holding.

mod common;

use std::os::unix::net::UnixStream;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use common::Overlay;
use hoarder_e2e::{Client, InputEvent, Selection, Session, TextField, keys};
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
        wait_until("the daemon socket", || {
            if let Ok(Some(status)) = daemon.0.try_wait() {
                panic!("daemon exited early ({status})");
            }
            UnixStream::connect(&socket).is_ok()
        });
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

fn history_len(session: &Session) -> usize {
    let out = session
        .command(HOARDER)
        .args(["history", "list", "--json"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "history list failed");
    serde_json::from_slice::<Value>(&out.stdout)
        .unwrap()
        .as_array()
        .expect("a JSON array")
        .len()
}

/// Copy each text in turn from a fresh app, waiting for the Daemon to
/// record it; the last one ends up on top of History.
fn copy_all(session: &Session, texts: &[&str]) -> Vec<Client> {
    let mut apps = Vec::new();
    for (n, text) in texts.iter().enumerate() {
        let app = session.client().unwrap();
        app.set_selection(Selection::Clipboard, &[("text/plain", text.as_bytes())])
            .unwrap();
        apps.push(app);
        wait_until("the Daemon to record a copy", || {
            history_len(session) == n + 1
        });
    }
    apps
}

/// `wl-paste --no-newline`.
fn paste(session: &Session) -> String {
    let out = session
        .command("wl-paste")
        .arg("--no-newline")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// History holds "older" and "newer"; a focused text field of `kind` with
/// `app_id` waits for a paste.
fn desk(app_id: &str, kind: TextField) -> (Session, Daemon, Vec<Client>, Client) {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session);
    let apps = copy_all(&session, &["older", "newer"]);
    let field = session.client().unwrap();
    field.open_text_field(app_id, kind).unwrap();
    (session, daemon, apps, field)
}

/// Open the Picker, pick the second Clip ("older") and press Enter.
fn pick_older(session: &Session, extra: &[&str]) -> common::Finished {
    let mut args = vec!["picker"];
    args.extend_from_slice(extra);
    let picker = Overlay::start(session, &args, &["HEADLESS-1"]);
    session.input().tap(keys::KEY_DOWN).unwrap();
    session.input().tap(keys::KEY_ENTER).unwrap();
    picker.finish()
}

#[test]
fn the_restored_clip_lands_in_the_focused_text_field() {
    let (session, _daemon, _apps, field) = desk("org.example.editor", TextField::Editor);

    let done = pick_older(&session, &[]);

    assert!(done.status.success(), "{}", done.stderr);
    wait_until("the text field to receive the paste", || {
        field.text().unwrap() == "older"
    });
}

#[test]
fn terminals_get_ctrl_shift_v() {
    // A Terminal field ignores Ctrl+V, so only Ctrl+Shift+V fills it.
    let (session, _daemon, _apps, field) = desk("foot", TextField::Terminal);

    let done = pick_older(&session, &[]);

    assert!(done.status.success(), "{}", done.stderr);
    wait_until("the terminal to receive the paste", || {
        field.text().unwrap() == "older"
    });
}

#[test]
fn terminal_apps_come_from_the_config() {
    let (session, _daemon, _apps, field) = desk("org.example.term", TextField::Terminal);

    let done = pick_older(
        &session,
        &["--set", "picker.terminal_apps=[\"org.example.term\"]"],
    );

    assert!(done.status.success(), "{}", done.stderr);
    wait_until("the terminal to receive the paste", || {
        field.text().unwrap() == "older"
    });
}

#[test]
fn with_auto_paste_off_the_picker_only_restores() {
    let (session, _daemon, _apps, field) = desk("org.example.editor", TextField::Editor);

    let done = pick_older(&session, &["--set", "picker.auto_paste=false"]);

    assert!(done.status.success(), "{}", done.stderr);
    assert_eq!(paste(&session), "older");
    // Any paste would have been typed before the Picker exited.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(field.text().unwrap(), "");
}

#[test]
fn a_terminal_left_off_the_list_gets_plain_ctrl_v() {
    // Plain Ctrl+V is a literal ^V to a terminal: nothing lands.
    let (session, _daemon, _apps, field) = desk("foot", TextField::Terminal);

    let done = pick_older(&session, &["--set", "picker.terminal_apps=[]"]);

    assert!(done.status.success(), "{}", done.stderr);
    assert_eq!(paste(&session), "older");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(field.text().unwrap(), "");
    let events = field.take_input_events().unwrap();
    let ctrl_v = events.windows(2).any(|w| {
        w == [
            InputEvent::Modifiers { depressed: 4 },
            InputEvent::Key {
                key: keys::KEY_V,
                pressed: true,
            },
        ]
    });
    assert!(ctrl_v, "the terminal should still see Ctrl+V: {events:?}");
}
