//! Sensitivity tiers (ADR-0006, PLAN.md §5.1): a Secret is never read, a
//! Concealed copy never reaches disk and is wiped after its TTL, a Normal one
//! goes to History. Observed through the socket, wl-clipboard, the helper
//! app's own view of who pasted from it, and the files on disk.

use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use hoarder_e2e::{Selection, Session};
use serde_json::{Value, json};

const HOARDER: &str = env!("CARGO_BIN_EXE_hoarder");

/// A token the built-in GitHub pattern conceals.
const TOKEN: &[u8] = b"ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789";

/// A running `hoarder daemon`, killed when dropped.
struct Daemon {
    child: Child,
    socket: PathBuf,
}

impl Daemon {
    fn start(session: &Session, args: &[&str]) -> Daemon {
        Daemon::start_with(session, args, |_| {})
    }

    fn start_with(
        session: &Session,
        args: &[&str],
        tweak: impl FnOnce(&mut std::process::Command),
    ) -> Daemon {
        let log = std::fs::File::create(session.runtime_dir().join("daemon.log")).unwrap();
        let mut command = session.command(HOARDER);
        command
            .args(args)
            .arg("daemon")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log);
        tweak(&mut command);
        let child = command.spawn().expect("start hoarder daemon");
        let mut daemon = Daemon {
            child,
            socket: session.runtime_dir().join("hoarder/hoarder.sock"),
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while UnixStream::connect(&daemon.socket).is_err() {
            if let Ok(Some(status)) = daemon.child.try_wait() {
                panic!("daemon exited early ({status})");
            }
            assert!(Instant::now() < deadline, "daemon socket never appeared");
            std::thread::sleep(Duration::from_millis(20));
        }
        daemon
    }

    fn status(&self) -> Value {
        let mut stream = UnixStream::connect(&self.socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        writeln!(stream, "{}", json!({"v": 1, "id": 1, "op": "status"})).unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        serde_json::from_str::<Value>(&line).unwrap()["ok"].clone()
    }

    /// The live Concealed Clips, as `list` with `concealed` shows them.
    fn concealed(&self) -> Vec<Value> {
        let mut stream = UnixStream::connect(&self.socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let list = json!({"v": 1, "id": 1, "op": "list", "concealed": true});
        writeln!(stream, "{list}").unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        let clips = serde_json::from_str::<Value>(&line).unwrap()["ok"].clone();
        (clips.as_array().unwrap().iter())
            .filter(|c| c["sensitivity"] == "concealed")
            .cloned()
            .collect()
    }

    /// Wait until the Daemon has seen an app's Offer and finished with it,
    /// and return the clipboard's status.
    fn settled(&self) -> Value {
        let mut clipboard = Value::Null;
        wait_until("the Daemon to finish reading", || {
            clipboard = self.status()["clipboard"].clone();
            clipboard["owner"] == "app" && clipboard["reading"] == false
        });
        clipboard
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// `wl-paste --no-newline`, or `None` if the clipboard is empty.
fn paste(session: &Session) -> Option<Vec<u8>> {
    let out = session
        .command("wl-paste")
        .args(["--no-newline", "--type", "text/plain"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    out.status.success().then_some(out.stdout)
}

/// `hoarder history list --json`.
fn history(session: &Session) -> Vec<Value> {
    let out = session
        .command(HOARDER)
        .args(["history", "list", "--json"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice::<Value>(&out.stdout)
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

/// Every file under `dir`, recursively.
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files(&path));
        } else {
            found.push(path);
        }
    }
    found
}

/// Assert that no file the Daemon wrote (SQLite, its WAL, blobs) holds
/// `secret`.
fn assert_never_on_disk(session: &Session, secret: &[u8]) {
    let home = session.runtime_dir().join("home");
    let all = files(&home);
    assert!(
        all.iter().any(|p| p.to_string_lossy().contains("hoarder")),
        "the Daemon wrote nothing at all, so this proves nothing: {all:?}"
    );
    for path in all {
        let data = std::fs::read(&path).unwrap_or_default();
        assert!(
            !data.windows(secret.len()).any(|w| w == secret),
            "{} holds the secret",
            path.display()
        );
    }
}

/// Copy `text` as a Normal Clip and wait for History to hold it. History
/// works in order, so whatever was copied before has been dealt with too.
fn copy_normal_and_wait(session: &Session, text: &[u8]) -> hoarder_e2e::Client {
    let app = session.client().unwrap();
    app.set_selection(Selection::Clipboard, &[("text/plain", text)])
        .unwrap();
    let preview = String::from_utf8_lossy(text).into_owned();
    wait_until("the Normal Clip in History", || {
        history(session)
            .iter()
            .any(|c| c["preview"] == preview.as_str())
    });
    app
}

#[test]
fn a_secret_offer_is_never_read() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);

    let manager = session.client().unwrap();
    manager
        .set_selection(
            Selection::Clipboard,
            &[
                ("text/plain", b"hunter2".as_slice()),
                ("x-kde-passwordManagerHint", b"secret"),
            ],
        )
        .unwrap();
    let clipboard = daemon.settled();
    let _later = copy_normal_and_wait(&session, b"after the secret");

    assert_eq!(clipboard["sensitivity"], "secret");
    assert_eq!(clipboard["kept"], json!([]));
    assert_eq!(manager.pasted().unwrap(), Vec::<String>::new());
    assert_eq!(history(&session).len(), 1);
}

#[test]
fn hoarder_copy_sensitive_is_secret_to_the_daemon() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);

    let out = session
        .command(HOARDER)
        .args(["copy", "--sensitive", "hunter2"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success());

    wait_until("the copy to be seen", || {
        daemon.status()["clipboard"]["sensitivity"] == "secret"
    });
    let _later = copy_normal_and_wait(&session, b"after the secret");
    assert_eq!(history(&session).len(), 1);
    assert_never_on_disk(&session, b"hunter2");
}

#[test]
fn a_token_is_concealed_kept_for_takeover_and_never_reaches_disk() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);

