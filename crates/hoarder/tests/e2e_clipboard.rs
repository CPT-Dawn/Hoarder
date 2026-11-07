//! `hoarder copy` / `hoarder paste` against wl-clipboard: the same cases run
//! with both tools, in every pairing, and must agree byte for byte.

use std::fs::File;
use std::io::Write as _;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use hoarder_e2e::Session;

const HOARDER: &str = env!("CARGO_BIN_EXE_hoarder");

/// Which program plays wl-copy or wl-paste.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    WlClipboard,
    Hoarder,
}

impl Tool {
    fn copy(self, session: &Session) -> Command {
        match self {
            Tool::WlClipboard => session.command("wl-copy"),
            Tool::Hoarder => {
                let mut cmd = session.command(HOARDER);
                cmd.arg("copy");
                cmd
            }
        }
    }

    fn paste(self, session: &Session) -> Command {
        match self {
            Tool::WlClipboard => session.command("wl-paste"),
            Tool::Hoarder => {
                let mut cmd = session.command(HOARDER);
                cmd.arg("paste");
                cmd
            }
        }
    }
}

const PAIRS: [(Tool, Tool); 4] = [
    (Tool::WlClipboard, Tool::WlClipboard),
    (Tool::Hoarder, Tool::WlClipboard),
    (Tool::WlClipboard, Tool::Hoarder),
    (Tool::Hoarder, Tool::Hoarder),
];

/// One step of a case: a copy or a paste, with its arguments.
#[derive(Debug, Clone)]
enum Step {
    /// Copy with these args, feeding `stdin` (or nothing, closed, if `None`).
    Copy(&'static [&'static str], Option<&'static [u8]>),
    /// Paste with these args; its outcome is what gets compared.
    Paste(&'static [&'static str]),
}

use Step::{Copy, Paste};

/// What a paste observably did.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Outcome {
    code: Option<i32>,
    stdout: Vec<u8>,
}

/// What a finished command did.
#[derive(Debug)]
struct Ran {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: String,
}

/// Run `cmd` to completion. Its output goes to files, not pipes: wl-copy's
/// forked child keeps inherited stdio open, which would block a pipe reader.
fn run(session: &Session, mut cmd: Command, stdin: Option<&[u8]>) -> Ran {
    let out_path = session.runtime_dir().join("cmd.stdout");
    let err_path = session.runtime_dir().join("cmd.stderr");
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(File::create(&out_path).expect("stdout file"))
    .stderr(File::create(&err_path).expect("stderr file"));
    let mut child = cmd.spawn().expect("spawn");
    if let Some(data) = stdin {
        let mut pipe = child.stdin.take().expect("piped stdin");
        pipe.write_all(data).expect("write stdin");
    }
    let status = wait_with_timeout(&mut child, Duration::from_secs(10));
    Ran {
        status,
        stdout: std::fs::read(&out_path).unwrap_or_default(),
        stderr: std::fs::read_to_string(&err_path).unwrap_or_default(),
    }
}

