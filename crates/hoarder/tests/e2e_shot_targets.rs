//! The rest of `hoarder shot`'s surface (PLAN.md §10): the `output` and
//! `last` Targets, `--confirm`, and `screenshot.freeze = false` as `--live`.

mod common;

use common::{Overlay, as_8bit};

use hoarder_e2e::{Client, OutputSpec, Pattern, Rgb8, Session, keys};

const HOARDER: &str = env!("CARGO_BIN_EXE_hoarder");

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

/// Two outputs side by side: a 320×240 Ramp and a 400×300 Checker.
fn two_outputs() -> (Session, Client) {
    let session = Session::start(&[OutputSpec::new(320, 240), OutputSpec::new(400, 300)]).unwrap();
    let app = session.client().unwrap();
    app.paint(session.output("HEADLESS-1"), &Pattern::Ramp)
        .unwrap();
    app.paint(session.output("HEADLESS-2"), &Pattern::Checker { cell: 1 })
        .unwrap();
    (session, app)
}

fn drag(session: &Session, output: &str, from: (f64, f64), to: (f64, f64)) {
    let input = session.input();
    let out = session.output(output);
    input.move_to(out, from.0, from.1).unwrap();
    input.button(keys::BTN_LEFT, true).unwrap();
    input.move_to(out, to.0, to.1).unwrap();
    input.button(keys::BTN_LEFT, false).unwrap();
}

/// `hoarder ARGS` without an Overlay, to completion.
fn run(session: &Session, args: &[&str]) -> std::process::Output {
    session.command(HOARDER).args(args).output().unwrap()
}

#[test]
fn the_confirm_flag_enters_refine_like_the_setting() {
    let (session, _app) = two_outputs();
    let overlay = Overlay::start(
        &session,
        &["shot", "--confirm", "-o", "-"],
        &["HEADLESS-1", "HEADLESS-2"],
    );
    drag(&session, "HEADLESS-1", (10.0, 10.0), (90.0, 70.0));
    // Refine: a drag inside the selection moves it rather than capturing.
    drag(&session, "HEADLESS-1", (50.0, 40.0), (60.0, 45.0));
    session.input().tap(keys::KEY_ENTER).unwrap();
    let png = overlay.finish().png();

    assert_eq!((png.width, png.height), (80, 60));
    assert_cut_from(&png, &Pattern::Ramp, (320, 240), (20, 15));
}

#[test]
fn freeze_false_selects_over_the_live_desktop_like_live() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    let app = session.client().unwrap();
    let out = session.output("HEADLESS-1");
    app.paint(out, &Pattern::solid(Rgb8(255, 255, 255)))
        .unwrap();
    let overlay = Overlay::start(
        &session,
        &["shot", "-o", "-", "--set", "screenshot.freeze=false"],
        &["HEADLESS-1"],
    );

    // Frozen, the Capture would still be white.
    app.paint(out, &Pattern::Ramp).unwrap();
    drag(&session, "HEADLESS-1", (10.0, 10.0), (90.0, 70.0));
    let png = overlay.finish().png();

    assert_eq!((png.width, png.height), (80, 60));
    assert_cut_from(&png, &Pattern::Ramp, (320, 240), (10, 10));
}

#[test]
fn shot_output_captures_the_focused_output_without_an_overlay() {
    let (session, _app) = two_outputs();
    session.swaymsg(&["focus", "output", "HEADLESS-2"]).unwrap();

    let out = run(&session, &["shot", "output", "-o", "-"]);

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let png = common::Png::decode(&out.stdout);
    assert_eq!((png.width, png.height), (400, 300));
    assert_cut_from(&png, &Pattern::Checker { cell: 1 }, (400, 300), (0, 0));
}

#[test]
fn shot_last_takes_the_last_region_again_without_an_overlay() {
    let (session, _app) = two_outputs();
    let none = run(&session, &["shot", "last", "-o", "-"]);
    assert_eq!(none.status.code(), Some(2), "no last Region yet");

    let first = Overlay::start(
        &session,
        &["shot", "-o", "-"],
        &["HEADLESS-1", "HEADLESS-2"],
    );
    drag(&session, "HEADLESS-1", (30.0, 20.0), (130.0, 70.0));
    assert_eq!(first.finish().png().width, 100);

    let again = run(&session, &["shot", "last", "-o", "-"]);

    assert!(
        again.status.success(),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
    let png = common::Png::decode(&again.stdout);
    assert_eq!((png.width, png.height), (100, 50));
    assert_cut_from(&png, &Pattern::Ramp, (320, 240), (30, 20));
}

#[test]
fn shot_last_with_an_off_screen_last_region_is_a_usage_error() {
    let (session, _app) = two_outputs();
    let state = session.runtime_dir().join("home/.local/state/hoarder");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(
        state.join("state.toml"),
        "[last_region]\nx = 5000.0\ny = 5000.0\nwidth = 100.0\nheight = 50.0\n",
    )
    .unwrap();

    let out = run(&session, &["shot", "last", "-o", "-"]);

    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("off-screen"), "{err}");
}
