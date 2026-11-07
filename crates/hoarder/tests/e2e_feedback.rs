//! Shutter sound and flash (PLAN.md §4.2): both off unless the config asks.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, Instant};

use hoarder_e2e::{OutputSpec, Pattern, Rgb8, Session};

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

/// Run `hoarder shot all` with `args` while grim watches HEADLESS-1; return
/// whether any frame showed white at (10, 10), which is painted black, and
/// the black-painted Capture hoarder saved.
fn watch_shot(session: &Session, args: &[&str]) -> (bool, Vec<u8>) {
    let app = session.client().unwrap();
    app.paint(session.output("HEADLESS-1"), &Pattern::solid(Rgb8(0, 0, 0)))
        .unwrap();
    let out = session.runtime_dir().join("shot.png");
    let mut child = session
        .command(env!("CARGO_BIN_EXE_hoarder"))
        .args(["shot", "all", "--no-notify", "--no-copy", "-o"])
        .arg(&out)
        .args(args)
        .spawn()
        .unwrap();
    let mut saw_white = false;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        let frame = session.screenshot("HEADLESS-1").unwrap();
        saw_white |= frame.pixel(10, 10) == Rgb8(255, 255, 255);
    };
    assert!(status.success(), "{status}");
    (saw_white, std::fs::read(out).unwrap())
}

/// Every pixel of a PNG Capture is black.
fn all_black(png_bytes: &[u8]) -> bool {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png_bytes))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    let channels = info.color_type.samples();
    buf[..info.buffer_size()]
        .chunks(channels)
        .all(|px| px[..3].iter().all(|&c| c == 0))
}

#[test]
fn flash_whitens_the_screen_but_not_the_capture() {
    let session = Session::start(&[OutputSpec::new(160, 120)]).unwrap();

    let (saw_white, capture) = watch_shot(&session, &["--set", "screenshot.flash=true"]);

    assert!(saw_white, "the screen never flashed");
    assert!(all_black(&capture), "the flash leaked into the Capture");
    // And it is gone again afterwards.
    let after = session.screenshot("HEADLESS-1").unwrap();
    assert_eq!(after.pixel(10, 10), Rgb8(0, 0, 0));
}

#[test]
fn no_flash_by_default() {
    let session = Session::start(&[OutputSpec::new(160, 120)]).unwrap();

    let (saw_white, capture) = watch_shot(&session, &[]);

    assert!(!saw_white, "the screen flashed");
    assert!(all_black(&capture));
}
