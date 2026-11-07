//! The public socket API (ADR-0004, PLAN.md §11, docs/ipc.md): a conformance
//! suite that speaks raw JSON lines to a real Daemon in a headless Sway
//! session, one test per op and per event, plus the same-user check and the
//! documented socat/jq examples. wl-clipboard is the independent oracle for
//! what is pasteable.

use std::collections::VecDeque;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::fd::{AsFd as _, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
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
    fn start(session: &Session, args: &[&str]) -> Daemon {
        let log = std::fs::File::create(session.runtime_dir().join("daemon.log")).unwrap();
        let child = session
            .command(HOARDER)
            .args(args)
            .arg("daemon")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("start hoarder daemon");
        let mut daemon = Daemon {
            child,
            socket: session.runtime_dir().join("hoarder/hoarder.sock"),
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while UnixStream::connect(&daemon.socket).is_err() {
            if let Ok(Some(status)) = daemon.child.try_wait() {
                panic!("daemon exited early ({status}): {}", log_of(session));
            }
            assert!(Instant::now() < deadline, "daemon socket never appeared");
            std::thread::sleep(Duration::from_millis(20));
        }
        daemon
    }

    fn connect(&self) -> Wire {
        Wire::connect(&self.socket)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn log_of(session: &Session) -> String {
    std::fs::read_to_string(session.runtime_dir().join("daemon.log")).unwrap_or_default()
}

/// One raw connection: JSON lines out, JSON lines in, events kept aside
/// while a response is awaited.
struct Wire {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: u64,
    events: VecDeque<Value>,
}

impl Wire {
    fn connect(socket: &Path) -> Wire {
        let stream = UnixStream::connect(socket).expect("connect to the socket");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let reader = BufReader::new(stream.try_clone().unwrap());
        Wire {
            stream,
            reader,
            next_id: 1,
            events: VecDeque::new(),
        }
    }

    /// The request line for `op` (an object with an `op`), numbered.
    fn line(&mut self, mut op: Value) -> (u64, String) {
        let id = self.next_id;
        self.next_id += 1;
        op["v"] = json!(1);
        op["id"] = json!(id);
        (id, format!("{op}\n"))
    }

    /// Send `op` and return the whole response line.
    fn call(&mut self, op: Value) -> Value {
        let (id, line) = self.line(op);
        self.stream.write_all(line.as_bytes()).unwrap();
        self.response(id)
    }

    /// Send `op` with `fd` attached (`SCM_RIGHTS`); the whole response.
    fn call_with_fd(&mut self, op: Value, fd: BorrowedFd<'_>) -> Value {
        use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags, sendmsg};
        let (id, line) = self.line(op);
        let mut space = [std::mem::MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = SendAncillaryBuffer::new(&mut space);
        let fds = [fd];
        ancillary.push(SendAncillaryMessage::ScmRights(&fds));
        let sent = sendmsg(
            &self.stream,
            &[std::io::IoSlice::new(line.as_bytes())],
            &mut ancillary,
            SendFlags::NOSIGNAL,
        )
        .unwrap();
        self.stream.write_all(&line.as_bytes()[sent..]).unwrap();
        self.response(id)
    }

    /// `ok` of a call that must succeed.
    fn ok(&mut self, op: Value) -> Value {
        let response = self.call(op.clone());
        assert!(response.get("ok").is_some(), "{op} failed: {response}");
        response["ok"].clone()
    }

    /// `err.code` of a call that must fail; its message must say something.
    fn err(&mut self, op: Value) -> String {
        let response = self.call(op.clone());
        assert!(response.get("ok").is_none(), "{op} succeeded: {response}");
        assert!(
            !response["err"]["message"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "{response}"
        );
        response["err"]["code"].as_str().unwrap().to_owned()
    }

    fn read_line(&mut self) -> Value {
        let mut line = String::new();
        let n = self.reader.read_line(&mut line).expect("a line in time");
        assert!(n > 0, "the Daemon hung up");
        let value: Value =
            serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad line {line:?}: {e}"));
        assert_eq!(value["v"], 1, "every line carries the version: {line}");
        value
    }

    fn response(&mut self, id: u64) -> Value {
        loop {
            let line = self.read_line();
            if line.get("event").is_some() {
                self.events.push_back(line);
            } else {
                // One request at a time, so this is its answer.
                assert_eq!(line["id"], id, "the answer to request {id}: {line}");
                let keys: Vec<&String> = line.as_object().unwrap().keys().collect();
                assert_eq!(keys.len(), 3, "v, id and one of ok/err: {line}");
                return line;
            }
        }
    }

    /// The next event line, whatever it is.
    fn event(&mut self) -> Value {
        if let Some(event) = self.events.pop_front() {
            return event;
        }
        let line = self.read_line();
        assert!(line.get("event").is_some(), "an event, not {line}");
        line
    }

    /// The next `name` event, skipping other kinds.
    fn next(&mut self, name: &str) -> Value {
        loop {
            let event = self.event();
            if event["event"] == name {
                return event;
            }
        }
    }

    /// A subscribed connection, with its opening events read.
    fn subscribed(daemon: &Daemon) -> Wire {
        let mut wire = daemon.connect();
        assert_eq!(wire.ok(json!({"op": "subscribe"})), Value::Null);
        for _ in 0..2 {
            assert_eq!(wire.event()["event"], "selection");
        }
        assert_eq!(wire.event()["event"], "state");
        wire
    }
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Copy `offer` from a fresh app, which keeps serving it until dropped.
fn copy(session: &Session, offer: &[(&str, &[u8])]) -> hoarder_e2e::Client {
    let app = session.client().unwrap();
    app.set_selection(Selection::Clipboard, offer).unwrap();
    app
}

/// Copy `text` and wait for its `clip_added` on `events`.
fn add(session: &Session, events: &mut Wire, text: &str) -> (hoarder_e2e::Client, Value) {
    let app = copy(session, &[("text/plain", text.as_bytes())]);
    let clip = events.next("clip_added")["clip"].clone();
    assert_eq!(clip["preview"], text);
    (app, clip)
}

/// `wl-paste --no-newline --type MIME`.
fn paste(session: &Session, mime: &str) -> Vec<u8> {
    let out = session
        .command("wl-paste")
        .args(["--no-newline", "--type", mime])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "wl-paste -t {mime} failed");
    out.stdout
}

fn pipe() -> (std::fs::File, std::os::fd::OwnedFd) {
    let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
    (std::fs::File::from(read), write)
}

fn read_all(mut file: std::fs::File) -> Vec<u8> {
    let mut data = Vec::new();
    file.read_to_end(&mut data).unwrap();
    data
}

fn assert_rfc3339(value: &Value) {
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("{value} is a time"));
    text.parse::<jiff::Timestamp>()
        .unwrap_or_else(|e| panic!("{text} is not RFC 3339: {e}"));
}

/// Every field a ClipSummary promises, with its type.
fn assert_clip(clip: &Value) {
    assert!(clip["id"].is_i64(), "{clip}");
    assert!(
        ["text", "image", "files", "other"].contains(&clip["kind"].as_str().unwrap()),
        "{clip}"
    );
    assert!(clip["preview"].is_string(), "{clip}");
    assert!(clip["app"].is_string() || clip["app"].is_null(), "{clip}");
    assert_rfc3339(&clip["at"]);
    assert_rfc3339(&clip["created"]);
    assert!(clip["pinned"].is_boolean(), "{clip}");
    assert!(clip["size"].is_u64(), "{clip}");
    assert!(clip["types"].is_array(), "{clip}");
}

// ---- ops -------------------------------------------------------------

#[test]
fn hello_negotiates_the_version() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let hello = daemon.connect().ok(json!({"op": "hello"}));
    assert_eq!(hello["protocol"], 1);
    assert_eq!(hello["versions"], json!([1]));
    assert_eq!(hello["daemon"], env!("CARGO_PKG_VERSION"));
    assert_eq!(hello["pid"], daemon.child.id());
}

#[test]
fn status_describes_the_daemon_and_each_selection() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let status = daemon.connect().ok(json!({"op": "status"}));
    assert_eq!(status["daemon"], env!("CARGO_PKG_VERSION"));
    assert_eq!(status["pid"], daemon.child.id());
    assert_eq!(status["data_control"], "ext");
    assert_eq!(status["persist"], true);
    assert_eq!(status["state"], "active");
    assert!(status.get("until").is_none(), "{status}");
    assert_eq!(
        status["clipboard"],
        json!({"owner": "none", "types": [], "kept": [], "reading": false, "sensitivity": "normal"})
    );
    assert_eq!(
        status["primary"],
        Value::Null,
        "primary is ignored by default"
    );
}

#[test]
fn subscribe_answers_then_tells_the_present() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let _app = copy(&session, &[("text/plain", b"now".as_slice())]);
    wait_until("the Daemon to see the copy", || {
        daemon.connect().ok(json!({"op": "status"}))["clipboard"]["owner"] == "app"
    });

    let mut wire = daemon.connect();
    assert_eq!(wire.ok(json!({"op": "subscribe"})), Value::Null);
    assert_eq!(
        wire.event(),
        json!({"v": 1, "event": "selection", "selection": "clipboard", "types": ["text/plain"]})
    );
    assert_eq!(
        wire.event(),
        json!({"v": 1, "event": "selection", "selection": "primary", "types": null})
    );
    assert_eq!(
        wire.event(),
        json!({"v": 1, "event": "state", "state": "active"})
    );
}

