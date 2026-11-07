//! Routing through the Daemon (PLAN.md §5.2): with the Daemon up, `copy` and
//! `shot` hand their data over the socket and leave nothing running behind,
//! Captures become image Clips with thumbnails, and `paste --watch` runs off
//! the Daemon's event stream. Observed through the CLI, wl-clipboard and /proc.

use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use hoarder_e2e::{OutputSpec, Pattern, Session};
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

/// A tag no other test uses, put in the environment of the commands under
/// test so their descendants can be found in /proc.
fn marker(session: &Session) -> String {
    format!("HOARDER_TEST_MARK={}", session.runtime_dir().display())
}

/// Processes still alive whose environment carries `marker`.
fn marked_processes(marker: &str) -> Vec<String> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Ok(environ) = std::fs::read(entry.path().join("environ")) else {
            continue;
        };
        if environ.split(|b| *b == 0).any(|v| v == marker.as_bytes()) {
            let cmdline = std::fs::read(entry.path().join("cmdline")).unwrap_or_default();
            found.push(String::from_utf8_lossy(&cmdline).replace('\0', " "));
        }
    }
    found
}

/// `cmd` run to completion with its marker set, its output in files (a
/// lingering child holding a pipe would otherwise stall the read).
fn run_marked(session: &Session, mut cmd: Command, stdin: Option<&[u8]>) -> Output {
    let (key, value) = marker(session)
        .split_once('=')
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .unwrap();
    let out = session.runtime_dir().join("marked.stdout");
    let err = session.runtime_dir().join("marked.stderr");
    cmd.env(key, value)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(std::fs::File::create(&out).unwrap())
        .stderr(std::fs::File::create(&err).unwrap());
    let mut child = cmd.spawn().unwrap();
    if let Some(data) = stdin {
        use std::io::Write as _;
        child.stdin.take().unwrap().write_all(data).unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "command did not finish");
        std::thread::sleep(Duration::from_millis(5));
    };
    Output {
        status,
        stdout: std::fs::read(out).unwrap(),
        stderr: std::fs::read(err).unwrap(),
    }
}

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
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

fn wl_paste(session: &Session, args: &[&str]) -> (Option<i32>, Vec<u8>) {
    let out = session
        .command("wl-paste")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (out.status.code(), out.stdout)
}

#[test]
fn copy_through_the_daemon_leaves_nothing_running_and_is_recorded() {
    let session = Session::start_default().unwrap();
    let _daemon = Daemon::start(&session);

    let mut copy = session.command(HOARDER);
    copy.arg("copy");
    let out = run_marked(&session, copy, Some(b"handed over"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(marked_processes(&marker(&session)), Vec::<String>::new());

    assert_eq!(
        wl_paste(&session, &["-n"]),
        (Some(0), b"handed over".to_vec())
    );
    let clips = history(&session);
    assert_eq!(clips.len(), 1, "{clips:?}");
    assert_eq!(clips[0]["preview"], "handed over");
}

/// Width, height and colour type of a PNG.
fn png_info(bytes: &[u8]) -> (u32, u32) {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let reader = decoder.read_info().expect("a valid PNG");
    let info = reader.info();
    (info.width, info.height)
}

#[test]
fn a_capture_is_an_image_clip_with_a_thumbnail_as_soon_as_shot_exits() {
    let session = Session::start(&[OutputSpec::new(640, 400)]).unwrap();
    let painter = session.client().unwrap();
    painter
        .paint(session.output("HEADLESS-1"), &Pattern::Quadrants)
        .unwrap();
    let _daemon = Daemon::start(&session);

    let mut shot = session.command(HOARDER);
    shot.args(["shot", "all", "--no-save", "--no-notify"]);
    let out = run_marked(&session, shot, None);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(!stderr.contains("History is off"), "{stderr}");
    assert_eq!(marked_processes(&marker(&session)), Vec::<String>::new());

    // No waiting: the Clip is there the moment `shot` has returned.
    let clips = history(&session);
    assert_eq!(clips.len(), 1, "{clips:?}");
    assert_eq!(clips[0]["kind"], "image");
    assert_eq!(clips[0]["preview"], "640×400");
    assert_eq!(clips[0]["types"], serde_json::json!(["image/png"]));

    let (code, pasted) = wl_paste(&session, &["-t", "image/png"]);
    assert_eq!(code, Some(0));
    assert_eq!(png_info(&pasted), (640, 400));

    let id = clips[0]["id"].to_string();
    let thumb = hoarder(&session, &["history", "get", &id, "--thumb"]);
    assert!(
        thumb.status.success(),
        "{}",
        String::from_utf8_lossy(&thumb.stderr)
    );
    assert_eq!(png_info(&thumb.stdout), (256, 160));
}

/// Start `paste --watch` as `program` (plus `args`), logging one line per
/// run of its command to `name.log`: `$CLIPBOARD_STATE` and the bytes it
/// got. The command's parent pid goes to `name.ppid`.
fn start_watcher(session: &Session, name: &str, mut cmd: Command) -> (Child, std::path::PathBuf) {
    let log = session.runtime_dir().join(format!("{name}.log"));
    let ppid = session.runtime_dir().join(format!("{name}.ppid"));
    let script = format!(
        "echo $PPID >> {1}; printf '%s ' \"$CLIPBOARD_STATE\" >> {0}; \
         od -An -c | tr -s ' \\n' ' ' >> {0}; echo >> {0}",
        log.display(),
        ppid.display()
    );
    let child = cmd
        .args(["--watch", "sh", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn watcher");
    (child, log)
}

fn lines(path: &std::path::Path) -> usize {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .matches('\n')
        .count()
}

#[test]
fn paste_watch_runs_off_the_daemons_event_stream() {
    let session = Session::start_default().unwrap();
    let _daemon = Daemon::start(&session);

    let (mut reference, reference_log) =
        start_watcher(&session, "wl-paste", session.command("wl-paste"));
    let mut ours = session.command(HOARDER);
    // Without data control of its own, only the Daemon can tell it anything.
    ours.arg("paste").env(
        "HOARDER_HIDE_GLOBALS",
        "ext_data_control_manager_v1,zwlr_data_control_manager_v1",
    );
    let (mut watcher, log) = start_watcher(&session, "hoarder", ours);
    wait_until("the first runs", || {
        lines(&reference_log) >= 1 && lines(&log) >= 1
    });

    let changes: [&[&str]; 4] = [
        &["one"],
        &["-t", "image/png", "img"],
        &["-c"],
        &["--sensitive", "pw"],
    ];
    for args in changes {
        let before = lines(&reference_log);
        // Not `output()`: wl-copy's fork would hold the pipes open.
        let status = session
            .command("wl-copy")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        wait_until("wl-paste to see the change", || {
            lines(&reference_log) > before
        });
    }
    // Let a Takeover after the clear settle, then compare.
    std::thread::sleep(Duration::from_millis(300));
    let expected = std::fs::read_to_string(&reference_log).unwrap();
    wait_until("hoarder to see what wl-paste saw", || {
        std::fs::read_to_string(&log).unwrap_or_default() == expected
    });

    // Every run of the command is a child of the watcher itself.
    let ppids = std::fs::read_to_string(session.runtime_dir().join("hoarder.ppid")).unwrap();
    for ppid in ppids.lines() {
        assert_eq!(ppid, watcher.id().to_string(), "{ppids}");
    }
    for child in [&mut watcher, &mut reference] {
        let _ = child.kill();
        let _ = child.wait();
    }
}
