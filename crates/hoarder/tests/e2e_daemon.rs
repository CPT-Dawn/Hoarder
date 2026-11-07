//! `hoarder daemon`: Persistence by Takeover (ADR-0005), the Reader rules and
//! the socket (PLAN.md §5.1, §11). wl-clipboard is the independent oracle for
//! what is pasteable.

use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use hoarder_e2e::{Selection, Session};
use serde_json::{Value, json};

const HOARDER: &str = env!("CARGO_BIN_EXE_hoarder");

/// A running `hoarder daemon`, killed when dropped.
struct Daemon {
    child: Child,
    socket: PathBuf,
}

impl Daemon {
    /// Start the Daemon with `args` before the subcommand (e.g. `--set k=v`)
    /// and wait until its socket answers.
    fn start(session: &Session, args: &[&str]) -> Daemon {
        Self::start_with(session, args, |_| {})
    }

    fn start_with(session: &Session, args: &[&str], tweak: impl FnOnce(&mut Command)) -> Daemon {
        let log = std::fs::File::create(session.runtime_dir().join("daemon.log")).unwrap();
        let mut cmd = session.command(HOARDER);
        cmd.args(args)
            .arg("daemon")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log);
        tweak(&mut cmd);
        let child = cmd.spawn().expect("start hoarder daemon");
        let mut daemon = Daemon {
            child,
            socket: session.runtime_dir().join("hoarder/hoarder.sock"),
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(Some(status)) = daemon.child.try_wait() {
                panic!(
                    "daemon exited early ({status}): {}",
                    std::fs::read_to_string(session.runtime_dir().join("daemon.log")).unwrap()
                );
            }
            if UnixStream::connect(&daemon.socket).is_ok() {
                return daemon;
            }
            assert!(Instant::now() < deadline, "daemon socket never appeared");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Send one request and return the response line as JSON.
    fn call(&self, request: Value) -> Value {
        let mut stream = UnixStream::connect(&self.socket).expect("connect to the socket");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        writeln!(stream, "{request}").unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad response {line:?}: {e}"))
    }

    fn status(&self) -> Value {
        let response = self.call(json!({"v": 1, "id": 1, "op": "status"}));
        response["ok"].clone()
    }

    /// Wait until the Daemon has finished reading the Selection it sees.
    fn settled(&self) -> Value {
        let mut status = Value::Null;
        wait_until("the Daemon to finish reading", || {
            status = self.status();
            status["clipboard"]["reading"] == false
        });
        status
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

/// `wl-paste ARGS`: exit code and stdout.
fn wl_paste(session: &Session, args: &[&str]) -> (Option<i32>, Vec<u8>) {
    let out = session
        .command("wl-paste")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (out.status.code(), out.stdout)
}

/// The types `wl-paste --list-types` shows, or `None` if nothing is copied.
fn offered(session: &Session) -> Option<Vec<String>> {
    let (code, out) = wl_paste(session, &["--list-types"]);
    (code == Some(0)).then(|| {
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    })
}

fn paste_type(session: &Session, mime: &str) -> Vec<u8> {
    let (code, out) = wl_paste(session, &["--no-newline", "--type", mime]);
    assert_eq!(code, Some(0), "wl-paste -t {mime} failed");
    out
}

#[test]
fn hello_names_the_protocol_and_the_daemon() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);

    let response = daemon.call(json!({"v": 1, "id": 7, "op": "hello"}));

    assert_eq!(response["v"], 1);
    assert_eq!(response["id"], 7);
    assert_eq!(response["ok"]["protocol"], 1);
    assert_eq!(response["ok"]["daemon"], env!("CARGO_PKG_VERSION"));
    assert_eq!(response["ok"]["pid"], daemon.child.id());
}

#[test]
fn the_socket_lives_in_a_private_directory() {
    use std::os::unix::fs::PermissionsExt as _;
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);

    let dir = daemon.socket.parent().unwrap();
    let mode = std::fs::metadata(dir).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
}

#[test]
fn text_outlives_the_app_that_copied_it() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let app = session.client().unwrap();
    let types = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"];
    app.set_selection(
        Selection::Clipboard,
        &[
            (types[0], b"hello, hoard".as_slice()),
            (types[1], b"hello, hoard"),
            (types[2], b"hello, hoard"),
        ],
    )
    .unwrap();
    daemon.settled();

    app.exit();

    wait_until("Takeover", || offered(&session).is_some());
    assert_eq!(offered(&session).unwrap(), types);
    for mime in types {
        assert_eq!(paste_type(&session, mime), b"hello, hoard");
    }
}