#[test]
fn list_and_search_show_history_newest_first() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let mut events = Wire::subscribed(&daemon);
    let (_a, first) = add(&session, &mut events, "an apple a day");
    let (_b, second) = add(&session, &mut events, "a banana split");

    let mut wire = daemon.connect();
    let all = wire.ok(json!({"op": "list"}));
    let ids: Vec<&Value> = all.as_array().unwrap().iter().map(|c| &c["id"]).collect();
    assert_eq!(ids, [&second["id"], &first["id"]]);
    all.as_array().unwrap().iter().for_each(assert_clip);
    assert_eq!(all[0], second, "list and clip_added agree");

    assert_eq!(wire.ok(json!({"op": "list", "limit": 1})), json!([second]));
    assert_eq!(
        wire.ok(json!({"op": "search", "query": "APPLE"})),
        json!([first])
    );
    assert_eq!(wire.ok(json!({"op": "search", "query": "a "})), json!([]));
    assert_eq!(
        wire.ok(json!({"op": "search", "query": "a d", "limit": 5})),
        json!([first])
    );
}

#[test]
fn get_streams_a_representation_into_a_passed_pipe() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let mut events = Wire::subscribed(&daemon);
    let _app = copy(
        &session,
        &[
            ("text/html", b"<b>bold</b>".as_slice()),
            ("text/plain", b"bold".as_slice()),
        ],
    );
    let clip = events.next("clip_added")["clip"].clone();
    let id = clip["id"].clone();
    let mut wire = daemon.connect();

    let (read, write) = pipe();
    let got = wire.call_with_fd(json!({"op": "get", "clip": id}), write.as_fd());
    drop(write);
    assert_eq!(
        got["ok"],
        json!({"clip": clip, "mime": "text/html", "size": 11})
    );
    assert_eq!(read_all(read), b"<b>bold</b>");

    let (read, write) = pipe();
    let got = wire.call_with_fd(
        json!({"op": "get", "clip": id, "mime": "text/plain"}),
        write.as_fd(),
    );
    drop(write);
    assert_eq!(got["ok"]["size"], 4);
    assert_eq!(read_all(read), b"bold");

    let (_read, write) = pipe();
    let missing = wire.call_with_fd(
        json!({"op": "get", "clip": id, "mime": "image/png"}),
        write.as_fd(),
    );
    assert_eq!(missing["err"]["code"], "not_found");
    assert_eq!(wire.err(json!({"op": "get", "clip": id})), "bad_request");
    let (_read, write) = pipe();
    let gone = wire.call_with_fd(json!({"op": "get", "clip": 999_999}), write.as_fd());
    assert_eq!(gone["err"]["code"], "not_found");
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(width, height, image::Rgb([200, 30, 90]));
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

