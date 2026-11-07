//! `hoarder shot --live`: no freeze. The Overlay draws over the live desktop,
//! hides, and the Capture is taken after one frame callback, so it holds
//! what the desktop shows at release and none of the Overlay's pixels.

mod common;

use std::time::{Duration, Instant};

use common::{Overlay, as_8bit};
use hoarder_e2e::{Image, OutputSpec, Pattern, Rgb8, Session, keys};

/// Asserts the PNG is exactly `pattern` as painted on an 8-bit output of
/// physical size `size`, starting at physical pixel `origin`.
fn assert_cut_from(png: &common::Png, pattern: &Pattern, size: (u32, u32), origin: (u32, u32)) {
    assert_eq!(png.depth, 8, "an 8-bit output gives an 8-bit PNG");
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

/// Poll grim until `ready` holds for `output`'s screenshot (at most 5 s).
fn screenshot_when(session: &Session, output: &str, ready: impl Fn(&Image) -> bool) -> Image {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let shot = session.screenshot(output).unwrap();
        if ready(&shot) {
            return shot;
        }
        assert!(Instant::now() < deadline, "the screen never got there");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A white output, as the user sees it through the Overlay's 45% dimming.
fn dimmed_white(p: Rgb8) -> bool {
    (138..=142).contains(&p.0) && p.0 == p.1 && p.1 == p.2
}

#[test]
fn live_captures_what_the_desktop_shows_at_release_without_overlay_pixels() {
    let session = Session::start(&[OutputSpec::new(640, 480)]).unwrap();
    let app = session.client().unwrap();
    let out = session.output("HEADLESS-1");
    let white = Pattern::solid(Rgb8(255, 255, 255));
    app.paint(out, &white).unwrap();

    let file = session.runtime_dir().join("live.png");
    let overlay = Overlay::start(
        &session,
        &["shot", "region", "--live", "-o", file.to_str().unwrap()],
        &["HEADLESS-1"],
    );
    // The Overlay dims the live desktop: there is no Frozen Frame under it.
    let seen = session.screenshot("HEADLESS-1").unwrap();
    assert!(
        dimmed_white(seen.pixel(320, 240)),
        "{:?}",
        seen.pixel(320, 240)
    );

    // The desktop changes under the Overlay; a live Capture must see it.
    app.paint(out, &Pattern::Ramp).unwrap();

    // Select the whole output but its last row and column. There is no room
    // around the selection, so the size pill sits inside it: Overlay pixels
    // are on screen within the Region right up to the release.
    let input = session.input();
    input.move_to(out, 0.0, 0.0).unwrap();
    input.button(keys::BTN_LEFT, true).unwrap();
    input.move_to(out, 639.0, 479.0).unwrap();
    screenshot_when(&session, "HEADLESS-1", |s| {
        let p = s.pixel(10, 10);
        p.0 < 60
            && p.1 < 60
            && p.2 < 60
            && s.pixel(639, 200) != as_8bit(Pattern::Ramp.colour_at(639, 200, 640, 480))
    });
    input.button(keys::BTN_LEFT, false).unwrap();
    let done = overlay.finish();
    assert!(done.status.success(), "{}: {}", done.status, done.stderr);
    let png = common::Png::decode(&std::fs::read(&file).unwrap());

    assert_eq!((png.width, png.height), (639, 479));
    assert_cut_from(&png, &Pattern::Ramp, (640, 480), (0, 0));
}

#[test]
fn live_all_captures_every_output_without_an_overlay() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    let app = session.client().unwrap();
    app.paint(session.output("HEADLESS-1"), &Pattern::Quadrants)
        .unwrap();

    let out = session
        .command(env!("CARGO_BIN_EXE_hoarder"))
        .args(["shot", "all", "--live", "-o", "-"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let png = common::Png::decode(&out.stdout);
    assert_eq!((png.width, png.height), (320, 240));
    assert_cut_from(&png, &Pattern::Quadrants, (320, 240), (0, 0));
}
