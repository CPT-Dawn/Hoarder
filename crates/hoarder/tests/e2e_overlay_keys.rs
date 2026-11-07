//! The Overlay's other Targets and precision tools (PLAN.md §4.2): O and a
//! click on empty desktop for the Output, A for All Outputs, Enter for the
//! last Region, Space to move a drag, arrows to nudge, and Refine.

mod common;

use common::{Overlay, as_8bit};

use std::time::{Duration, Instant};

use hoarder_e2e::{Client, Image, OutputSpec, Pattern, Rgb8, Session, keys};

/// Asserts the PNG is exactly `pattern` as painted on an 8-bit output of
/// physical size `size`, starting at physical pixel `origin`.
fn assert_cut_from(
    png: &common::Png,
    pattern: &Pattern,
    size: (u32, u32),
    origin: (u32, u32),
    what: &str,
) {
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
    assert!(wrong.is_empty(), "{what}: pixels differ: {wrong:#?}");
}

/// Two outputs side by side: a 320×240 Ramp and a 400×300 Checker.
/// The painting client is returned because its surfaces go when it does.
fn two_outputs() -> (Session, Client) {
    let session = Session::start(&[OutputSpec::new(320, 240), OutputSpec::new(400, 300)]).unwrap();
    let app = session.client().unwrap();
    app.paint(session.output("HEADLESS-1"), &Pattern::Ramp)
        .unwrap();
    app.paint(session.output("HEADLESS-2"), &Pattern::Checker { cell: 1 })
        .unwrap();
    (session, app)
}

fn start(session: &Session, args: &[&str]) -> Overlay {
    let outputs: Vec<&str> = session.outputs().iter().map(|o| o.name.as_str()).collect();
    Overlay::start(session, args, &outputs)
}

#[test]
fn a_captures_all_outputs() {
    let (session, _app) = two_outputs();
    let overlay = start(&session, &["shot", "-o", "-"]);
    session.input().tap(keys::KEY_A).unwrap();
    let png = overlay.finish().png();

    assert_eq!((png.width, png.height), (720, 300));
    assert_cut_from(
        &png.crop(0, 0, 320, 240),
        &Pattern::Ramp,
        (320, 240),
        (0, 0),
        "left output",
    );
    assert_cut_from(
        &png.crop(320, 0, 400, 300),
        &Pattern::Checker { cell: 1 },
        (400, 300),
        (0, 0),
        "right output",
    );
}

#[test]
fn o_captures_the_output_under_the_pointer() {
    let (session, _app) = two_outputs();
    let overlay = start(&session, &["shot", "-o", "-"]);
    let input = session.input();
    input
        .move_to(session.output("HEADLESS-2"), 50.0, 50.0)
        .unwrap();
    input.tap(keys::KEY_O).unwrap();
    let png = overlay.finish().png();

    assert_eq!((png.width, png.height), (400, 300));
    assert_cut_from(
        &png,
        &Pattern::Checker { cell: 1 },
        (400, 300),
        (0, 0),
        "the right output",
    );
}

#[test]
fn a_click_on_empty_desktop_captures_that_output() {
    let (session, _app) = two_outputs();
    let overlay = start(&session, &["shot", "-o", "-"]);
    let input = session.input();
    input
        .move_to(session.output("HEADLESS-1"), 100.0, 100.0)
        .unwrap();
    input.click(keys::BTN_LEFT).unwrap();
    let png = overlay.finish().png();

    assert_eq!((png.width, png.height), (320, 240));
    assert_cut_from(&png, &Pattern::Ramp, (320, 240), (0, 0), "the left output");
}

/// Drag from `from` to `to` on `output` with the left button.
fn drag(session: &Session, output: &str, from: (f64, f64), to: (f64, f64)) {
    let input = session.input();
    let out = session.output(output);
    input.move_to(out, from.0, from.1).unwrap();
    input.button(keys::BTN_LEFT, true).unwrap();
    input.move_to(out, to.0, to.1).unwrap();
    input.button(keys::BTN_LEFT, false).unwrap();
}