#[test]
fn thumb_streams_a_small_png() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let mut events = Wire::subscribed(&daemon);
    let big = png(1024, 512);
    let _app = copy(&session, &[("image/png", &big)]);
    let clip = events.next("clip_added")["clip"].clone();
    assert_eq!(clip["kind"], "image");
    assert_eq!(clip["preview"], "1024×512");
    let mut wire = daemon.connect();

    // The thumbnail is made after the Clip is recorded.
    let mut thumb = Value::Null;
    let mut data = Vec::new();
    wait_until("a thumbnail", || {
        let (read, write) = pipe();
        thumb = wire.call_with_fd(json!({"op": "thumb", "clip": clip["id"]}), write.as_fd());
        drop(write);
        data = read_all(read);
        thumb.get("ok").is_some()
    });
    assert_eq!(thumb["ok"]["mime"], "image/png");
    assert_eq!(thumb["ok"]["size"], data.len());
    let small = image::load_from_memory(&data).unwrap();
    assert_eq!((small.width(), small.height()), (256, 128));

    let (_t, text) = add(&session, &mut events, "no picture");
    let (_read, write) = pipe();
    let none = wire.call_with_fd(json!({"op": "thumb", "clip": text["id"]}), write.as_fd());
    assert_eq!(none["err"]["code"], "not_found");
}

