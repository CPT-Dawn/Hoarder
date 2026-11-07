//! Window snapping (PLAN.md §4.2): hovering highlights the window under the
//! pointer, a click captures it by cropping the Frozen Frame, and W cycles
//! through the windows stacked under the pointer. Driven on Sway, whose
//! `get_tree` is the window-geometry provider.

mod common;

use common::{Overlay, Png};
use std::time::{Duration, Instant};

use hoarder_e2e::{Client, Image, Output, OutputSpec, Pattern, Rgb8, Session, keys};

/// A desktop colour no window uses.
const DESKTOP: Rgb8 = Rgb8(200, 30, 30);
const GREEN: Rgb8 = Rgb8(0, 200, 0);
const YELLOW: Rgb8 = Rgb8(220, 220, 0);

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

/// Open a `colour` probe window on `output`, float it with `commands` (e.g.
/// `"border none, resize set 200 120, move absolute position 100 60"`) and
/// wait until its content shows exactly at `content` (physical pixels of
/// `output`), on top.
fn float(
    session: &Session,
    app: &Client,
    output: &Output,
    (app_id, colour): (&str, Rgb8),
    commands: &str,
    content: (u32, u32, u32, u32),
) {
    session
        .swaymsg(&[&format!("focus output {}", output.name)])
        .unwrap();
    app.open_window_coloured(app_id, colour).unwrap();
    session
        .swaymsg(&[&format!("[app_id={app_id}] floating enable, {commands}")])
        .unwrap();
    screenshot_when(session, &output.name, |s| bbox(s, colour) == Some(content));
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
        ("probe.a", GREEN),
        "border pixel 4, resize set 200 120, move absolute position 100 60",
        (104, 64, 192, 112),
    );

    let overlay = Overlay::start(&session, &["shot", "region", "-o", "-"], &["HEADLESS-1"]);
    let input = session.input();
    input.move_to(out, 150.0, 100.0).unwrap();
    input.click(keys::BTN_LEFT).unwrap();
    let png = overlay.finish().png();

    assert_eq!((png.width, png.height), (192, 112));
    assert_all(&png, GREEN, "the window");
}

#[test]
fn a_window_on_a_scaled_output_is_cut_in_physical_pixels() {
    let session = Session::start(&[
        OutputSpec::new(640, 480),
        OutputSpec::new(800, 600).scale(2.0),
    ])
    .unwrap();
    let app = session.client().unwrap();
    let right = session.output("HEADLESS-2");
    for o in session.outputs() {
        app.paint(o, &Pattern::solid(DESKTOP)).unwrap();
    }
    // Logical (700, 40) 100×80 is physical (120, 80) 200×160 on HEADLESS-2.
    float(
        &session,
        &app,
        right,
        ("probe.a", GREEN),
        "border none, resize set 100 80, move absolute position 700 40",
        (120, 80, 200, 160),
    );

    let overlay = Overlay::start(
        &session,
        &["shot", "-o", "-"],
        &["HEADLESS-1", "HEADLESS-2"],
    );
    let input = session.input();
    input.move_to(right, 100.0, 70.0).unwrap();
    input.click(keys::BTN_LEFT).unwrap();
    let png = overlay.finish().png();

    assert_eq!((png.width, png.height), (200, 160));
    assert_all(&png, GREEN, "the window at scale 2");
}

/// Two floating windows: green `below` at (100, 60) 200×120 and, opened
/// later and so on top, yellow `above` at (150, 100) 300×200.
fn two_stacked_windows(session: &Session, app: &Client) {
    let out = session.output("HEADLESS-1");
    float(
        session,
        app,
        out,
        ("probe.below", GREEN),
        "border none, resize set 200 120, move absolute position 100 60",
        (100, 60, 200, 120),
    );
    float(
        session,
        app,
        out,
        ("probe.above", YELLOW),
        "border none, resize set 300 200, move absolute position 150 100",
        (150, 100, 300, 200),
    );
}

#[test]
fn w_cycles_through_the_windows_under_the_pointer() {
    let session = Session::start(&[OutputSpec::new(640, 480)]).unwrap();
    let app = session.client().unwrap();
    let out = session.output("HEADLESS-1");
    app.paint(out, &Pattern::solid(DESKTOP)).unwrap();
    two_stacked_windows(&session, &app);

    let shot = |w_taps: usize| {
        let overlay = Overlay::start(&session, &["shot", "-o", "-"], &["HEADLESS-1"]);
        let input = session.input();
        input.move_to(out, 200.0, 150.0).unwrap();
        for _ in 0..w_taps {
            input.tap(keys::KEY_W).unwrap();
        }
        input.click(keys::BTN_LEFT).unwrap();
        overlay.finish().png()
    };

    let top = shot(0);
    assert_eq!((top.width, top.height), (300, 200));
    assert_all(&top, YELLOW, "a click takes the topmost window");

    // What you saw: the lower window with the upper one's corner over it.
    let below = shot(1);
    assert_eq!((below.width, below.height), (200, 120), "W goes one down");
    assert_all(&below.crop(0, 0, 200, 40), GREEN, "the lower window");
    assert_all(
        &below.crop(50, 40, 150, 80),
        YELLOW,
        "the upper one over it",
    );

    let again = shot(2);
    assert_eq!((again.width, again.height), (300, 200), "W wraps around");
}