#[test]
fn enter_with_no_selection_captures_the_last_region_of_an_earlier_run() {
    let (session, _app) = two_outputs();
    let first = start(&session, &["shot", "-o", "-"]);
    drag(&session, "HEADLESS-1", (300.0, 100.0), (319.0, 160.0));
    let png = first.finish().png();
    assert_eq!((png.width, png.height), (19, 60));

    // A new process: the Region came through state.toml.
    let second = start(&session, &["shot", "-o", "-"]);
    session.input().tap(keys::KEY_ENTER).unwrap();
    let png = second.finish().png();
    assert_eq!((png.width, png.height), (19, 60));
    assert_cut_from(&png, &Pattern::Ramp, (320, 240), (300, 100), "last Region");
}

#[test]
fn enter_with_no_selection_and_no_last_region_does_nothing() {
    let (session, _app) = two_outputs();
    let overlay = start(&session, &["shot", "-o", "-"]);
    session.input().tap(keys::KEY_ENTER).unwrap();
    // Still up: Esc is what ends it.
    session.input().tap(keys::KEY_ESC).unwrap();
    let done = overlay.finish();
    assert_eq!(done.status.code(), Some(1), "exit code: {}", done.stderr);
}

#[test]
fn holding_space_while_dragging_moves_the_selection_instead_of_resizing_it() {
    let (session, _app) = two_outputs();
    let overlay = start(&session, &["shot", "-o", "-"]);
    let input = session.input();
    let out = session.output("HEADLESS-1");
    input.move_to(out, 20.0, 20.0).unwrap();
    input.button(keys::BTN_LEFT, true).unwrap();
    input.move_to(out, 100.0, 80.0).unwrap(); // (20, 20)-(100, 80)
    input.key_down(keys::KEY_SPACE).unwrap();
    input.move_to(out, 150.0, 110.0).unwrap(); // moved to (70, 50)-(150, 110)
    input.key_up(keys::KEY_SPACE).unwrap();
    input.move_to(out, 200.0, 130.0).unwrap(); // resized to (70, 50)-(200, 130)
    input.button(keys::BTN_LEFT, false).unwrap();
    let png = overlay.finish().png();

    assert_eq!((png.width, png.height), (130, 80));
    assert_cut_from(&png, &Pattern::Ramp, (320, 240), (70, 50), "moved Region");
}

#[test]
fn arrows_nudge_the_dragged_corner_by_1_px_and_shift_arrows_by_10() {
    let (session, _app) = two_outputs();
    let overlay = start(&session, &["shot", "-o", "-"]);
    let input = session.input();
    let out = session.output("HEADLESS-1");
    input.move_to(out, 20.0, 20.0).unwrap();
    input.button(keys::BTN_LEFT, true).unwrap();
    input.move_to(out, 100.0, 80.0).unwrap();
    input.tap(keys::KEY_RIGHT).unwrap();
    input.tap(keys::KEY_RIGHT).unwrap();
    input.chord(&[keys::KEY_LEFTSHIFT, keys::KEY_DOWN]).unwrap();
    input.tap(keys::KEY_UP).unwrap();
    input.button(keys::BTN_LEFT, false).unwrap();
    let png = overlay.finish().png();

    // 80×60, then +2 wide, +10 and -1 high.
    assert_eq!((png.width, png.height), (82, 69));
    assert_cut_from(&png, &Pattern::Ramp, (320, 240), (20, 20), "nudged Region");
}

/// The accent colour of the selection border and Refine's handles: Midnight
/// Bloom's, the theme when Zenith is not configured.
const ACCENT: Rgb8 = Rgb8(0x8d, 0xe3, 0xdc);