#[test]
fn restore_makes_a_clip_the_clipboard_and_moves_it_up() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let mut events = Wire::subscribed(&daemon);
    let (_a, old) = add(&session, &mut events, "bring me back");
    let (_b, _new) = add(&session, &mut events, "newer");

    let mut wire = daemon.connect();
    let restored = wire.ok(json!({"op": "restore", "clip": old["id"]}));
    assert_eq!(restored["id"], old["id"]);
    assert_clip(&restored);
    assert_eq!(paste(&session, "text/plain"), b"bring me back");
    let added = events.next("clip_added");
    assert_eq!(added["clip"], restored);
    assert_eq!(
        wire.ok(json!({"op": "list", "limit": 1}))[0]["id"],
        old["id"]
    );
    assert_eq!(
        wire.err(json!({"op": "restore", "clip": 999_999})),
        "not_found"
    );
}

#[test]
fn delete_removes_a_clip_and_says_so() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let mut events = Wire::subscribed(&daemon);
    let (_a, clip) = add(&session, &mut events, "doomed");
    let mut wire = daemon.connect();
    assert_eq!(
        wire.ok(json!({"op": "delete", "clip": clip["id"]})),
        Value::Null
    );
    assert_eq!(
        events.next("clip_removed"),
        json!({"v": 1, "event": "clip_removed", "clip": clip["id"]})
    );
    assert_eq!(wire.ok(json!({"op": "list"})), json!([]));
    assert_eq!(
        wire.err(json!({"op": "delete", "clip": clip["id"]})),
        "not_found"
    );
}

#[test]
fn pin_and_unpin_answer_with_the_clip() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let mut events = Wire::subscribed(&daemon);
    let (_a, clip) = add(&session, &mut events, "keep me");
    let mut wire = daemon.connect();
    let pinned = wire.ok(json!({"op": "pin", "clip": clip["id"]}));
    assert_eq!(pinned["pinned"], true);
    assert_eq!(pinned["id"], clip["id"]);
    let unpinned = wire.ok(json!({"op": "unpin", "clip": clip["id"]}));
    assert_eq!(unpinned["pinned"], false);
    assert_eq!(wire.err(json!({"op": "pin", "clip": 999_999})), "not_found");
    assert_eq!(
        wire.err(json!({"op": "unpin", "clip": 999_999})),
        "not_found"
    );
}

#[test]
fn clear_history_keeps_pins_unless_asked_and_removes_each_clip() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let mut events = Wire::subscribed(&daemon);
    let (_a, pinned) = add(&session, &mut events, "pinned one");
    let (_b, loose) = add(&session, &mut events, "loose one");
    let mut wire = daemon.connect();
    wire.ok(json!({"op": "pin", "clip": pinned["id"]}));

    assert_eq!(
        wire.ok(json!({"op": "clear_history"})),
        json!({"deleted": 1})
    );
    assert_eq!(events.next("clip_removed")["clip"], loose["id"]);
    let left = wire.ok(json!({"op": "list"}));
    assert_eq!(left.as_array().unwrap().len(), 1);
    assert_eq!(left[0]["id"], pinned["id"]);

    assert_eq!(
        wire.ok(json!({"op": "clear_history", "pinned": true})),
        json!({"deleted": 1})
    );
    assert_eq!(events.next("clip_removed")["clip"], pinned["id"]);
    assert_eq!(wire.ok(json!({"op": "list"})), json!([]));
}