/// A tiny but real PNG: 1×1, one red pixel.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8, 0xcf, 0xc0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xdd, 0x8d, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

#[test]
fn an_image_and_its_text_outlive_the_app_in_the_original_order() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let app = session.client().unwrap();
    app.set_selection(
        Selection::Clipboard,
        &[
            ("image/png", PNG),
            ("text/html", b"<img src=\"red.png\">"),
            ("text/plain", b"red.png"),
        ],
    )
    .unwrap();
    daemon.settled();

    app.exit();

    wait_until("Takeover", || offered(&session).is_some());
    assert_eq!(
        offered(&session).unwrap(),
        ["image/png", "text/html", "text/plain"]
    );
    assert_eq!(paste_type(&session, "image/png"), PNG);
    assert_eq!(paste_type(&session, "text/html"), b"<img src=\"red.png\">");
    assert_eq!(paste_type(&session, "text/plain"), b"red.png");
}

#[test]
fn hoarder_never_owns_the_selection_while_its_source_is_alive() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let app = session.client().unwrap();
    app.set_selection(Selection::Clipboard, &[("text/plain", b"mine".as_slice())])
        .unwrap();
    let status = daemon.settled();

    // Pastes keep going to the app, and the compositor never took its
    // Selection away.
    assert_eq!(paste_type(&session, "text/plain"), b"mine");
    std::thread::sleep(Duration::from_millis(300));
    assert!(app.owns_selection(Selection::Clipboard).unwrap());
    assert_eq!(status["clipboard"]["owner"], "app");
    assert_eq!(daemon.status()["clipboard"]["owner"], "app");
}

#[test]
fn nothing_is_kept_with_persist_off() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &["--set", "clipboard.persist=false"]);
    let app = session.client().unwrap();
    app.set_selection(
        Selection::Clipboard,
        &[("text/plain", b"fleeting".as_slice())],
    )
    .unwrap();
    daemon.settled();

    app.exit();

    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(offered(&session), None);
}

fn hoarder_clear(session: &Session) {
    let out = session.command(HOARDER).arg("clear").output().unwrap();
    assert!(
        out.status.success(),
        "hoarder clear failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn hoarder_clear_empties_a_taken_over_clipboard_for_good() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let app = session.client().unwrap();
    app.set_selection(
        Selection::Clipboard,
        &[("text/plain", b"forget me".as_slice())],
    )
    .unwrap();
    daemon.settled();
    app.exit();
    wait_until("Takeover", || offered(&session).is_some());

    hoarder_clear(&session);

    assert_eq!(offered(&session), None);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(offered(&session), None);
    assert_eq!(daemon.status()["clipboard"]["owner"], "none");
}

#[test]
fn hoarder_clear_while_the_app_lives_is_not_undone_when_it_exits() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let app = session.client().unwrap();
    app.set_selection(
        Selection::Clipboard,
        &[("text/plain", b"forget me".as_slice())],
    )
    .unwrap();
    daemon.settled();

    hoarder_clear(&session);
    assert_eq!(offered(&session), None);
    app.exit();

    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(offered(&session), None);
}

#[test]
fn hoarder_clear_works_without_a_daemon() {
    let session = Session::start_default().unwrap();
    let app = session.client().unwrap();
    app.set_selection(Selection::Clipboard, &[("text/plain", b"bye".as_slice())])
        .unwrap();

    hoarder_clear(&session);

    assert_eq!(offered(&session), None);
    assert!(!app.owns_selection(Selection::Clipboard).unwrap());
}

#[test]
fn a_clipboard_cleared_by_another_tool_is_restored() {
    // ADR-0005's known trade-off: only `hoarder clear` is honoured.
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let app = session.client().unwrap();
    app.set_selection(
        Selection::Clipboard,
        &[("text/plain", b"stubborn".as_slice())],
    )
    .unwrap();
    daemon.settled();
    app.exit();
    wait_until("Takeover", || offered(&session).is_some());

    let status = session.command("wl-copy").arg("--clear").status().unwrap();
    assert!(status.success());

    wait_until("the restore", || offered(&session).is_some());
    assert_eq!(paste_type(&session, "text/plain"), b"stubborn");
}