    let app = session.client().unwrap();
    app.set_selection(Selection::Clipboard, &[("text/plain", TOKEN)])
        .unwrap();
    let clipboard = daemon.settled();
    assert_eq!(clipboard["sensitivity"], "concealed");
    assert_eq!(clipboard["kept"], json!(["text/plain"]));

    // Still persists: the app quits and the token stays pasteable.
    app.exit();
    wait_until("Takeover", || paste(&session).as_deref() == Some(TOKEN));

    let _later = copy_normal_and_wait(&session, b"plain old text");
    assert_eq!(history(&session).len(), 1, "{:?}", history(&session));
    assert_never_on_disk(&session, TOKEN);
}

#[test]
fn anything_copied_from_an_excluded_app_is_concealed() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);

    let keepass = session.client().unwrap();
    keepass.open_window("org.keepassxc.KeePassXC").unwrap();
    keepass
        .set_selection(
            Selection::Clipboard,
            &[("text/plain", b"correct horse".as_slice())],
        )
        .unwrap();
    assert_eq!(daemon.settled()["sensitivity"], "concealed");
    drop(keepass);

    let _later = copy_normal_and_wait(&session, b"plain old text");
    assert_eq!(history(&session).len(), 1);
    assert_never_on_disk(&session, b"correct horse");
}

#[test]
fn a_concealed_clip_still_held_by_hoarder_is_cleared_when_it_expires() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &["--set", "privacy.concealed_ttl_secs=2"]);

    let app = session.client().unwrap();
    app.set_selection(Selection::Clipboard, &[("text/plain", TOKEN)])
        .unwrap();
    daemon.settled();
    app.exit();
    wait_until("Takeover", || paste(&session).as_deref() == Some(TOKEN));

    wait_until("the clipboard to be cleared", || paste(&session).is_none());
    assert_eq!(daemon.status()["clipboard"]["kept"], json!([]));
}

#[test]
fn a_concealed_clip_the_app_still_serves_is_cleared_when_it_expires() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &["--set", "privacy.concealed_ttl_secs=1"]);

    let app = session.client().unwrap();
    app.set_selection(Selection::Clipboard, &[("text/plain", TOKEN)])
        .unwrap();
    daemon.settled();

    wait_until("the app to lose the Selection", || {
        !app.owns_selection(Selection::Clipboard).unwrap()
    });
    assert_eq!(paste(&session), None);
}

#[test]
fn an_expiring_concealed_clip_leaves_a_newer_copy_alone() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &["--set", "privacy.concealed_ttl_secs=1"]);

    let first = session.client().unwrap();
    first
        .set_selection(Selection::Clipboard, &[("text/plain", TOKEN)])
        .unwrap();
    daemon.settled();
    let newer = copy_normal_and_wait(&session, b"newer");

    std::thread::sleep(Duration::from_millis(1800));
    assert!(newer.owns_selection(Selection::Clipboard).unwrap());
    assert_eq!(paste(&session).as_deref(), Some(b"newer".as_slice()));
}

#[test]
fn restoring_a_clip_over_a_concealed_selection_forgets_it() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let earlier = copy_normal_and_wait(&session, b"plain old text");
    drop(earlier);
    let app = session.client().unwrap();
    app.set_selection(Selection::Clipboard, &[("text/plain", TOKEN)])
        .unwrap();
    daemon.settled();
    app.exit();
    wait_until("Takeover", || paste(&session).as_deref() == Some(TOKEN));
    assert_eq!(daemon.concealed().len(), 1);

    let id = history(&session)[0]["id"].to_string();
    let out = session
        .command(HOARDER)
        .args(["history", "restore", &id])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success());

    assert_eq!(
        paste(&session).as_deref(),
        Some(b"plain old text".as_slice())
    );
    assert_eq!(daemon.concealed(), Vec::<Value>::new());
    assert_eq!(daemon.status()["clipboard"]["sensitivity"], "normal");
}

#[test]
fn the_focused_app_comes_from_sway_ipc_without_foreign_toplevel() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start_with(&session, &[], |cmd| {
        cmd.env("HOARDER_HIDE_GLOBALS", "zwlr_foreign_toplevel_manager_v1");
    });

    let keepass = session.client().unwrap();
    keepass.open_window("org.keepassxc.KeePassXC").unwrap();
    keepass
        .set_selection(
            Selection::Clipboard,
            &[("text/plain", b"correct horse".as_slice())],
        )
        .unwrap();

    assert_eq!(daemon.settled()["sensitivity"], "concealed");
}