#[test]
fn pause_and_resume_answer_and_announce_the_state() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let mut events = Wire::subscribed(&daemon);
    let mut wire = daemon.connect();

    assert_eq!(wire.ok(json!({"op": "pause"})), json!({"state": "paused"}));
    assert_eq!(
        events.next("state"),
        json!({"v": 1, "event": "state", "state": "paused"})
    );
    assert_eq!(wire.ok(json!({"op": "status"}))["state"], "paused");

    let _app = copy(&session, &[("text/plain", b"unrecorded".as_slice())]);
    assert_eq!(events.next("selection")["types"], json!(["text/plain"]));

    assert_eq!(wire.ok(json!({"op": "resume"})), json!({"state": "active"}));
    assert_eq!(
        events.next("state"),
        json!({"v": 1, "event": "state", "state": "active"})
    );
    assert_eq!(
        wire.ok(json!({"op": "list"})),
        json!([]),
        "nothing recorded while paused"
    );

    // A timed Pause says when it ends, and announces its own end.
    let timed = wire.ok(json!({"op": "pause", "secs": 1}));
    assert_eq!(timed["state"], "paused");
    assert_rfc3339(&timed["until"]);
    let event = events.next("state");
    assert_eq!(event["until"], timed["until"]);
    assert_eq!(
        events.next("state"),
        json!({"v": 1, "event": "state", "state": "active"})
    );
}

fn memfd(data: &[u8]) -> std::fs::File {
    let fd = rustix::fs::memfd_create("offer", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = std::fs::File::from(fd);
    file.write_all(data).unwrap();
    file
}

#[test]
fn offer_serves_a_passed_memfd_as_the_clipboard() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let mut events = Wire::subscribed(&daemon);
    let mut wire = daemon.connect();

    let data = memfd(b"offered");
    let response = wire.call_with_fd(
        json!({"op": "offer", "types": ["text/plain", "UTF8_STRING"]}),
        data.as_fd(),
    );
    assert_eq!(response["ok"], Value::Null, "{response}");
    assert_eq!(paste(&session, "UTF8_STRING"), b"offered");
    assert_eq!(
        events.next("selection")["types"],
        json!(["text/plain", "UTF8_STRING"])
    );
    assert_eq!(events.next("clip_added")["clip"]["preview"], "offered");

    assert_eq!(
        wire.err(json!({"op": "offer", "types": ["text/plain"]})),
        "bad_request",
        "an offer without its memfd"
    );
    let (_read, write) = pipe();
    let piped = wire.call_with_fd(
        json!({"op": "offer", "types": ["text/plain"]}),
        write.as_fd(),
    );
    assert_eq!(piped["err"]["code"], "bad_request", "a pipe is not a memfd");
}

#[test]
fn paste_streams_the_current_selection_from_its_source() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let mut events = Wire::subscribed(&daemon);
    let _app = copy(&session, &[("text/plain", b"live data".as_slice())]);
    events.next("selection");
    let mut wire = daemon.connect();

    let (read, write) = pipe();
    let response = wire.call_with_fd(json!({"op": "paste", "mime": "text/plain"}), write.as_fd());
    drop(write);
    assert_eq!(response["ok"], Value::Null, "{response}");
    assert_eq!(read_all(read), b"live data");

    let (_read, write) = pipe();
    let response = wire.call_with_fd(json!({"op": "paste", "mime": "image/png"}), write.as_fd());
    assert_eq!(response["err"]["code"], "not_found");
}

#[test]
fn clear_empties_the_clipboard_and_announces_it() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let mut events = Wire::subscribed(&daemon);
    let _app = copy(&session, &[("text/plain", b"wipe me".as_slice())]);
    events.next("selection");
    let mut wire = daemon.connect();
    assert_eq!(wire.ok(json!({"op": "clear"})), Value::Null);
    assert_eq!(
        events.next("selection"),
        json!({"v": 1, "event": "selection", "selection": "clipboard", "types": null})
    );
    let out = session
        .command("wl-paste")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success(), "the clipboard is empty");
}

fn config_path(session: &Session) -> PathBuf {
    session
        .runtime_dir()
        .join("home/.config/hoarder/config.toml")
}

