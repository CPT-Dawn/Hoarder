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

/// Copy `offer` from a fresh app, which keeps serving it until dropped.
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

#[test]
fn history_restore_makes_a_clip_the_selection_again() {
    let session = Session::start_default().unwrap();
    let _daemon = Daemon::start(&session);
    let _old = copy(
        &session,
        &[
            ("text/html", b"<b>old</b>".as_slice()),
            ("text/plain", b"old"),
        ],
    );
    wait_until("the first Clip", || history(&session).len() == 1);
    let _new = copy(&session, &[("text/plain", b"new".as_slice())]);
    wait_until("the second Clip", || history(&session).len() == 2);
    let old = history(&session)[1]["id"].to_string();

    let out = hoarder(&session, &["history", "restore", &old]);

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(paste(&session, "text/html"), b"<b>old</b>");
    assert_eq!(paste(&session, "text/plain"), b"old");
    let clips = history(&session);
    assert_eq!(clips.len(), 2, "a Restore is not a new Clip: {clips:?}");
    assert_eq!(
        clips[0]["id"].to_string(),
        old,
        "restored Clip moves to the top"
    );
}

#[test]
fn history_is_readable_only_by_its_owner() {
    use std::os::unix::fs::PermissionsExt as _;
    let session = Session::start_default().unwrap();
    let _daemon = Daemon::start(&session);
    let big = vec![b'x'; 300 * 1024];
    let _app = copy(&session, &[("text/plain", big.as_slice())]);
    wait_until("the Clip", || history(&session).len() == 1);

    let mut files = 0;
    let mut dirs = vec![data_dir(&session)];
    while let Some(dir) = dirs.pop() {
        assert_eq!(mode(&dir), 0o700, "{}", dir.display());
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                assert_eq!(mode(&path), 0o600, "{}", path.display());
                files += 1;
            }
        }
    }
    // The database, its WAL files and the blob.
    assert!(files >= 2, "only {files} files");

    fn mode(path: &std::path::Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }
}

/// The files under `blobs/`.
fn blobs(session: &Session) -> Vec<PathBuf> {
    std::fs::read_dir(data_dir(session).join("blobs"))
        .map(|d| d.map(|e| e.unwrap().path()).collect())
        .unwrap_or_default()
}

