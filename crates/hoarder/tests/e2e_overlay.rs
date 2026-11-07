//! `hoarder shot region`: the Overlay, driven by the virtual pointer and
//! keyboard, yields a Region cut byte for byte from the Frozen Frames.

mod common;

use common::{Overlay, as_8bit};
use std::time::{Duration, Instant};

use hoarder_e2e::{
    BitDepth, Image, OutputSpec, Pattern, PixelFormat, Rgb8, Rgb10, Selection, Session, Transform,
    keys,
};

/// Asserts the `w`×`h` PNG is exactly `pattern` as painted on an 8-bit output
/// of physical size `size`, starting at physical pixel `origin`.
fn assert_cut_from(
    png: &common::Png,
    pattern: &Pattern,
    size: (u32, u32),
    origin: (u32, u32),
    what: &str,
) {
    assert_eq!(png.depth, 8, "{what}: an 8-bit output gives an 8-bit PNG");
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

#[test]
fn dragging_captures_the_byte_exact_region() {
    let session = Session::start(&[OutputSpec::new(640, 480)]).unwrap();
    let app = session.client().unwrap();
    let out = session.output("HEADLESS-1");
    app.paint(out, &Pattern::Ramp).unwrap();

    let overlay = Overlay::start(&session, &["shot", "region", "-o", "-"], &["HEADLESS-1"]);
    let input = session.input();
    input.move_to(out, 100.0, 50.0).unwrap();
    input.button(keys::BTN_LEFT, true).unwrap();
    input.move_to(out, 250.0, 130.0).unwrap();
    input.move_to(out, 300.0, 250.0).unwrap();
    input.button(keys::BTN_LEFT, false).unwrap();
    let png = overlay.finish().png();

    assert_eq!((png.width, png.height), (200, 200));
    assert_cut_from(&png, &Pattern::Ramp, (640, 480), (100, 50), "region");
}

#[test]
fn a_drag_across_two_outputs_yields_one_region() {
    let session = Session::start(&[OutputSpec::new(320, 240), OutputSpec::new(400, 300)]).unwrap();
    let app = session.client().unwrap();
    let (left, right) = (session.output("HEADLESS-1"), session.output("HEADLESS-2"));
    app.paint(left, &Pattern::Ramp).unwrap();
    app.paint(right, &Pattern::Checker { cell: 1 }).unwrap();

    // No Target given: Region is the default.
    let overlay = Overlay::start(
        &session,
        &["shot", "-o", "-"],
        &["HEADLESS-1", "HEADLESS-2"],
    );
    let input = session.input();
    input.move_to(left, 300.0, 100.0).unwrap();
    input.button(keys::BTN_LEFT, true).unwrap();
    input.move_to(left, 319.0, 120.0).unwrap();
    input.move_to(right, 30.0, 160.0).unwrap();
    input.button(keys::BTN_LEFT, false).unwrap();
    let png = overlay.finish().png();

    // The last 20 columns of the left output, then the first 30 of the right one.
    assert_eq!((png.width, png.height), (50, 60));
    let (l, r) = (png.crop(0, 0, 20, 60), png.crop(20, 0, 30, 60));
    assert_cut_from(&l, &Pattern::Ramp, (320, 240), (300, 100), "left output");
    assert_cut_from(
        &r,
        &Pattern::Checker { cell: 1 },
        (400, 300),
        (0, 100),
        "right output",
    );
}

/// The accent colour of the selection border: Midnight Bloom's, the theme
/// when Zenith is not configured.
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
fn the_overlay_shows_the_frozen_frame_1_to_1_dimmed_outside_the_selection_at_scale_1_25() {
    let session = Session::start(&[OutputSpec::new(600, 480).scale(1.25)]).unwrap();
    let app = session.client().unwrap();
    let out = session.output("HEADLESS-1");
    let checker = Pattern::Checker { cell: 1 };
    app.paint(out, &checker).unwrap();

    let overlay = Overlay::start(&session, &["shot", "region", "-o", "-"], &["HEADLESS-1"]);
    let input = session.input();
    // Logical (40, 40)-(200, 120) is physical (50, 50)-(250, 150).
    input.move_to(out, 40.0, 40.0).unwrap();
    input.button(keys::BTN_LEFT, true).unwrap();
    input.move_to(out, 200.0, 120.0).unwrap();
    let seen = screenshot_when(&session, "HEADLESS-1", |s| s.pixel(250, 100) == ACCENT);

    assert_eq!(seen.size(), (600, 480), "grim sees physical pixels");
    let mut wrong = Vec::new();
    for y in 50..150 {
        for x in 50..250 {
            let want = as_8bit(checker.colour_at(x, y, 600, 480));
            if seen.pixel(x, y) != want && wrong.len() < 5 {
                wrong.push(format!("({x}, {y}): {:?}, want {want:?}", seen.pixel(x, y)));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "the selection shows the frame 1:1: {wrong:#?}"
    );
    for (x, y) in [(49, 49), (250, 150), (49, 100), (150, 49)] {
        assert_eq!(seen.pixel(x, y), ACCENT, "border pixel ({x}, {y})");
    }
    // Outside the selection, white is dimmed to 55% and black stays black.
    let Rgb8(r, g, b) = seen.pixel(10, 10);
    assert!(
        (138..=142).contains(&r) && r == g && g == b,
        "dimmed white: {r} {g} {b}"
    );
    assert_eq!(seen.pixel(11, 10), Rgb8(0, 0, 0), "dimmed black");
    // The size pill sits just below the selection's left edge: dark, where
    // the dimmed checkerboard alternates 140 and 0.
    let pill = [seen.pixel(51, 170), seen.pixel(52, 170)];
    assert!(
        pill.iter().all(|p| p.0 < 60),
        "a size pill below the selection: {pill:?}"
    );

    input.button(keys::BTN_LEFT, false).unwrap();
    let png = overlay.finish().png();
    assert_eq!((png.width, png.height), (200, 100));
    assert_cut_from(&png, &checker, (600, 480), (50, 50), "region at scale 1.25");
}

#[test]
fn a_rotated_output_shows_and_captures_its_frame_upright() {
    let spec = OutputSpec::new(640, 480).transform(Transform::Rotate90);
    let session = Session::start(&[spec]).unwrap();
    let app = session.client().unwrap();
    let out = session.output("HEADLESS-1");
    app.paint(out, &Pattern::Quadrants).unwrap();
    let size = spec.physical_size();

    let overlay = Overlay::start(&session, &["shot", "region", "-o", "-"], &["HEADLESS-1"]);
    let input = session.input();
    input.move_to(out, 100.0, 100.0).unwrap();
    input.button(keys::BTN_LEFT, true).unwrap();
    input.move_to(out, 380.0, 540.0).unwrap();
    let seen = screenshot_when(&session, "HEADLESS-1", |s| s.pixel(380, 300) == ACCENT);
    for (x, y) in [(101, 101), (378, 101), (101, 538), (378, 538)] {
        assert_eq!(
            seen.pixel(x, y),
            as_8bit(Pattern::Quadrants.colour_at(x, y, size.0, size.1)),
            "the Overlay shows the frame upright at ({x}, {y})"
        );
    }
    input.button(keys::BTN_LEFT, false).unwrap();
    let png = overlay.finish().png();

    assert_eq!((png.width, png.height), (280, 440));
    assert_cut_from(
        &png,
        &Pattern::Quadrants,
        size,
        (100, 100),
        "rotated region",
    );
}

#[test]
fn a_region_of_a_10bit_output_keeps_every_bit() {
    let session = Session::start(&[OutputSpec::new(640, 480).depth(BitDepth::Ten)]).unwrap();
    let app = session.client().unwrap();
    let out = session.output("HEADLESS-1");
    app.paint_as(out, &Pattern::Ramp, PixelFormat::Xrgb2101010)
        .unwrap();

    let overlay = Overlay::start(&session, &["shot", "region", "-o", "-"], &["HEADLESS-1"]);
    let input = session.input();
    input.move_to(out, 10.0, 2.0).unwrap();
    input.button(keys::BTN_LEFT, true).unwrap();
    input.move_to(out, 600.0, 6.0).unwrap();
    input.button(keys::BTN_LEFT, false).unwrap();
    let png = overlay.finish().png();

    assert_eq!((png.width, png.height, png.depth), (590, 4, 16));
    for y in 0..4 {
        for x in 0..590 {
            let Rgb10(r, g, b) = Pattern::Ramp.colour_at(10 + x, 2 + y, 640, 480);
            assert_eq!(png.rgb(x, y).map(|v| v >> 6), [r, g, b], "({x}, {y})");
        }
    }
}

fn clipboard_text(session: &Session) -> Vec<u8> {
    let out = session
        .command("wl-paste")
        .args(["--no-newline", "--type", "text/plain"])
        .output()
        .expect("wl-paste should run");
    assert!(out.status.success(), "the clipboard should still be served");
    out.stdout
}

#[test]
fn esc_cancels_with_exit_code_1_and_leaves_the_clipboard_alone() {
    let session = Session::start_default().unwrap();
    let app = session.client().unwrap();
    let out = session.output("HEADLESS-1");
    app.paint(out, &Pattern::Quadrants).unwrap();
    app.set_selection(
        Selection::Clipboard,
        &[("text/plain", b"keep me".as_slice())],
    )
    .unwrap();

    let overlay = Overlay::start(&session, &["shot", "region", "-o", "-"], &["HEADLESS-1"]);
    let input = session.input();
    input.move_to(out, 10.0, 10.0).unwrap();
    input.button(keys::BTN_LEFT, true).unwrap();
    input.move_to(out, 200.0, 100.0).unwrap();
    input.tap(keys::KEY_ESC).unwrap();
    let done = overlay.finish();

    assert_eq!(done.status.code(), Some(1), "exit code: {}", done.stderr);
    assert!(done.stdout.is_empty(), "nothing is written when cancelled");
    assert_eq!(clipboard_text(&session), b"keep me");
}

#[test]
fn right_click_cancels_with_exit_code_1() {
    let session = Session::start_default().unwrap();
    let app = session.client().unwrap();
    let out = session.output("HEADLESS-1");
    app.paint(out, &Pattern::Quadrants).unwrap();

    let overlay = Overlay::start(&session, &["shot", "region", "-o", "-"], &["HEADLESS-1"]);
    let input = session.input();
    input.move_to(out, 10.0, 10.0).unwrap();
    input.click(keys::BTN_RIGHT).unwrap();
    let done = overlay.finish();

    assert_eq!(done.status.code(), Some(1), "exit code: {}", done.stderr);
    assert!(done.stdout.is_empty(), "nothing is written when cancelled");
}

/// Budget (PLAN.md §12): key press to Overlay visible on 4K under 60 ms.
/// Measured by hoarder itself, from start to the first frame callback of
/// every Overlay surface. Only enforced in optimised builds; run with
/// `cargo test --release --test e2e_overlay budget -- --nocapture`.
#[test]
fn budget_overlay_visible_on_4k() {
    let session = Session::start(&[OutputSpec::new(3840, 2160)]).unwrap();
    let app = session.client().unwrap();
    app.paint(session.output("HEADLESS-1"), &Pattern::Quadrants)
        .unwrap();

    let mut samples = Vec::new();
    for _ in 0..5 {
        let overlay = Overlay::start_with(
            &session,
            &["shot", "region", "-o", "-"],
            &["HEADLESS-1"],
            |cmd| {
                cmd.env("HOARDER_TIMINGS", "1");
            },
        );
        session.input().tap(keys::KEY_ESC).unwrap();
        let done = overlay.finish();
        eprintln!("{}", done.stderr.trim_end());
        let ms: f64 = done
            .stderr
            .lines()
            .find_map(|l| l.split("overlay_visible_ms=").nth(1))
            .unwrap_or_else(|| panic!("no timing printed: {}", done.stderr))
            .trim()
            .parse()
            .unwrap();
        samples.push(ms);
    }
    samples.sort_by(f64::total_cmp);
    let median = samples[samples.len() / 2];
    eprintln!("key press -> Overlay visible on 4K: median {median:.1} ms of {samples:?}");
    if !cfg!(debug_assertions) {
        assert!(median < 60.0, "over the 60 ms budget: {samples:?}");
    }
}
