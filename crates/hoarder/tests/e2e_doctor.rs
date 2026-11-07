//! `hoarder doctor`: what Hoarder found, which paths it chose, and every
//! problem with a fix. A stand-in `systemctl` keeps the real user manager out
//! of reach.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use hoarder_e2e::Session;

const HOARDER: &str = env!("CARGO_BIN_EXE_hoarder");

/// A `systemctl` stand-in. `is-enabled` prints `state`; `show-environment`
/// prints `environment`.
fn fake_systemctl(session: &Session, state: &str, environment: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let bin = session.runtime_dir().join("fakebin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join("systemctl");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ncase \"$*\" in\n  *is-enabled*) echo {state}; [ {state} = enabled ];;\n  *show-environment*) printf '{environment}';;\nesac\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

fn hoarder(session: &Session, bin: &Path) -> Command {
    let mut cmd = session.command(HOARDER);
    let path = std::env::var("PATH").unwrap_or_default();
    cmd.env("PATH", format!("{}:{path}", bin.display()))
        .stdin(Stdio::null());
    cmd
}

fn doctor(session: &Session, bin: &Path, tweak: impl FnOnce(&mut Command)) -> (Output, String) {
    let mut cmd = hoarder(session, bin);
    cmd.arg("doctor");
    tweak(&mut cmd);
    let out = cmd.output().expect("hoarder doctor should run");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    (out, text)
}

/// The value doctor reports on the line starting with `label`.
fn fact<'a>(report: &'a str, label: &str) -> &'a str {
    report
        .lines()
        .find_map(|l| l.strip_prefix(label).filter(|r| r.starts_with(' ')))
        .unwrap_or_else(|| panic!("no {label:?} line in:\n{report}"))
        .trim()
}

/// The reported problems as (what, fix), checking each one has a fix.
fn problems(report: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = report.lines().collect();
    lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| l.strip_prefix("  ! ").map(|what| (i, what)))
        .map(|(i, what)| {
            let fix = lines
                .get(i + 1)
                .and_then(|l| l.strip_prefix("    fix: "))
                .filter(|f| !f.trim().is_empty())
                .unwrap_or_else(|| panic!("problem {what:?} has no fix in:\n{report}"));
            (what.to_owned(), fix.to_owned())
        })
        .collect()
}

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
            .unwrap();
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

#[test]
fn doctor_offers_the_service_when_the_daemon_is_not_running() {
    let session = Session::start_default().unwrap();
    let bin = fake_systemctl(&session, "disabled", "");

    let (out, report) = doctor(&session, &bin, |_| {});

    assert_eq!(out.status.code(), Some(1), "{report}");
    assert_eq!(fact(&report, "service"), "disabled");
    assert_eq!(fact(&report, "daemon"), "not running");
    let problems = problems(&report);
    assert!(
        problems
            .iter()
            .any(|(_, fix)| fix.contains("hoarder service enable")),
        "{report}"
    );
}

#[test]
fn doctor_wants_the_session_exported_to_the_user_manager() {
    let session = Session::start_default().unwrap();
    let bin = fake_systemctl(&session, "enabled", "HOME=/home/someone\\n");
    let _daemon = Daemon::start(&session);

    let (_, report) = doctor(&session, &bin, |_| {});

    assert_eq!(fact(&report, "service"), "enabled");
    assert!(
        fact(&report, "daemon").starts_with("running (pid "),
        "{report}"
    );
    let problems = problems(&report);
    assert!(
        problems
            .iter()
            .any(|(what, fix)| what.contains("WAYLAND_DISPLAY")
                && fix.contains("hoarder setup sway --apply")),
        "{report}"
    );
}

#[test]
fn doctor_is_happy_with_a_healthy_session() {
    let session = Session::start_default().unwrap();
    let env = format!("WAYLAND_DISPLAY={}\\n", session.wayland_display());
    let bin = fake_systemctl(&session, "enabled", &env);
    let _daemon = Daemon::start(&session);

    let (out, report) = doctor(&session, &bin, |_| {});

    assert_eq!(problems(&report), [], "{report}");
    assert!(out.status.success(), "{report}");
    assert!(report.contains("No problems found."), "{report}");
}

