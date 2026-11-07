//! Shutter sound and flash (PLAN.md §4.2): both off unless the config asks.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, Instant};

use hoarder_e2e::{OutputSpec, Session};

/// A `canberra-gtk-play` stand-in that appends its arguments to `<dir>/calls`.
fn fake_player(session: &Session) -> PathBuf {
    let dir = session.runtime_dir().join("fake-bin");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("canberra-gtk-play");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\necho \"canberra-gtk-play $*\" >> {}\n",
            dir.join("calls").display()
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    dir
}

fn hoarder(session: &Session, fake: &Path, args: &[&str]) -> Output {
    let path = std::env::var("PATH").unwrap_or_default();
    let out = session
        .command(env!("CARGO_BIN_EXE_hoarder"))
        .env("PATH", format!("{}:{path}", fake.display()))
        .args(args)
        .output()
        .expect("hoarder should run");
    assert!(
        out.status.success(),
        "hoarder failed ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn calls(fake: &Path) -> String {
    std::fs::read_to_string(fake.join("calls")).unwrap_or_default()
}

#[test]
fn shutter_sound_plays_when_enabled() {
    let session = Session::start(&[OutputSpec::new(64, 48)]).unwrap();
    let fake = fake_player(&session);

    hoarder(
        &session,
        &fake,
        &[
            "--set",
            "screenshot.sound=true",
            "shot",
            "all",
            "--no-notify",
        ],
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    while calls(&fake).is_empty() {
        assert!(Instant::now() < deadline, "no sound played");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        calls(&fake).starts_with("canberra-gtk-play -i screen-capture"),
        "{}",
        calls(&fake)
    );
}

#[test]
fn no_sound_by_default() {
    let session = Session::start(&[OutputSpec::new(64, 48)]).unwrap();
    let fake = fake_player(&session);

    hoarder(&session, &fake, &["shot", "all", "--no-notify"]);

    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(calls(&fake), "");
}
