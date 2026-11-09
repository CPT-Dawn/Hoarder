//! `hoarder copy` / `hoarder paste` against wl-clipboard: the same cases run
//! with both tools, in every pairing, and must agree byte for byte. Hoarder
//! plays its part as `hoarder copy`/`paste` and as the Compat shims (a
//! `wl-copy`/`wl-paste` symlink to `hoarder`), with and without the Daemon.

use std::fs::File;
use std::io::Write as _;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use hoarder_e2e::Session;

const HOARDER: &str = env!("CARGO_BIN_EXE_hoarder");

/// Which program plays wl-copy or wl-paste.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    WlClipboard,
    Hoarder,
    /// `hoarder` behind a symlink named `wl-copy` or `wl-paste`.
    Shim,
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
            Tool::Shim => session.command(shim(session, "wl-copy")),
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
            Tool::Shim => session.command(shim(session, "wl-paste")),
        }
    }
}

/// A symlink to `hoarder` called `name`, as `hoarder-wl-compat` installs.
fn shim(session: &Session, name: &str) -> PathBuf {
    let dir = session.runtime_dir().join("shims");
    let link = dir.join(name);
    if !link.exists() {
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(HOARDER, &link).unwrap();
    }
    link
}

const PAIRS: [(Tool, Tool); 7] = [
    (Tool::WlClipboard, Tool::WlClipboard),
    (Tool::Hoarder, Tool::WlClipboard),
    (Tool::WlClipboard, Tool::Hoarder),
    (Tool::Hoarder, Tool::Hoarder),
    (Tool::Shim, Tool::WlClipboard),
    (Tool::WlClipboard, Tool::Shim),
    (Tool::Shim, Tool::Shim),
];

/// A running `hoarder daemon`, killed when dropped.
struct Daemon(Child);

