//! Window snapping (PLAN.md §4.2): hovering highlights the window under the
//! pointer, a click captures it by cropping the Frozen Frame, and W cycles
//! through the windows stacked under the pointer. Driven on Sway, whose
//! `get_tree` is the window-geometry provider.

mod common;

use common::{Overlay, Png};
use std::time::{Duration, Instant};

use hoarder_e2e::{Client, Image, Output, OutputSpec, Pattern, Rgb8, Session, keys};

/// What a probe window shows: one grey pixel, stretched.
const PROBE: Rgb8 = Rgb8(0x80, 0x80, 0x80);
/// The accent colour of borders on the Overlay.
const ACCENT: Rgb8 = Rgb8(0x3d, 0x8e, 0xff);
/// A desktop colour nothing else uses.
const DESKTOP: Rgb8 = Rgb8(200, 30, 30);

/// The bounding box of `colour` in `image`, as `(x, y, width, height)`.
fn bbox(image: &Image, colour: Rgb8) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = image.size();
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if image.pixel(x, y) == colour {
                (x0, y0) = (x0.min(x), y0.min(y));
                (x1, y1) = (x1.max(x), y1.max(y));
            }
        }
    }
    (x0 != u32::MAX).then(|| (x0, y0, x1 + 1 - x0, y1 + 1 - y0))
}

/// Poll grim until `ready` holds for `output`'s screenshot (at most 5 s).
fn screenshot_when(session: &Session, output: &str, ready: impl Fn(&Image) -> bool) -> Image {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let shot = session.screenshot(output).unwrap();
        if ready(&shot) {
            return shot;
        }
        assert!(Instant::now() < deadline, "{output} never got there");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Open a probe window on `output`, float it with `commands` (e.g.
/// `"border none, resize set 200 120, move absolute position 100 60"`) and
/// wait until its grey content shows exactly at `content` (physical pixels
/// of `output`).
fn float(
    session: &Session,
    app: &Client,
    output: &Output,
    app_id: &str,
    commands: &str,
    content: (u32, u32, u32, u32),
) {
    session
        .swaymsg(&[&format!("focus output {}", output.name)])
        .unwrap();
    app.open_window(app_id).unwrap();
    session
        .swaymsg(&[&format!("[app_id={app_id}] floating enable, {commands}")])
        .unwrap();
    let (x, y, w, h) = content;
    screenshot_when(session, &output.name, |s| {
        s.pixel(x, y) == PROBE
            && s.pixel(x + w - 1, y + h - 1) == PROBE
            && (x == 0 || s.pixel(x - 1, y) != PROBE)
            && (y == 0 || s.pixel(x, y - 1) != PROBE)
            && s.pixel(x + w, y + h - 1) != PROBE
            && s.pixel(x + w - 1, y + h) != PROBE
    });
}

fn assert_all(png: &Png, colour: Rgb8, what: &str) {
    assert_eq!(png.depth, 8, "{what}: 8-bit output, 8-bit PNG");
    let Rgb8(r, g, b) = colour;
    let want = [r as u16, g as u16, b as u16];
    for y in 0..png.height {
        for x in 0..png.width {
            assert_eq!(png.rgb(x, y), want, "{what}: pixel ({x}, {y})");
        }
    }
}

#[test]
fn clicking_a_window_captures_exactly_its_content() {
    let session = Session::start(&[OutputSpec::new(640, 480)]).unwrap();
    let app = session.client().unwrap();
    let out = session.output("HEADLESS-1");
    app.paint(out, &Pattern::solid(DESKTOP)).unwrap();
    // A 4 px border: the window's content is what gets captured, not its frame.
    float(
        &session,
        &app,
        out,
        "probe.a",
        "border pixel 4, resize set 200 120, move absolute position 100 60",
        (104, 64, 192, 112),
    );
    let seen = session.screenshot("HEADLESS-1").unwrap();
    assert_eq!(bbox(&seen, PROBE), Some((104, 64, 192, 112)));

    let overlay = Overlay::start(&session, &["shot", "region", "-o", "-"], &["HEADLESS-1"]);
    let input = session.input();
    input.move_to(out, 150.0, 100.0).unwrap();
    input.click(keys::BTN_LEFT).unwrap();
    let png = overlay.finish().png();

    assert_eq!((png.width, png.height), (192, 112));
    assert_all(&png, PROBE, "the window");
}
