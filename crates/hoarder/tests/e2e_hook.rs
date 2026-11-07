//! The `on_capture` hook (PLAN.md §4.1 step 7): it runs after every Capture
//! with the Capture's path, size and Target in its environment, and a hook that
//! fails or hangs is logged but never costs the user the Capture.

use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::{Duration, Instant};

use hoarder_e2e::{OutputSpec, Session};

fn hoarder(session: &Session, args: &[&str]) -> Output {
    session
        .command(env!("CARGO_BIN_EXE_hoarder"))
        .args(args)
        .output()
        .expect("hoarder should run")
}

fn assert_ok(out: &Output) {
    assert!(
        out.status.success(),
        "hoarder failed ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn home(session: &Session) -> PathBuf {
    session.runtime_dir().join("home")
}

fn write_config(session: &Session, text: &str) {
    let dir = home(session).join(".config/hoarder");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), text).unwrap();
}

/// Wait up to 10 s for `path` to exist with content, then return it.
fn wait_for_file(path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && !text.is_empty()
        {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "{} never appeared",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A hook that writes its environment, one `KEY=value` per line, to `out`.
fn env_dump_hook(out: &Path) -> String {
    format!(
        "printf '%s\\n' \"PATH=$HOARDER_PATH\" \"WIDTH=$HOARDER_WIDTH\" \
         \"HEIGHT=$HOARDER_HEIGHT\" \"TARGET=$HOARDER_TARGET\" > {}.tmp && mv {0}.tmp {0}",
        out.display()
    )
}

fn toml_string(s: &str) -> String {
    format!("'''{s}'''")
}

#[test]
fn hook_receives_the_saved_path_size_and_target() {
    let session = Session::start(&[OutputSpec::new(320, 240), OutputSpec::new(200, 100)]).unwrap();
    let dump = session.runtime_dir().join("hook-env");
    write_config(
        &session,
        &format!(
            "[screenshot]\nnotify = false\non_capture = {}\n",
            toml_string(&env_dump_hook(&dump))
        ),
    );

    let out = hoarder(&session, &["shot", "all"]);

    assert_ok(&out);
    let saved = String::from_utf8_lossy(&out.stdout).trim_end().to_owned();
    assert!(Path::new(&saved).is_file(), "saved: {saved:?}");
    assert_eq!(
        wait_for_file(&dump),
        format!("PATH={saved}\nWIDTH=520\nHEIGHT=240\nTARGET=all\n")
    );
}

#[test]
fn hook_gets_the_output_file_with_dash_o() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    let dump = session.runtime_dir().join("hook-env");
    let file = session.runtime_dir().join("out.png");
    write_config(
        &session,
        &format!(
            "[screenshot]\nnotify = false\non_capture = {}\n",
            toml_string(&env_dump_hook(&dump))
        ),
    );

    let out = hoarder(&session, &["shot", "all", "-o", file.to_str().unwrap()]);

    assert_ok(&out);
    assert_eq!(
        wait_for_file(&dump),
        format!(
            "PATH={}\nWIDTH=320\nHEIGHT=240\nTARGET=all\n",
            file.display()
        )
    );
}

#[test]
fn hook_output_never_leaks_into_a_png_on_stdout() {
    let session = Session::start(&[OutputSpec::new(64, 48)]).unwrap();
    write_config(
        &session,
        "[screenshot]\nnotify = false\non_capture = 'echo noise; echo more noise >&2'\n",
    );

    let out = hoarder(&session, &["shot", "all", "-o", "-"]);

    assert_ok(&out);
    let reader = png::Decoder::new(std::io::Cursor::new(&out.stdout))
        .read_info()
        .expect("stdout is exactly one PNG");
    assert_eq!((reader.info().width, reader.info().height), (64, 48));
    assert!(!out.stdout.ends_with(b"noise\n"), "hook wrote to stdout");
}

#[test]
fn a_failing_hook_is_logged_and_the_capture_still_lands() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    write_config(
        &session,
        "[screenshot]\nnotify = false\non_capture = 'exit 7'\n",
    );

    let out = hoarder(&session, &["shot", "all"]);

    assert_ok(&out);
    let saved = String::from_utf8_lossy(&out.stdout).trim_end().to_owned();
    assert!(Path::new(&saved).is_file(), "saved: {saved:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("on_capture hook failed") && stderr.contains('7'),
        "stderr: {stderr}"
    );
}

#[test]
fn a_hanging_hook_does_not_hold_up_the_shot() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    let pid_file = session.runtime_dir().join("hook-pid");
    write_config(
        &session,
        &format!(
            "[screenshot]\nnotify = false\non_capture = {}\n",
            toml_string(&format!(
                "echo $$ > {0}.tmp && mv {0}.tmp {0}; exec sleep 60",
                pid_file.display()
            ))
        ),
    );
    let log = std::fs::File::create(session.runtime_dir().join("stderr")).unwrap();

    let started = Instant::now();
    let status = session
        .command(env!("CARGO_BIN_EXE_hoarder"))
        .args(["shot", "all"])
        .stdout(Stdio::null())
        .stderr(log)
        .status()
        .unwrap();
    let took = started.elapsed();

    let pid = wait_for_file(&pid_file);
    let _ = std::process::Command::new("kill").arg(pid.trim()).status();
    assert!(status.success(), "{status}");
    assert!(
        took < Duration::from_secs(5),
        "shot waited {took:?} for the hook"
    );
}

#[test]
fn no_hook_runs_when_on_capture_is_empty() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    write_config(&session, "[screenshot]\nnotify = false\n");

    let out = hoarder(&session, &["shot", "all"]);

    assert_ok(&out);
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("hook"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