#[test]
fn dragging_over_a_window_still_draws_a_region() {
    let session = Session::start(&[OutputSpec::new(640, 480)]).unwrap();
    let app = session.client().unwrap();
    let out = session.output("HEADLESS-1");
    app.paint(out, &Pattern::solid(DESKTOP)).unwrap();
    two_stacked_windows(&session, &app);

    let overlay = Overlay::start(&session, &["shot", "-o", "-"], &["HEADLESS-1"]);
    let input = session.input();
    input.move_to(out, 200.0, 150.0).unwrap();
    input.button(keys::BTN_LEFT, true).unwrap();
    input.move_to(out, 400.0, 300.0).unwrap();
    input.button(keys::BTN_LEFT, false).unwrap();
    let png = overlay.finish().png();

    assert_eq!((png.width, png.height), (200, 150));
}

/// Dark, like the label pill behind a window's title.
fn is_dark(Rgb8(r, g, b): Rgb8) -> bool {
    r < 60 && g < 60 && b < 60
}

#[test]
fn hovering_tints_the_window_under_the_pointer_and_names_it() {
    let session = Session::start(&[OutputSpec::new(640, 480)]).unwrap();
    let app = session.client().unwrap();
    let out = session.output("HEADLESS-1");
    app.paint(out, &Pattern::solid(DESKTOP)).unwrap();
    two_stacked_windows(&session, &app);

    let overlay = Overlay::start(&session, &["shot", "-o", "-"], &["HEADLESS-1"]);
    let idle = session.screenshot("HEADLESS-1").unwrap();
    let dimmed_green = idle.pixel(110, 70);
    let dimmed_desktop = idle.pixel(50, 50);

    // Only the lower window is under (120, 70).
    let input = session.input();
    input.move_to(out, 120.0, 70.0).unwrap();
    let seen = screenshot_when(&session, "HEADLESS-1", |s| s.pixel(110, 70) != dimmed_green);
    let Rgb8(r, g, b) = seen.pixel(110, 70);
    assert!(
        g > dimmed_green.1 && b > 20 && r < g,
        "the hovered window is brighter and tinted: {r} {g} {b}"
    );
    assert_ne!(seen.pixel(99, 59), dimmed_desktop, "a border just outside");
    assert_eq!(seen.pixel(50, 50), dimmed_desktop, "the desktop stays dim");
    assert_eq!(
        seen.pixel(400, 250),
        idle.pixel(400, 250),
        "the other window is not highlighted"
    );
    // The title sits in a dark pill in the middle of what shows of it.
    assert!(
        (150..250).any(|x| is_dark(seen.pixel(x, 120))),
        "a title label in the middle of the window"
    );

    // Off the windows, the highlight goes.
    input.move_to(out, 600.0, 450.0).unwrap();
    screenshot_when(&session, "HEADLESS-1", |s| s == &idle);
    input.tap(keys::KEY_ESC).unwrap();
    assert_eq!(overlay.finish().status.code(), Some(1));
}

#[test]
fn without_a_provider_there_is_no_highlight_and_the_rest_still_works() {
    let session = Session::start(&[OutputSpec::new(640, 480)]).unwrap();
    let app = session.client().unwrap();
    let out = session.output("HEADLESS-1");
    app.paint(out, &Pattern::solid(DESKTOP)).unwrap();
    two_stacked_windows(&session, &app);
    let no_provider = |cmd: &mut std::process::Command| {
        cmd.env_remove("SWAYSOCK");
    };

    // Hover a window, then drag a Region on the desktop.
    let overlay = Overlay::start_with(
        &session,
        &["shot", "--no-copy", "-o", "-"],
        &["HEADLESS-1"],
        no_provider,
    );
    let idle = session.screenshot("HEADLESS-1").unwrap();
    let input = session.input();
    input.move_to(out, 120.0, 70.0).unwrap();
    input.move_to(out, 500.0, 350.0).unwrap();
    input.button(keys::BTN_LEFT, true).unwrap();
    input.move_to(out, 600.0, 420.0).unwrap();
    let seen = screenshot_when(&session, "HEADLESS-1", |s| s.pixel(550, 400) == DESKTOP);
    assert_eq!(seen.pixel(110, 70), idle.pixel(110, 70), "no highlight");
    input.button(keys::BTN_LEFT, false).unwrap();
    let done = overlay.finish();
    assert_eq!(done.stderr, "", "no errors without a provider");
    let png = done.png();
    assert_eq!((png.width, png.height), (100, 70));

    // A click on a window takes its Output instead.
    let overlay = Overlay::start_with(
        &session,
        &["shot", "--no-copy", "-o", "-"],
        &["HEADLESS-1"],
        no_provider,
    );
    input.move_to(out, 120.0, 70.0).unwrap();
    input.click(keys::BTN_LEFT).unwrap();
    let done = overlay.finish();
    assert_eq!(done.stderr, "", "no errors without a provider");
    let png = done.png();
    assert_eq!((png.width, png.height), (640, 480));
}