/// `hoarder history get ARGS` stdout, which must succeed.
fn get(session: &Session, args: &[&str]) -> Vec<u8> {
    let out = hoarder(session, &[&["history", "get"], args].concat());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

#[test]
fn large_representations_are_blobs_and_small_ones_inline() {
    let session = Session::start_default().unwrap();
    let _daemon = Daemon::start(&session);
    // 300 KB of a non-repeating pattern; the threshold is 256 KB.
    let big: Vec<u8> = (0..300 * 1024u32).map(|i| (i * 7 % 251) as u8).collect();
    let _small = copy(&session, &[("text/plain", b"tiny".as_slice())]);
    wait_until("the small Clip", || history(&session).len() == 1);
    assert!(blobs(&session).is_empty(), "{:?}", blobs(&session));

    let _big = copy(
        &session,
        &[
            ("application/octet-stream", big.as_slice()),
            ("text/plain", b"a label"),
        ],
    );
    wait_until("the large Clip", || history(&session).len() == 2);

    assert_eq!(blobs(&session).len(), 1, "{:?}", blobs(&session));
    assert_eq!(std::fs::read(&blobs(&session)[0]).unwrap(), big);
    let id = history(&session)[0]["id"].to_string();
    assert_eq!(get(&session, &[&id]), big);
    assert_eq!(get(&session, &[&id, "--type", "text/plain"]), b"a label");
    let small = history(&session)[1]["id"].to_string();
    assert_eq!(get(&session, &[&small]), b"tiny");
}

#[test]
fn deleting_a_clip_removes_it_and_its_blob() {
    let session = Session::start_default().unwrap();
    let _daemon = Daemon::start(&session);
    let big = vec![7u8; 300 * 1024];
    let _app = copy(&session, &[("image/png", big.as_slice())]);
    wait_until("the Clip", || history(&session).len() == 1);
    let id = history(&session)[0]["id"].to_string();

    let out = hoarder(&session, &["history", "delete", &id]);

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(history(&session).is_empty());
    assert!(blobs(&session).is_empty(), "{:?}", blobs(&session));
    let again = hoarder(&session, &["history", "delete", &id]);
    assert_eq!(
        again.status.code(),
        Some(4),
        "deleting a missing Clip fails"
    );
}

#[test]
fn a_file_list_is_recorded_as_files() {
    let session = Session::start_default().unwrap();
    let _daemon = Daemon::start(&session);
    let uris = b"file:///home/me/notes.txt\r\nfile:///home/me/My%20Photo.png\r\n";
    let _app = copy(
        &session,
        &[
            ("text/uri-list", uris.as_slice()),
            ("text/plain", b"/home/me/notes.txt\n/home/me/My Photo.png"),
        ],
    );
    wait_until("the Clip", || history(&session).len() == 1);

    let clip = &history(&session)[0];
    assert_eq!(clip["kind"], "files");
    assert_eq!(clip["preview"], "notes.txt, My Photo.png");
    assert_eq!(
        clip["types"],
        serde_json::json!(["text/uri-list", "text/plain"])
    );
    let id = clip["id"].to_string();
    assert_eq!(get(&session, &[&id]), uris);
}

#[test]
fn a_gnome_file_copy_is_recorded_as_files() {
    let session = Session::start_default().unwrap();
    let _daemon = Daemon::start(&session);
    let copied = b"copy\nfile:///home/me/a.txt\nfile:///home/me/b%20c.pdf";
    let _app = copy(
        &session,
        &[("x-special/gnome-copied-files", copied.as_slice())],
    );
    wait_until("the Clip", || history(&session).len() == 1);

    let clip = &history(&session)[0];
    assert_eq!(clip["kind"], "files");
    assert_eq!(clip["preview"], "a.txt, b c.pdf");
}

#[test]
fn history_get_json_describes_the_clip() {
    let session = Session::start_default().unwrap();
    let _daemon = Daemon::start(&session);
    let _app = copy(&session, &[("text/plain", b"described".as_slice())]);
    wait_until("the Clip", || history(&session).len() == 1);
    let line = String::from_utf8(hoarder(&session, &["history", "list"]).stdout).unwrap();
    assert!(line.ends_with("\tdescribed\n"), "{line:?}");

    // A whole `list` line picks its Clip, as in `history list | dmenu`.
    let json: Value = serde_json::from_slice(&get(&session, &[line.trim_end(), "--json"])).unwrap();

    assert_eq!(json["mime"], "text/plain");
    assert_eq!(json["size"], 9);
    assert_eq!(json["clip"]["preview"], "described");
}

#[test]
fn a_secret_is_never_recorded() {
    let session = Session::start_default().unwrap();
    let _daemon = Daemon::start(&session);
    let _secret = copy(
        &session,
        &[
            ("text/plain", b"hunter2".as_slice()),
            ("x-kde-passwordManagerHint", b"secret"),
        ],
    );
    let _after = copy(&session, &[("text/plain", b"harmless".as_slice())]);
    wait_until("the harmless Clip", || !history(&session).is_empty());

    let clips = history(&session);
    assert_eq!(clips.len(), 1, "{clips:?}");
    assert_eq!(clips[0]["preview"], "harmless");
}

#[test]
fn history_outlives_the_daemon() {
    let session = Session::start_default().unwrap();
    let daemon = Daemon::start(&session);
    let _app = copy(&session, &[("image/png", PNG)]);
    wait_until("the Clip", || history(&session).len() == 1);
    let before = history(&session);
    drop(daemon);

    let _daemon = Daemon::start(&session);

    // The app still owns the clipboard, so the new Daemon sees the same
    // copy again and only bumps its last use.
    let without_at = |mut clips: Vec<Value>| {
        clips.iter_mut().for_each(|c| c["at"] = Value::Null);
        clips
    };
    assert_eq!(without_at(history(&session)), without_at(before.clone()));
    assert_eq!(before[0]["kind"], "image");
    assert_eq!(before[0]["preview"], "1×1");
}

/// A tiny but real PNG: 1×1, one red pixel.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8, 0xcf, 0xc0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xdd, 0x8d, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

/// Long answers (a big `list`) must arrive whole even when the Front end
/// reads them slower than the Daemon writes.
#[test]
fn answers_larger_than_the_socket_buffer_arrive_whole() {
    use std::io::{BufRead as _, BufReader, Write as _};
    let session = Session::start_default().unwrap();
    let _daemon = Daemon::start(&session);
    let mut stream =
        UnixStream::connect(session.runtime_dir().join("hoarder/hoarder.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let requests: String = (0..5000)
        .map(|id| format!("{{\"v\":1,\"id\":{id},\"op\":\"hello\"}}\n"))
        .collect();
    stream.write_all(requests.as_bytes()).unwrap();
    // Let the Daemon run into a full socket before anything is read.
    std::thread::sleep(Duration::from_millis(200));

    let mut reader = BufReader::new(stream);
    for id in 0..5000 {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["id"], id);
    }
}

#[test]
fn history_needs_the_daemon() {
    let session = Session::start_default().unwrap();

    let out = hoarder(&session, &["history", "list"]);

    assert_eq!(out.status.code(), Some(4));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Daemon is not running"), "{stderr}");
}
