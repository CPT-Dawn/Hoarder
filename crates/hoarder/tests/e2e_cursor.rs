//! The cursor layer: frozen apart from the frame, left out of a Capture by
//! default, and composited in with `--cursor` or `C` in the Overlay.

mod common;

use common::{Overlay, Png, as_8bit};

use hoarder_e2e::{Image, OutputSpec, Pattern, Session, keys};

const SIZE: (u32, u32) = (640, 480);
/// Where the pointer rests when the Capture is asked for.
const POINTER: (f64, f64) = (100.0, 100.0);

/// A painted output with the pointer resting on it.
fn desktop() -> (Session, hoarder_e2e::Client) {
    let session = Session::start(&[OutputSpec::new(SIZE.0, SIZE.1)]).unwrap();
    let app = session.client().unwrap();
    let out = session.output("HEADLESS-1");
    app.paint(out, &Pattern::Ramp).unwrap();
    session.input().move_to(out, POINTER.0, POINTER.1).unwrap();
    (session, app)
}

/// What the compositor itself shows on that desktop with the cursor drawn in
/// (`grim -c`). Taken from a twin session: once anything has asked headless
/// Sway for cursors it draws a software cursor into every later capture,
/// which would spoil the session under test.
fn with_cursor_per_grim() -> Image {
    let (session, _app) = desktop();
    // The compositor draws the cursor on its next frame.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let shot = session
            .command("grim")
            .args(["-c", "-t", "ppm", "-o", "HEADLESS-1", "-"])
            .output()
            .unwrap();
        assert!(shot.status.success());
        let image = Image::from_ppm(&shot.stdout).unwrap();
        if cursor_pixels(&image) > 20 {
            return image;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "grim -c never shows a cursor at the pointer"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// A desktop to capture, and `grim -c`'s view of its twin.
fn setup() -> (Session, hoarder_e2e::Client, Image) {
    let grim = with_cursor_per_grim();
    let (session, app) = desktop();
    (session, app, grim)
}

/// How many pixels near the pointer differ from the painted pattern.
fn cursor_pixels(image: &Image) -> usize {
    let (x0, y0) = (POINTER.0 as u32 - 8, POINTER.1 as u32 - 8);
    (y0..y0 + 48)
        .flat_map(|y| (x0..x0 + 48).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            image.pixel(x, y) != as_8bit(Pattern::Ramp.colour_at(x, y, SIZE.0, SIZE.1))
        })
        .count()
}

/// Drag out (50, 50)-(250, 250), around the resting pointer, pressing `keys`
/// first, and return the Capture.
fn capture(session: &Session, args: &[&str], keys_first: &[u32]) -> Png {
    let mut all = vec!["shot", "region", "-o", "-"];
    all.extend_from_slice(args);
    let overlay = Overlay::start(session, &all, &["HEADLESS-1"]);
    let input = session.input();
    for &k in keys_first {
        input.tap(k).unwrap();
    }
    let out = session.output("HEADLESS-1");
    input.move_to(out, 50.0, 50.0).unwrap();
    input.button(keys::BTN_LEFT, true).unwrap();
    input.move_to(out, 250.0, 250.0).unwrap();
    input.button(keys::BTN_LEFT, false).unwrap();
    let png = overlay.finish().png();
    assert_eq!((png.width, png.height), (200, 200));
    png
}

/// Pixels of the Capture that differ from the painted pattern.
fn changed(png: &Png) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for y in 0..png.height {
        for x in 0..png.width {
            let p = as_8bit(Pattern::Ramp.colour_at(50 + x, 50 + y, SIZE.0, SIZE.1));
            if png.rgb(x, y) != [p.0 as u16, p.1 as u16, p.2 as u16] {
                out.push((x + 50, y + 50));
            }
        }
    }
    out
}

/// The Capture shows the cursor where and as the compositor draws it:
/// every pixel within 2 of `grim -c`'s.
fn assert_cursor_like_grim(png: &Png, grim: &Image) {
    let changed = changed(png);
    assert!(
        changed.len() > 20,
        "the cursor is in the Capture ({} pixels changed)",
        changed.len()
    );
    let mut wrong = Vec::new();
    for y in 0..png.height {
        for x in 0..png.width {
            let g = grim.pixel(50 + x, 50 + y);
            let got = png.rgb(x, y);
            let near = [g.0, g.1, g.2]
                .iter()
                .zip(got)
                .all(|(&a, b)| (a as i32 - b as i32).abs() <= 2);
            if !near && wrong.len() < 5 {
                wrong.push(format!("({}, {}): {got:?}, grim -c {g:?}", 50 + x, 50 + y));
            }
        }
    }
    assert!(wrong.is_empty(), "cursor pixels: {wrong:#?}");
}

#[test]
fn the_cursor_is_left_out_by_default() {
    let (session, _app) = desktop();
    let png = capture(&session, &[], &[]);
    assert_eq!(changed(&png), vec![], "no cursor in the Capture");
}

#[test]
fn with_cursor_flag_the_cursor_is_composited_in() {
    let (session, _app, grim) = setup();
    let png = capture(&session, &["--cursor"], &[]);
    assert_cursor_like_grim(&png, &grim);
}

#[test]
fn c_in_the_overlay_toggles_the_cursor() {
    let (session, _app, grim) = setup();
    let png = capture(&session, &[], &[keys::KEY_C]);
    assert_cursor_like_grim(&png, &grim);

    let out = session.output("HEADLESS-1");
    session.input().move_to(out, POINTER.0, POINTER.1).unwrap();
    let png = capture(&session, &["--cursor"], &[keys::KEY_C]);
    assert_eq!(changed(&png), vec![], "C turns --cursor off again");
}

#[test]
fn screenshot_cursor_in_the_config_turns_it_on() {
    let (session, _app, grim) = setup();
    let png = capture(&session, &["--set", "screenshot.cursor=true"], &[]);
    assert_cursor_like_grim(&png, &grim);
}