/// Poll grim until `ready` holds for `output`'s screenshot (at most 5 s).
fn screenshot_when(session: &Session, output: &str, ready: impl Fn(&Image) -> bool) -> Image {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let shot = session.screenshot(output).unwrap();
        if ready(&shot) {
            return shot;
        }
        assert!(Instant::now() < deadline, "the Overlay never got there");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn shift_release_enters_refine_where_handles_resize_and_enter_captures() {
    let (session, _app) = two_outputs();
    let overlay = start(&session, &["shot", "-o", "-"]);
    let input = session.input();
    let out = session.output("HEADLESS-1");
    input.move_to(out, 50.0, 40.0).unwrap();
    input.button(keys::BTN_LEFT, true).unwrap();
    input.move_to(out, 150.0, 120.0).unwrap();
    input.key_down(keys::KEY_LEFTSHIFT).unwrap();
    input.button(keys::BTN_LEFT, false).unwrap();
    input.key_up(keys::KEY_LEFTSHIFT).unwrap();

    // Refine: the selection stays, with handles on its corners and sides,
    // reaching past the border.
    let seen = screenshot_when(&session, "HEADLESS-1", |s| s.pixel(152, 122) == ACCENT);
    assert_eq!(seen.pixel(47, 37), ACCENT, "top-left handle");
    assert_eq!(seen.pixel(100, 36), ACCENT, "top handle");

    // Drag the bottom-right handle out, then the left edge in.
    drag(&session, "HEADLESS-1", (150.0, 120.0), (200.0, 140.0));
    drag(&session, "HEADLESS-1", (50.0, 80.0), (60.0, 80.0));
    // The left edge is the active handle now: arrows move it.
    input.tap(keys::KEY_LEFT).unwrap();
    input.tap(keys::KEY_ENTER).unwrap();
    let png = overlay.finish().png();

    assert_eq!((png.width, png.height), (141, 100));
    assert_cut_from(&png, &Pattern::Ramp, (320, 240), (59, 40), "refined Region");
}

#[test]
fn with_screenshot_confirm_every_release_enters_refine() {
    let (session, _app) = two_outputs();
    let overlay = start(
        &session,
        &["shot", "-o", "-", "--set", "screenshot.confirm=true"],
    );
    drag(&session, "HEADLESS-1", (10.0, 10.0), (90.0, 70.0));
    // A drag inside the selection moves it.
    drag(&session, "HEADLESS-1", (50.0, 40.0), (60.0, 45.0));
    session.input().tap(keys::KEY_ENTER).unwrap();
    let png = overlay.finish().png();

    assert_eq!((png.width, png.height), (80, 60));
    assert_cut_from(
        &png,
        &Pattern::Ramp,
        (320, 240),
        (20, 15),
        "confirmed Region",
    );
}

/// Enter Refine on HEADLESS-1 with a Shift+release over (50, 40)–(150, 120).
fn into_refine(session: &Session) {
    let input = session.input();
    let out = session.output("HEADLESS-1");
    input.move_to(out, 50.0, 40.0).unwrap();
    input.button(keys::BTN_LEFT, true).unwrap();
    input.move_to(out, 150.0, 120.0).unwrap();
    input.key_down(keys::KEY_LEFTSHIFT).unwrap();
    input.button(keys::BTN_LEFT, false).unwrap();
    input.key_up(keys::KEY_LEFTSHIFT).unwrap();
}

/// The accent-filled part of the Refine toolbar: the focused action's pill,
/// on the row of the size label below the selection.
fn focused_pill(shot: &Image) -> Option<u32> {
    (160..320).find(|&x| (125..150).any(|y| shot.pixel(x, y) == ACCENT))
}

#[test]
fn tab_moves_through_the_refine_toolbar_and_enter_runs_the_focused_action() {
    let (session, _app) = two_outputs();
    let overlay = start(&session, &["shot", "-o", "-"]);
    into_refine(&session);
    // Capture is focused first: its pill is filled with the accent.
    let first = screenshot_when(&session, "HEADLESS-1", |s| focused_pill(s).is_some());
    let at_capture = focused_pill(&first).unwrap();

    session.input().tap(keys::KEY_TAB).unwrap();
    let second = screenshot_when(&session, "HEADLESS-1", |s| {
        focused_pill(s).is_some_and(|x| x > at_capture)
    });
    let at_cursor = focused_pill(&second).unwrap();
    session.input().tap(keys::KEY_TAB).unwrap();
    screenshot_when(&session, "HEADLESS-1", |s| {
        focused_pill(s).is_some_and(|x| x > at_cursor)
    });
    // Third is Cancel.
    session.input().tap(keys::KEY_ENTER).unwrap();

    assert_eq!(overlay.finish().status.code(), Some(1), "cancelled");
}

#[test]
fn shift_tab_wraps_round_to_the_last_action() {
    let (session, _app) = two_outputs();
    let overlay = start(&session, &["shot", "-o", "-"]);
    into_refine(&session);
    screenshot_when(&session, "HEADLESS-1", |s| focused_pill(s).is_some());
    session
        .input()
        .chord(&[keys::KEY_LEFTSHIFT, keys::KEY_TAB])
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    session.input().tap(keys::KEY_ENTER).unwrap();

    assert_eq!(overlay.finish().status.code(), Some(1), "cancelled");
}
