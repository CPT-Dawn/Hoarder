//! `hoarder config get|set|edit|reset|check|path`, and live reload.

use std::io::BufRead as _;
use std::path::PathBuf;
use std::process::{Child, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use hoarder_e2e::Session;

fn hoarder(session: &Session, args: &[&str]) -> Output {
    session
        .command(env!("CARGO_BIN_EXE_hoarder"))
        .args(args)
        .output()
        .expect("hoarder should run")
}

fn stdout(out: &Output) -> String {
    assert!(
        out.status.success(),
        "hoarder failed ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout.clone()).unwrap()
}

fn config_path(session: &Session) -> PathBuf {
    session
        .runtime_dir()
        .join("home/.config/hoarder/config.toml")
}

fn write_config(session: &Session, text: &str) {
    let path = config_path(session);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn config_path_is_under_xdg_config_home() {
    let session = Session::start_default().unwrap();

    let out = hoarder(&session, &["config", "path"]);

    assert_eq!(
        stdout(&out).trim_end(),
        config_path(&session).to_str().unwrap()
    );
}

#[test]
fn config_set_round_trips_a_commented_file() {
    let session = Session::start_default().unwrap();
    let original = "# my hoarder\n\n[screenshot]\n# keep it lossless\nformat   = \"png\"  # for now\nsave = true\n";
    write_config(&session, original);

    stdout(&hoarder(
        &session,
        &["config", "set", "screenshot.format", "webp"],
    ));
    assert_eq!(
        std::fs::read_to_string(config_path(&session)).unwrap(),
        original.replace("\"png\"", "\"webp\"")
    );
    assert_eq!(
        stdout(&hoarder(&session, &["config", "get", "screenshot.format"])),
        "webp\n"
    );

    stdout(&hoarder(
        &session,
        &["config", "set", "screenshot.format", "png"],
    ));
    assert_eq!(
        std::fs::read_to_string(config_path(&session)).unwrap(),
        original
    );
}

#[test]
fn config_set_without_a_file_starts_from_the_documented_defaults() {
    let session = Session::start_default().unwrap();

    stdout(&hoarder(
        &session,
        &["config", "set", "history.max_clips", "50"],
    ));

    let text = std::fs::read_to_string(config_path(&session)).unwrap();
    assert!(text.contains("max_clips = 50\n"), "{text}");
    assert!(text.contains("# png | webp | jpeg | avif | qoi"), "{text}");
    assert_eq!(
        stdout(&hoarder(&session, &["config", "get", "history.max_clips"])),
        "50\n"
    );
}

#[test]
fn config_set_refuses_a_bad_value_and_leaves_the_file_alone() {
    let session = Session::start_default().unwrap();
    write_config(&session, "[screenshot]\nformat = \"png\"\n");

    let out = hoarder(&session, &["config", "set", "screenshot.format", "gif"]);

    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("gif"));
    assert_eq!(
        std::fs::read_to_string(config_path(&session)).unwrap(),
        "[screenshot]\nformat = \"png\"\n"
    );
}

#[test]
fn config_check_reports_line_and_column_of_bad_keys_and_values() {
    let session = Session::start_default().unwrap();
    let path = config_path(&session);

    write_config(&session, "[screenshot]\nsave = true\n  nmae = \"x\"\n");
    let out = hoarder(&session, &["config", "check"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&format!("{}:3:3: ", path.display())),
        "bad key: {stderr}"
    );
    assert!(stderr.contains("nmae"), "{stderr}");

    write_config(&session, "[history]\nmax_clips = -5\n");
    let out = hoarder(&session, &["config", "check"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&format!("{}:2:13: ", path.display())),
        "bad value: {stderr}"
    );

    write_config(&session, "[history]\nmax_clips = 5\n");
    stdout(&hoarder(&session, &["config", "check"]));
}

#[test]
fn config_reset_restores_a_default_or_the_whole_file() {
    let session = Session::start_default().unwrap();
    write_config(&session, "# mine\n[pick]\nformat = \"hsl\"  # fancy\n");

    stdout(&hoarder(&session, &["config", "reset", "pick.format"]));
    assert_eq!(
        std::fs::read_to_string(config_path(&session)).unwrap(),
        "# mine\n[pick]\nformat = \"hex\"  # fancy\n"
    );

    stdout(&hoarder(&session, &["config", "reset"]));
    let text = std::fs::read_to_string(config_path(&session)).unwrap();
    assert!(text.contains("[screenshot.avif]"), "{text}");
    stdout(&hoarder(&session, &["config", "check"]));
}

#[test]
fn config_edit_opens_the_editor_then_checks() {
    let session = Session::start_default().unwrap();

    let out = session
        .command(env!("CARGO_BIN_EXE_hoarder"))
        .args(["config", "edit"])
        .env("EDITOR", "sed -i s/^format.=..png./format=\\\"qoi\\\"/")
        .output()
        .unwrap();
    stdout(&out);
    assert_eq!(
        stdout(&hoarder(&session, &["config", "get", "screenshot.format"])),
        "qoi\n"
    );

    let out = session
        .command(env!("CARGO_BIN_EXE_hoarder"))
        .args(["config", "edit"])
        .env("EDITOR", "sed -i s/^save.=.true/save=1/")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains(":26:6: "));
}

#[test]
fn config_get_honours_set_overrides() {
    let session = Session::start_default().unwrap();

    let out = hoarder(
        &session,
        &["--set", "pick.format=rgb", "config", "get", "pick.format"],
    );

    assert_eq!(stdout(&out), "rgb\n");
}

/// A running `hoarder config get --watch`, its stdout lines on a channel.
struct Watching {
    child: Child,
    lines: mpsc::Receiver<String>,
}

impl Watching {
    fn start(session: &Session, key: &str) -> Watching {
        let mut child = session
            .command(env!("CARGO_BIN_EXE_hoarder"))
            .args(["config", "get", key, "--watch"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let out = std::io::BufReader::new(child.stdout.take().unwrap());
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in out.lines() {
                if tx.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        Watching { child, lines }
    }

    fn next(&self) -> String {
        self.lines
            .recv_timeout(Duration::from_secs(10))
            .expect("a line within 10 s")
    }
}

impl Drop for Watching {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Long enough for a watching process to have woken, settled and re-read the
/// file, so the half-written state is what it sees.
const WATCH_SETTLED: Duration = Duration::from_millis(500);

#[test]
fn a_running_process_picks_up_config_changes_without_restart() {
    let session = Session::start_default().unwrap();
    let watching = Watching::start(&session, "screenshot.format");
    assert_eq!(
        watching.next(),
        "png",
        "the default, before any file exists"
    );

    // `config set` from another process.
    stdout(&hoarder(
        &session,
        &["config", "set", "screenshot.format", "webp"],
    ));
    assert_eq!(watching.next(), "webp");

    // An editor that writes a temporary file and renames it over the config.
    let path = config_path(&session);
    let tmp = path.with_file_name(".config.toml.swp");
    std::fs::write(&tmp, "[screenshot]\nformat = \"avif\"\n").unwrap();
    std::fs::rename(&tmp, &path).unwrap();
    assert_eq!(watching.next(), "avif");

    // An editor that truncates the file and then writes it, caught between
    // the two: the empty file must not revert the value to its default.
    drop(std::fs::File::create(&path).unwrap());
    std::thread::sleep(WATCH_SETTLED);
    std::fs::write(&path, "[screenshot]\nformat = \"qoi\"\n").unwrap();
    assert_eq!(watching.next(), "qoi");

    // A broken save is ignored (the last good value stands); the fix is seen.
    std::fs::write(&path, "[screenshot]\nformat = \"gif\"\n").unwrap();
    std::fs::write(&path, "[screenshot]\nformat = \"jpeg\"\n").unwrap();
    assert_eq!(watching.next(), "jpeg");
}