impl Daemon {
    fn start(session: &Session) -> Daemon {
        let child = session
            .command(HOARDER)
            .arg("daemon")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start hoarder daemon");
        let socket = session.runtime_dir().join("hoarder/hoarder.sock");
        let deadline = Instant::now() + Duration::from_secs(10);
        while UnixStream::connect(&socket).is_err() {
            assert!(Instant::now() < deadline, "daemon socket never appeared");
            std::thread::sleep(Duration::from_millis(20));
        }
        Daemon(child)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

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
        let _ = pipe.write_all(data); // a usage error exits before reading
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
    // Through the Daemon when it runs: a clipboard wl-copy cleared would be
    // restored by Takeover.
    for primary in [false, true] {
        let mut clear = Tool::Hoarder.copy(session);
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
    for (name, steps) in CASES {
        assert_parity(&session, name, steps);
    }
}

/// With the Daemon serving what Hoarder copies, every case still behaves
/// as wl-copy → wl-paste does without one.
#[test]
fn copying_through_the_daemon_agrees_with_wl_clipboard() {
    let session = Session::start_default().unwrap();
    let reference: Vec<Vec<Outcome>> = CASES
        .iter()
        .map(|(_, steps)| play(&session, steps, Tool::WlClipboard, Tool::WlClipboard))
        .collect();
    let _daemon = Daemon::start(&session);
    for ((name, steps), reference) in CASES.iter().zip(reference) {
        for copier in [Tool::Hoarder, Tool::Shim] {
            for paster in [Tool::WlClipboard, Tool::Hoarder, Tool::Shim] {
                let got = play(&session, steps, copier, paster);
                assert_eq!(
                    got, reference,
                    "case {name:?}: {copier:?} copy → {paster:?} paste with the Daemon up \
                     disagrees with wl-clipboard"
                );
            }
        }
    }
}

const CASES: &[(&str, &[Step])] = &[
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

/// Wait until `path` holds `lines` complete (newline-terminated) lines, then
/// return its text. A line still being written does not count, or the
/// watcher's command could be caught halfway through its last line.
fn wait_for_lines(path: &std::path::Path, lines: usize) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        if text.matches('\n').count() >= lines {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "watcher wrote only {text:?}, wanted {lines} lines"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn wait_for_lines_waits_out_a_half_written_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log");
    std::fs::write(&path, "one\ntwo").unwrap();
    let writer = {
        let path = path.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let mut log = std::fs::OpenOptions::new().append(true).open(path).unwrap();
            log.write_all(b"\n").unwrap();
        })
    };
    assert_eq!(wait_for_lines(&path, 2), "one\ntwo\n");
    writer.join().unwrap();
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
    assert_eq!(watch_log(&session, Tool::Shim), reference);
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
    foreground_serves_until_replaced(&session, Tool::Shim);
}

#[test]
fn copy_and_paste_exit_3_without_data_control() {
    let session = Session::start_default().unwrap();
    for (tool, args) in [
        ("copy", &["x"][..]),
        ("paste", &[][..]),
        ("copy", &["-f", "x"]),
    ] {
        let mut cmd = session.command(HOARDER);
        cmd.arg(tool).args(args).env(
            "HOARDER_HIDE_GLOBALS",
            "ext_data_control_manager_v1,zwlr_data_control_manager_v1",
        );
        let out = run(&session, cmd, None);
        assert_eq!(
            out.status.code(),
            Some(3),
            "{tool} {args:?}: {}",
            out.stderr
        );
        assert!(out.stderr.contains("ext-data-control"), "{}", out.stderr);
    }
}

/// Without ext-data-control (Wayfire, older wlroots), `copy` and `paste`
/// speak wlr-data-control, with no Daemon to lean on.
#[test]
fn copy_and_paste_fall_back_to_wlr_data_control() {
    let session = Session::start_default().unwrap();
    let wlr_only = |mut cmd: Command| {
        cmd.env("HOARDER_HIDE_GLOBALS", "ext_data_control_manager_v1");
        cmd
    };
    for primary in [false, true] {
        let flag: &[&str] = if primary { &["--primary"] } else { &[] };
        let mut copy = wlr_only(Tool::Hoarder.copy(&session));
        copy.args(flag).args(["--type", "text/x-hoard", "hello"]);
        let copied = run(&session, copy, None);
        assert!(copied.status.success(), "copy: {}", copied.stderr);

        let mut reference = session.command("wl-paste");
        reference.args(flag).arg("--no-newline");
        assert_eq!(run(&session, reference, None).stdout, b"hello");

        let mut list = wlr_only(Tool::Hoarder.paste(&session));
        list.args(flag).arg("--list-types");
        let types = run(&session, list, None).stdout;
        assert!(types.starts_with(b"text/x-hoard\n"), "{types:?}");
        let mut paste = wlr_only(Tool::Hoarder.paste(&session));
        paste.args(flag).arg("--no-newline");
        let pasted = run(&session, paste, None);
        assert_eq!(pasted.stdout, b"hello", "paste: {}", pasted.stderr);

        let mut clear = wlr_only(Tool::Hoarder.copy(&session));
        clear.args(flag).arg("--clear");
        assert!(run(&session, clear, None).status.success());
        let mut gone = session.command("wl-paste");
        gone.args(flag);
        assert!(!run(&session, gone, None).status.success(), "cleared");
    }
}

/// What a copy with odd arguments observably did: whether it succeeded, and
/// what both selections held afterwards.
#[derive(Debug, PartialEq, Eq)]
struct CopyEffect {
    ok: bool,
    clipboard: Option<Vec<u8>>,
    primary: Option<Vec<u8>>,
}

fn copy_effect(session: &Session, tool: Tool, args: &[&str], stdin: Option<&[u8]>) -> CopyEffect {
    for primary in [false, true] {
        let mut clear = Tool::Hoarder.copy(session);
        clear.arg("--clear");
        if primary {
            clear.arg("--primary");
        }
        assert!(run(session, clear, None).status.success());
    }
    let mut cmd = tool.copy(session);
    cmd.args(args);
    let ok = run(session, cmd, stdin).status.success();
    let held = |primary: bool| {
        let mut cmd = Tool::WlClipboard.paste(session);
        cmd.arg("--no-newline");
        if primary {
            cmd.arg("--primary");
        }
        let out = run(session, cmd, None);
        out.status.success().then_some(out.stdout)
    };
    CopyEffect {
        ok,
        clipboard: held(false),
        primary: held(true),
    }
}

/// Copy argument lists wl-copy treats in its own way: unknown options are a
/// usage error, options may follow the text, `--` ends options.
type OddCase = (&'static str, &'static [&'static str], Option<&'static [u8]>);
const ODD_ARGS: &[OddCase] = &[
    ("unknown long option", &["--bogus"], Some(b"in")),
    ("unknown short option", &["-z", "text"], Some(b"in")),
    ("typo of --sensitive", &["--sensitve"], Some(b"secret")),
    ("option after text", &["hello", "-n"], None),
    ("text after option", &["-n", "hello"], None),
    ("option between text", &["a", "-p", "b"], None),
    ("dash dash ends options", &["--", "-v"], None),
    ("dash dash then flag-like", &["--", "--primary", "x"], None),
    ("lone dash", &["-"], None),
    ("flag after dash dash is text", &["hello", "--", "-n"], None),
    ("abbreviated long option", &["--prim", "x"], None),
    ("abbreviated clear", &["--cle"], None),
    ("value attached", &["-ttext/x-a", "hello"], None),
    ("bundled shorts", &["-pn", "hello"], None),
];

#[test]
fn copy_parses_options_like_wl_copy() {
    let session = Session::start_default().unwrap();
    for (name, args, stdin) in ODD_ARGS {
        let reference = copy_effect(&session, Tool::WlClipboard, args, *stdin);
        for tool in [Tool::Hoarder, Tool::Shim] {
            let got = copy_effect(&session, tool, args, *stdin);
            assert_eq!(
                got, reference,
                "case {name:?} {args:?}: {tool:?} disagrees with wl-copy"
            );
        }
    }
}

/// `-v`/`--version` print a version and succeed in wl-copy and wl-paste, so
/// the shims must too.
#[test]
fn shims_print_a_version_for_dash_v() {
    let session = Session::start_default().unwrap();
    for name in ["wl-copy", "wl-paste"] {
        for flag in ["-v", "--version"] {
            let mut real = session.command(name);
            real.arg(flag);
            assert!(run(&session, real, None).status.success(), "{name} {flag}");
            let mut cmd = session.command(shim(&session, name));
            cmd.arg(flag);
            let got = run(&session, cmd, None);
            assert!(got.status.success(), "{name} {flag}: {}", got.stderr);
            assert!(!got.stdout.is_empty(), "{name} {flag} printed nothing");
        }
    }
}

/// wl-paste rejects what it does not know, too.
#[test]
fn paste_rejects_unknown_options_like_wl_paste() {
    let session = Session::start_default().unwrap();
    let mut real = Tool::WlClipboard.paste(&session);
    real.arg("--bogus");
    let want = run(&session, real, None).status.code();
    assert_ne!(want, Some(0));
    for tool in [Tool::Hoarder, Tool::Shim] {
        let mut cmd = tool.paste(&session);
        cmd.arg("--bogus");
        let out = run(&session, cmd, None);
        assert!(
            !out.status.success() && out.stdout.is_empty(),
            "{tool:?}: {}",
            out.stderr
        );
    }
}
