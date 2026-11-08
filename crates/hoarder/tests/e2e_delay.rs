//! `hoarder shot --delay N`: a countdown ring on the pointer's output, then
//! the freeze at zero, then (for a Region) the Overlay.

mod common;

use std::process::Stdio;
use std::time::{Duration, Instant};

use common::as_8bit;
use hoarder_e2e::{Image, OutputSpec, Pattern, Rgb8, Session, keys};

const WHITE: Rgb8 = Rgb8(255, 255, 255);

/// Asserts the PNG is exactly `pattern` as painted on an 8-bit output of
/// physical size `size`, starting at physical pixel `origin`.
fn assert_cut_from(png: &common::Png, pattern: &Pattern, size: (u32, u32), origin: (u32, u32)) {
    let mut wrong = Vec::new();
    for y in 0..png.height {
        for x in 0..png.width {
            let Rgb8(r, g, b) =
                as_8bit(pattern.colour_at(origin.0 + x, origin.1 + y, size.0, size.1));
            let want = [r as u16, g as u16, b as u16];
            let got = png.rgb(x, y);
            if got != want && wrong.len() < 5 {
                wrong.push(format!("({x}, {y}): got {got:?}, want {want:?}"));
            }
        }
    }
    assert!(wrong.is_empty(), "pixels differ: {wrong:#?}");
}

/// Poll grim until `ready` holds for `output`'s screenshot (at most 10 s).
fn screenshot_when(session: &Session, output: &str, ready: impl Fn(&Image) -> bool) -> Image {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let shot = session.screenshot(output).unwrap();
        if ready(&shot) {
            return shot;
        }
        assert!(Instant::now() < deadline, "the screen never got there");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The Overlay's 45% dimming of white.
fn dimmed_white(p: Rgb8) -> bool {
    (138..=142).contains(&p.0) && p.0 == p.1 && p.1 == p.2
}

#[test]
fn the_countdown_freezes_at_zero_and_opens_the_overlay() {
    let session = Session::start(&[OutputSpec::new(640, 480)]).unwrap();
    let app = session.client().unwrap();
    let out = session.output("HEADLESS-1");
    app.paint(out, &Pattern::solid(WHITE)).unwrap();
    let input = session.input();
    input.move_to(out, 600.0, 20.0).unwrap();

    let file = session.runtime_dir().join("delayed.png");
    let started = Instant::now();
    let mut child = session
        .command(env!("CARGO_BIN_EXE_hoarder"))
        .args(["shot", "region", "--delay", "1.5", "-o"])
        .arg(&file)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // A countdown ring in the middle of the output; nothing else changes and
    // there is no Overlay yet.
    let ring = screenshot_when(&session, "HEADLESS-1", |s| s.pixel(320, 240) != WHITE);
    assert_eq!(ring.pixel(10, 10), WHITE, "no Overlay during the countdown");
    assert_eq!(ring.pixel(320, 100), WHITE, "the ring is small");
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "the ring shows during the countdown"
    );

    // The desktop changes during the countdown: the freeze comes later.
    app.paint(out, &Pattern::Quadrants).unwrap();
    assert!(child.try_wait().unwrap().is_none(), "still counting down");

    // At zero the Overlay opens over the frozen desktop, ring gone.
    let seen = screenshot_when(&session, "HEADLESS-1", |s| dimmed_white(s.pixel(600, 400)));
    assert!(
        started.elapsed() >= Duration::from_millis(1500),
        "the Overlay waited for zero"
    );
    assert!(
        dimmed_white(seen.pixel(600, 400)),
        "{:?}",
        seen.pixel(600, 400)
    );
    assert!(
        dimmed_white(seen.pixel(330, 250)),
        "no ring on the Overlay: {:?}",
        seen.pixel(330, 250)
    );

    input.move_to(out, 0.0, 0.0).unwrap();
    input.button(keys::BTN_LEFT, true).unwrap();
    input.move_to(out, 640.0, 480.0).unwrap();
    input.button(keys::BTN_LEFT, false).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "hoarder did not exit");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success(), "{status}");

    let png = common::Png::decode(&std::fs::read(&file).unwrap());
    assert_eq!((png.width, png.height), (640, 480));
    assert_cut_from(&png, &Pattern::Quadrants, (640, 480), (0, 0));
}

#[test]
fn the_ring_shows_on_the_pointers_output_and_stays_out_of_the_capture() {
    let session = Session::start(&[OutputSpec::new(320, 240), OutputSpec::new(400, 300)]).unwrap();
    let app = session.client().unwrap();
    let (left, right) = (session.output("HEADLESS-1"), session.output("HEADLESS-2"));
    app.paint(left, &Pattern::solid(WHITE)).unwrap();
    app.paint(right, &Pattern::solid(WHITE)).unwrap();
    session.input().move_to(right, 50.0, 50.0).unwrap();

    let file = session.runtime_dir().join("all.png");
    let mut child = session
        .command(env!("CARGO_BIN_EXE_hoarder"))
        .args(["shot", "all", "--delay", "1", "-o"])
        .arg(&file)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    screenshot_when(&session, "HEADLESS-2", |s| s.pixel(200, 150) != WHITE);
    let other = session.screenshot("HEADLESS-1").unwrap();
    assert_eq!(other.pixel(160, 120), WHITE, "no ring on the other output");

    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "hoarder did not exit");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success(), "{status}");
    let png = common::Png::decode(&std::fs::read(&file).unwrap());
    assert_eq!((png.width, png.height), (720, 300));
    assert_cut_from(
        &png.crop(320, 0, 400, 300),
        &Pattern::solid(WHITE),
        (400, 300),
        (0, 0),
    );
}
