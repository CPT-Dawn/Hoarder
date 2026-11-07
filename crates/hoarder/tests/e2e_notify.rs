//! The Capture notification (PLAN.md §4.1 step 7): a thumbnail plus Open,
//! Edit, Show in folder and Delete, over org.freedesktop.Notifications.
//!
//! Each test runs its own `dbus-daemon` with a stand-in notification server
//! on it (see `bus`), so nothing here ever shows up on the real desktop.

mod bus;

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, Instant};

use bus::Bus;
use hoarder_e2e::{OutputSpec, Session};

/// A directory of stand-ins for `xdg-open` and friends, each appending its
/// name and arguments to `<dir>/calls`.
fn fake_tools(session: &Session, names: &[&str]) -> PathBuf {
    let dir = session.runtime_dir().join("fake-bin");
    std::fs::create_dir_all(&dir).unwrap();
    for name in names {
        let path = dir.join(name);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"{name} $*\" >> {}\n",
                dir.join("calls").display()
            ),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }
    dir
}

/// Wait for a line in `<fake>/calls` starting with `prefix`.
fn wait_for_call(fake: &Path, prefix: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let calls = std::fs::read_to_string(fake.join("calls")).unwrap_or_default();
        if let Some(line) = calls.lines().find(|l| l.starts_with(prefix)) {
            return line.to_owned();
        }
        assert!(
            Instant::now() < deadline,
            "never called {prefix:?}: {calls:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn hoarder(session: &Session, fake: Option<&Path>, args: &[&str]) -> Output {
    let mut cmd = session.command(env!("CARGO_BIN_EXE_hoarder"));
    if let Some(fake) = fake {
        let path = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", format!("{}:{path}", fake.display()));
    }
    let out = cmd.args(args).output().expect("hoarder should run");
    assert!(
        out.status.success(),
        "hoarder failed ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn saved_path(out: &Output) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(&out.stdout).trim_end())
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_capture_is_announced_with_a_thumbnail_and_four_actions() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    let bus = Bus::start(&session);

    let out = hoarder(&session, None, &["shot", "all"]);

    let saved = saved_path(&out);
    let n = bus.notified(1);
    assert_eq!(n.app_name, "Hoarder");
    assert!(n.summary.contains("Screenshot"), "summary: {}", n.summary);
    assert!(n.body.contains("320 × 240"), "body: {}", n.body);
    let keys: Vec<&str> = n.actions.iter().step_by(2).map(String::as_str).collect();
    let labels: Vec<&str> = n
        .actions
        .iter()
        .skip(1)
        .step_by(2)
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["default", "open", "edit", "folder", "delete"]);
    assert_eq!(labels[1..], ["Open", "Edit", "Show in folder", "Delete"]);
    assert_eq!(
        n.hints.get("image-path").map(String::as_str),
        Some(format!("file://{}", saved.display()).as_str())
    );
}

#[test]
fn no_notify_flag_keeps_quiet() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    let bus = Bus::start(&session);

    hoarder(&session, None, &["shot", "all", "--no-notify"]);
    hoarder(
        &session,
        None,
        &["--set", "screenshot.notify=false", "shot", "all"],
    );

    // A notification would have arrived well within this.
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(bus.count(), 0);
}

#[test]
fn delete_removes_the_file_and_closes_the_notification() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    let bus = Bus::start(&session);

    let saved = saved_path(&hoarder(&session, None, &["shot", "all"]));
    bus.notified(1);
    assert!(saved.is_file());
    bus.invoke(1, "delete");

    wait_until("the file is deleted", || !saved.exists());
    wait_until("the notification is closed", || bus.closed() == [1]);
}

#[test]
fn open_and_clicking_the_body_open_the_capture() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    let bus = Bus::start(&session);
    let fake = fake_tools(&session, &["xdg-open"]);

    let saved = saved_path(&hoarder(&session, Some(&fake), &["shot", "all"]));
    bus.notified(1);
    bus.invoke(1, "open");

    assert_eq!(
        wait_for_call(&fake, "xdg-open"),
        format!("xdg-open {}", saved.display())
    );
}

#[test]
fn edit_prefers_an_annotation_tool() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    let bus = Bus::start(&session);
    let fake = fake_tools(&session, &["xdg-open", "satty"]);

    let saved = saved_path(&hoarder(&session, Some(&fake), &["shot", "all"]));
    bus.notified(1);
    bus.invoke(1, "edit");

    assert_eq!(
        wait_for_call(&fake, "satty"),
        format!("satty --filename {}", saved.display())
    );
}

#[test]
fn show_in_folder_falls_back_to_opening_the_directory() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    let bus = Bus::start(&session);
    let fake = fake_tools(&session, &["xdg-open"]);

    let saved = saved_path(&hoarder(&session, Some(&fake), &["shot", "all"]));
    bus.notified(1);
    // The private bus has no org.freedesktop.FileManager1.
    bus.invoke(1, "folder");

    assert_eq!(
        wait_for_call(&fake, "xdg-open"),
        format!("xdg-open {}", saved.parent().unwrap().display())
    );
}

#[test]
fn a_capture_to_stdout_is_announced_without_file_actions() {
    let session = Session::start(&[OutputSpec::new(64, 48)]).unwrap();
    let bus = Bus::start(&session);

    hoarder(&session, None, &["shot", "all", "-o", "-"]);

    let n = bus.notified(1);
    assert!(n.actions.is_empty(), "actions: {:?}", n.actions);
    assert!(!n.hints.contains_key("image-path"));
}

#[test]
fn shot_is_silent_and_successful_without_a_notification_server() {
    let session = Session::start(&[OutputSpec::new(64, 48)]).unwrap();

    let out = hoarder(&session, None, &["shot", "all"]);

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("notif"), "stderr: {stderr}");
}

/// A 1× output beside a 2× one.
fn one_by_two() -> Session {
    Session::start(&[
        OutputSpec::new(320, 240),
        OutputSpec::new(640, 480).scale(2.0),
    ])
    .unwrap()
}

#[test]
fn a_mixed_scale_capture_carries_its_badge() {
    let session = one_by_two();
    let bus = Bus::start(&session);

    hoarder(&session, None, &["shot", "all"]);

    let n = bus.notified(1);
    assert!(n.body.contains("1280 × 480"), "body: {}", n.body);
    assert!(n.body.contains("mixed scale"), "body: {}", n.body);
}

#[test]
fn a_split_capture_announces_each_file() {
    let session = one_by_two();
    let bus = Bus::start(&session);

    hoarder(
        &session,
        None,
        &["shot", "all", "--set", "capture.mixed_scale=split"],
    );

    let bodies = [bus.notified(1).body, bus.notified(2).body];
    assert!(bodies.iter().any(|b| b.contains("320 × 240")), "{bodies:?}");
    assert!(bodies.iter().any(|b| b.contains("640 × 480")), "{bodies:?}");
    assert!(
        !bodies.iter().any(|b| b.contains("mixed scale")),
        "{bodies:?}"
    );
}