/// A child process killed when dropped.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn(mut cmd: Command) -> Running {
    Running(
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    )
}

#[test]
fn doctor_names_conflicting_clipboard_tools_in_this_session() {
    let session = Session::start_default().unwrap();
    let env = format!("WAYLAND_DISPLAY={}\\n", session.wayland_display());
    let bin = fake_systemctl(&session, "enabled", &env);
    let _daemon = Daemon::start(&session);
    // Stand-ins named like the real tools.
    std::os::unix::fs::symlink(which("sleep"), bin.join("wl-clip-persist")).unwrap();
    std::os::unix::fs::symlink(which("cat"), bin.join("cliphist")).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let in_session = |program: &str, args: &[&str]| {
        let mut cmd = session.command(program);
        cmd.env("PATH", &path).args(args);
        spawn(cmd)
    };
    let _persist = in_session("wl-clip-persist", &["60"]);
    let _cliphist = in_session("wl-paste", &["--watch", "cliphist", "store"]);
    let _watcher = in_session("wl-paste", &["-p", "--watch", "cat"]);
    // The same tool in another session is not this session's problem.
    let mut elsewhere = Command::new(bin.join("wl-clip-persist"));
    elsewhere
        .env_clear()
        .env("XDG_RUNTIME_DIR", "/nonexistent")
        .env("WAYLAND_DISPLAY", session.wayland_display())
        .arg("60");
    let elsewhere = spawn(elsewhere);

    let (out, report) = doctor(&session, &bin, |_| {});

    assert_eq!(out.status.code(), Some(1), "{report}");
    let problems = problems(&report);
    let about = |needle: &str| {
        problems
            .iter()
            .filter(|(what, _)| what.contains(needle))
            .collect::<Vec<_>>()
    };
    assert_eq!(about("wl-clip-persist").len(), 1, "{report}");
    assert!(
        !about(&format!("pid {}", elsewhere.0.id()))
            .iter()
            .any(|_| true),
        "{report}"
    );
    assert_eq!(about("cliphist").len(), 1, "{report}");
    assert!(
        about("cliphist")[0].1.contains("hoarder history"),
        "{report}"
    );
    let watchers = about("wl-paste");
    assert_eq!(watchers.len(), 2, "{report}");
    assert!(
        watchers.iter().any(|(_, fix)| fix.contains("on_clip")),
        "{report}"
    );
}

fn which(program: &str) -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|d| d.join(program))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("{program} is not on PATH"))
}

#[test]
fn doctor_reports_the_compositor_and_the_paths_it_chose() {
    let session = Session::start_default().unwrap();
    let bin = fake_systemctl(&session, "disabled", "");

    let (_, report) = doctor(&session, &bin, |_| {});

    assert_eq!(fact(&report, "compositor"), "Sway");
    assert_eq!(fact(&report, "capture"), "ext-image-copy-capture-v1");
    assert_eq!(fact(&report, "clipboard"), "ext-data-control-v1");
    assert_eq!(fact(&report, "windows"), "Sway IPC");
    assert_eq!(
        fact(&report, "config"),
        session
            .runtime_dir()
            .join("home/.config/hoarder/config.toml")
            .to_str()
            .unwrap()
    );
}

#[test]
fn doctor_reports_the_fallbacks_when_protocols_are_missing() {
    let session = Session::start_default().unwrap();
    let bin = fake_systemctl(&session, "disabled", "");

    let (_, report) = doctor(&session, &bin, |cmd| {
        cmd.env(
            "HOARDER_HIDE_GLOBALS",
            "ext_image_copy_capture_manager_v1,ext_data_control_manager_v1",
        );
    });

    assert_eq!(fact(&report, "capture"), "wlr-screencopy-unstable-v1");
    assert_eq!(fact(&report, "clipboard"), "wlr-data-control-unstable-v1");
}