#[test]
fn reload_rereads_the_config_or_says_why_not() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let mut wire = daemon.connect();
    let path = config_path(&session);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();

    std::fs::write(&path, "[clipboard]\npersist = false\n").unwrap();
    assert_eq!(wire.ok(json!({"op": "reload"})), Value::Null);
    assert_eq!(wire.ok(json!({"op": "status"}))["persist"], false);

    std::fs::write(&path, "[clipboard]\npersist = \"maybe\"\n").unwrap();
    let response = wire.call(json!({"op": "reload"}));
    assert_eq!(response["err"]["code"], "failed", "{response}");
    let message = response["err"]["message"].as_str().unwrap();
    assert!(message.contains("2"), "names the line: {message}");
    assert_eq!(wire.ok(json!({"op": "status"}))["persist"], false, "kept");
}

#[test]
fn malformed_requests_get_coded_errors_and_the_connection_lives_on() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let mut wire = daemon.connect();
    let mut raw = |line: &str| {
        wire.stream.write_all(line.as_bytes()).unwrap();
        wire.read_line()
    };
    let not_json = raw("hoard everything\n");
    assert_eq!(not_json["err"]["code"], "bad_request");
    assert!(not_json.get("id").is_none(), "no id to echo: {not_json}");
    assert_eq!(
        raw("{\"v\":1,\"op\":\"hello\"}\n")["err"]["code"],
        "bad_request"
    );
    let future = raw("{\"v\":99,\"id\":5,\"op\":\"hello\"}\n");
    assert_eq!(future["err"]["code"], "unsupported_version");
    assert_eq!(future["id"], 5);
    let dance = raw("{\"v\":1,\"id\":6,\"op\":\"dance\"}\n");
    assert_eq!(dance["err"]["code"], "unknown_op");
    assert_eq!(dance["id"], 6);
    let extra = raw("{\"v\":1,\"id\":7,\"op\":\"hello\",\"from\":\"the future\"}\n");
    assert_eq!(extra["ok"]["protocol"], 1, "unknown fields are ignored");
}

#[test]
fn requests_sent_before_hanging_up_the_write_side_are_answered() {
    // What `echo … | socat - UNIX-CONNECT:…` does.
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let mut wire = daemon.connect();
    let requests = concat!(
        "{\"v\":1,\"id\":1,\"op\":\"hello\"}\n",
        "{\"v\":1,\"id\":2,\"op\":\"list\"}\n",
        "{\"v\":1,\"id\":3,\"op\":\"status\"}\n",
    );
    wire.stream.write_all(requests.as_bytes()).unwrap();
    wire.stream.shutdown(std::net::Shutdown::Write).unwrap();
    // Answers may overtake each other (`list` goes via the History
    // thread); the ids pair them up.
    let mut answers: Vec<Value> = (0..3).map(|_| wire.read_line()).collect();
    answers.sort_by_key(|a| a["id"].as_u64());
    assert_eq!(answers[0]["ok"]["protocol"], 1);
    assert_eq!(answers[1]["ok"], json!([]));
    assert_eq!(answers[2]["ok"]["state"], "active");
    let mut rest = String::new();
    wire.reader.read_to_string(&mut rest).unwrap();
    assert_eq!(rest, "", "then the Daemon hangs up");
}

// ---- events ----------------------------------------------------------

#[test]
fn a_copy_announces_the_selection_then_the_new_clip() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let mut events = Wire::subscribed(&daemon);
    let _app = copy(&session, &[("text/plain", b"fresh".as_slice())]);
    assert_eq!(
        events.event(),
        json!({"v": 1, "event": "selection", "selection": "clipboard", "types": ["text/plain"]})
    );
    let added = events.event();
    assert_eq!(added["event"], "clip_added");
    assert_clip(&added["clip"]);
    assert_eq!(added["clip"]["preview"], "fresh");
    assert_eq!(added["clip"]["size"], 5);
    assert_eq!(added["clip"]["types"], json!(["text/plain"]));

    // The same content again is the same Clip, moved to the top.
    let _other = copy(&session, &[("text/plain", b"other".as_slice())]);
    events.next("clip_added");
    let _again = copy(&session, &[("text/plain", b"fresh".as_slice())]);
    assert_eq!(events.next("clip_added")["clip"]["id"], added["clip"]["id"]);
}

#[test]
fn private_copies_announce_no_clip() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let mut events = Wire::subscribed(&daemon);
    let _secret = copy(
        &session,
        &[
            ("text/plain", b"hunter2".as_slice()),
            ("x-kde-passwordManagerHint", b"secret".as_slice()),
        ],
    );
    events.next("selection");
    let (_app, clip) = add(&session, &mut events, "public");
    assert_eq!(
        clip["preview"], "public",
        "no clip_added came for the secret"
    );
}