fn wait_with_timeout(child: &mut Child, limit: Duration) -> ExitStatus {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return status;
        }
        if Instant::now() > deadline {
            child.kill().ok();
            panic!("command did not finish within {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Run `steps` with `copier` and `paster`, starting from an empty clipboard.
fn play(session: &Session, steps: &[Step], copier: Tool, paster: Tool) -> Vec<Outcome> {
    for primary in [false, true] {
        let mut clear = session.command("wl-copy");
        clear.arg("--clear");
        if primary {
            clear.arg("--primary");
        }
        assert!(run(session, clear, None).status.success());
    }
    let mut outcomes = Vec::new();
    for step in steps {
        match step {
            Copy(args, stdin) => {
                let mut cmd = copier.copy(session);
                cmd.args(*args);
                let out = run(session, cmd, *stdin);
                assert!(
                    out.status.success(),
                    "{copier:?} copy {args:?} failed: {}",
                    out.stderr
                );
            }
            Paste(args) => {
                let mut cmd = paster.paste(session);
                cmd.args(*args);
                let out = run(session, cmd, None);
                outcomes.push(Outcome {
                    code: out.status.code(),
                    stdout: out.stdout,
                });
            }
        }
    }
    outcomes
}

/// Every pairing of copier and paster must match wl-copy → wl-paste.
fn assert_parity(session: &Session, name: &str, steps: &[Step]) {
    let reference = play(session, steps, Tool::WlClipboard, Tool::WlClipboard);
    for (copier, paster) in &PAIRS[1..] {
        let got = play(session, steps, *copier, *paster);
        assert_eq!(
            got, reference,
            "case {name:?}: {copier:?} copy → {paster:?} paste disagrees with wl-clipboard"
        );
    }
}

#[test]
fn copy_and_paste_agree_with_wl_clipboard() {
    let session = Session::start_default().unwrap();
    let cases: &[(&str, &[Step])] = &[
        ("nothing copied", &[Paste(&[]), Paste(&["--list-types"])]),
        (
            "text from arguments",
            &[
                Copy(&["hello", "world"], None),
                Paste(&[]),
                Paste(&["-l"]),
                Paste(&["-n"]),
            ],
        ),
        (
            "text from stdin keeps its newline",
            &[
                Copy(&[], Some(b"line\n")),
                Paste(&[]),
                Paste(&["--no-newline"]),
            ],
        ),
        (
            "trim newline",
            &[Copy(&["-n"], Some(b"line\n\n")), Paste(&["-n"])],
        ),
        (
            "empty stdin",
            &[Copy(&[], Some(b"")), Paste(&[]), Paste(&["-l"])],
        ),
        (
            "png is sniffed and pasted raw",
            &[
                Copy(&[], Some(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR")),
                Paste(&["-l"]),
                Paste(&[]),
                Paste(&["-t", "image"]),
                Paste(&["-t", "text"]),
            ],
        ),
        (
            "binary is an octet stream",
            &[
                Copy(&[], Some(b"\x00\x01\xfe\xffbinary\x00")),
                Paste(&["-l"]),
                Paste(&[]),
            ],
        ),
        (
            "explicit non-text type",
            &[
                Copy(&["-t", "foo/bar"], Some(b"xyz")),
                Paste(&["-l"]),
                Paste(&[]),
                Paste(&["-t", "foo/bar"]),
                Paste(&["-t", "text"]),
            ],
        ),
        (
            "explicit text type adds aliases",
            &[
                Copy(&["--type", "text/html"], Some(b"<b>x</b>")),
                Paste(&["-l"]),
                Paste(&["-t", "text"]),
                Paste(&["-t", "STRING"]),
                Paste(&["-t", "text/html"]),
                Paste(&["-t", "html"]),
                Paste(&["-t", "image/png"]),
            ],
        ),
        (
            "sensitive",
            &[
                Copy(&["--sensitive", "hunter2"], None),
                Paste(&["-l"]),
                Paste(&[]),
            ],
        ),
        (
            "clear",
            &[
                Copy(&["gone"], None),
                Copy(&["-c"], None),
                Paste(&[]),
                Paste(&["-l"]),
            ],
        ),
        (
            "primary is separate",
            &[
                Copy(&["-p", "prim"], None),
                Paste(&["-p"]),
                Paste(&[]),
                Copy(&["clip"], None),
                Paste(&["--primary"]),
                Copy(&["--primary", "--clear"], None),
                Paste(&["-p"]),
                Paste(&[]),
            ],
        ),
        (
            "paste once",
            &[Copy(&["-o", "once"], None), Paste(&[]), Paste(&[])],
        ),
    ];
    for (name, steps) in cases {
        assert_parity(&session, name, steps);
    }
}

/// Wait until `path` holds `lines` lines, then return its text.
fn wait_for_lines(path: &std::path::Path, lines: usize) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        if text.lines().count() >= lines {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "watcher wrote only {text:?}, wanted {lines} lines"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// What `paste --watch` saw: one line per run of its command, holding
/// `$CLIPBOARD_STATE` and the bytes it got on stdin.
fn watch_log(session: &Session, paster: Tool) -> String {
    let log = session.runtime_dir().join(format!("watch-{paster:?}.log"));
    let mut clear = session.command("wl-copy");
    clear.arg("-c");
    assert!(run(session, clear, None).status.success());

    let script = format!(
        "printf '%s ' \"$CLIPBOARD_STATE\" >> {0}; od -An -c | tr -s ' \\n' ' ' >> {0}; echo >> {0}",
        log.display()
    );
    let mut watcher = paster
        .paste(session)
        .args(["--watch", "sh", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn watcher");
    let mut expected = 1;
    wait_for_lines(&log, expected);
    let changes: [&[&str]; 4] = [
        &["one"],
        &["-t", "image/png", "img"],
        &["-c"],
        &["--sensitive", "pw"],
    ];
    for args in changes {
        let mut cmd = session.command("wl-copy");
        cmd.args(args);
        assert!(run(session, cmd, None).status.success());
        expected += 1;
        wait_for_lines(&log, expected);
    }
    watcher.kill().ok();
    watcher.wait().ok();
    std::fs::read_to_string(&log).unwrap()
}

#[test]
fn paste_watch_agrees_with_wl_paste() {
    let session = Session::start_default().unwrap();
    let reference = watch_log(&session, Tool::WlClipboard);
    assert_eq!(watch_log(&session, Tool::Hoarder), reference);
}

/// `copy --foreground` serves from the process itself until replaced.
fn foreground_serves_until_replaced(session: &Session, copier: Tool) {
    let mut child = copier
        .copy(session)
        .args(["--foreground", "fg"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn copy");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let out = run(session, session.command("wl-paste"), None);
        if out.stdout == b"fg\n" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "{copier:?} --foreground never served"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        child.try_wait().unwrap().is_none(),
        "{copier:?} --foreground returned early"
    );
    let mut replace = session.command("wl-copy");
    replace.arg("next");
    assert!(run(session, replace, None).status.success());
    let status = wait_with_timeout(&mut child, Duration::from_secs(10));
    assert!(
        status.success(),
        "{copier:?} --foreground exited with {status}"
    );
}

#[test]
fn copy_foreground_agrees_with_wl_copy() {
    let session = Session::start_default().unwrap();
    foreground_serves_until_replaced(&session, Tool::WlClipboard);
    foreground_serves_until_replaced(&session, Tool::Hoarder);
}