#[test]
fn expiry_announces_each_evicted_clip() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &["--set", "history.max_clips=1"]);
    let mut events = Wire::subscribed(&daemon);
    let (_a, first) = add(&session, &mut events, "first out");
    let (_b, _second) = add(&session, &mut events, "second in");
    assert_eq!(events.next("clip_removed")["clip"], first["id"]);
}

#[test]
fn every_subscriber_hears_every_event_and_others_hear_none() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let mut one = Wire::subscribed(&daemon);
    let mut two = Wire::subscribed(&daemon);
    let mut quiet = daemon.connect();
    let _app = copy(&session, &[("text/plain", b"for all".as_slice())]);
    assert_eq!(one.next("clip_added"), two.next("clip_added"));
    quiet.ok(json!({"op": "hello"}));
    assert!(quiet.events.is_empty(), "{:?}", quiet.events);
}

// ---- access ----------------------------------------------------------

#[test]
fn the_socket_directory_is_private() {
    use std::os::unix::fs::PermissionsExt as _;
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let dir = daemon.socket.parent().unwrap();
    let mode = std::fs::metadata(dir).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
}

/// Another user, whom `sudo -n -u` may run commands as without a password:
/// set `HOARDER_TEST_INTRUDER` (CI does) to check the uid test for real.
fn intruder() -> Option<String> {
    let user = std::env::var("HOARDER_TEST_INTRUDER").ok()?;
    let works = std::process::Command::new("sudo")
        .args(["-n", "-u", &user, "true"])
        .status()
        .is_ok_and(|s| s.success());
    assert!(
        works,
        "HOARDER_TEST_INTRUDER={user}, but `sudo -n -u {user}` fails"
    );
    Some(user)
}

#[test]
fn connections_from_another_uid_are_refused() {
    use std::os::unix::fs::PermissionsExt as _;
    let Some(user) = intruder() else {
        eprintln!("skipped: set HOARDER_TEST_INTRUDER to a user sudo may run as");
        return;
    };
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    // Open every door but the uid check itself.
    let world = std::fs::Permissions::from_mode(0o777);
    std::fs::set_permissions(session.runtime_dir(), world.clone()).unwrap();
    std::fs::set_permissions(daemon.socket.parent().unwrap(), world.clone()).unwrap();
    std::fs::set_permissions(&daemon.socket, world).unwrap();

    let out = std::process::Command::new("sudo")
        .args(["-n", "-u", &user, "socat", "-t", "2", "-"])
        .arg(format!("UNIX-CONNECT:{}", daemon.socket.display()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"{\"v\":1,\"id\":1,\"op\":\"status\"}\n")?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(
        out.stdout.is_empty(),
        "another uid got an answer: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    // The same request from this uid is answered.
    assert!(daemon.connect().ok(json!({"op": "status"})).is_object());
}

// ---- docs/ipc.md -----------------------------------------------------

/// The shell examples in docs/ipc.md: each ```sh block, in order.
fn doc_examples() -> Vec<String> {
    let doc = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/ipc.md");
    let text = std::fs::read_to_string(doc).unwrap();
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        match (&mut current, line.trim_end()) {
            (None, "```sh") => current = Some(String::new()),
            (Some(_), "```") => blocks.push(current.take().unwrap()),
            (Some(block), line) => {
                block.push_str(line);
                block.push('\n');
            }
            (None, _) => {}
        }
    }
    blocks
}

fn have(tool: &str) -> bool {
    std::process::Command::new(tool)
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

#[test]
fn the_documented_examples_run() {
    let examples = doc_examples();
    assert!(examples.len() >= 10, "docs/ipc.md has its examples");
    if !have("socat") || !have("jq") {
        eprintln!("skipped: the examples need socat and jq");
        return;
    }
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session, &[]);
    let mut events = Wire::subscribed(&daemon);
    let _app = add(&session, &mut events, "an example Clip");
    for example in examples {
        let out = session
            .command("bash")
            .args(["-euo", "pipefail", "-c", &example])
            .current_dir(session.runtime_dir())
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "this example failed:\n{example}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !out.stdout.is_empty(),
            "this example printed nothing:\n{example}"
        );
    }
}
