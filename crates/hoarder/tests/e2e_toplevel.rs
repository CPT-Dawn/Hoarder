//! Toplevel window capture (PLAN.md §4.1, Window Targets): Shift+click, or
//! `screenshot.window_source = "toplevel"`, takes the window's own buffer
//! through the ext foreign-toplevel capture source, whole even where another
//! window covers it; `hoarder shot window --list` picks any window by title.

mod common;

use common::{Overlay, Png};
use std::process::Output as Run;
use std::time::{Duration, Instant};

use hoarder_e2e::{Client, Image, Output, OutputSpec, Pattern, Rgb8, Session, keys};

/// A desktop colour no window uses.
const DESKTOP: Rgb8 = Rgb8(200, 30, 30);
const GREEN: Rgb8 = Rgb8(0, 200, 0);
const YELLOW: Rgb8 = Rgb8(220, 220, 0);
const BLUE: Rgb8 = Rgb8(20, 40, 220);

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
fn screenshot_when(session: &Session, output: &str, ready: impl Fn(&Image) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready(&session.screenshot(output).unwrap()) {
        assert!(Instant::now() < deadline, "{output} never got there");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Open a `colour` probe window titled `app_id` on `output`, float it with
/// `commands` and wait until its content shows at `content` (physical
/// pixels of `output`).
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

/// Green `below` at (100, 60) 200×120, mostly covered by yellow `above`
/// at (150, 100) 300×200: only the strip y < 100 and x < 150 of `below`
/// shows.
fn covered_window(session: &Session, app: &Client) {
    let out = session.output("HEADLESS-1");
    app.paint(out, &Pattern::solid(DESKTOP)).unwrap();
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

/// Run `hoarder` to completion with `args`.
fn hoarder(session: &Session, args: &[&str]) -> Run {
    session
        .command(env!("CARGO_BIN_EXE_hoarder"))
        .args(args)
        .output()
        .expect("hoarder runs")
}

fn png_of(run: &Run) -> Png {
    assert!(
        run.status.success(),
        "hoarder failed ({}): {}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    Png::decode(&run.stdout)
}

#[test]
fn list_captures_a_covered_window_by_title() {
    let session = Session::start(&[OutputSpec::new(640, 480)]).unwrap();
    let app = session.client().unwrap();
    covered_window(&session, &app);

    let run = hoarder(
        &session,
        &[
            "shot",
            "window",
            "--list",
            "probe.below",
            "--no-copy",
            "-o",
            "-",
        ],
    );
    let png = png_of(&run);
    assert_eq!((png.width, png.height), (200, 120));
    assert_all(&png, GREEN, "the whole covered window");
}

/// Click the visible strip of the covered window, Shift held if `shift`.
fn click_covered_window(session: &Session, extra: &[&str], shift: bool) -> Png {
    let mut args = vec!["shot", "--no-copy", "-o", "-"];
    args.extend_from_slice(extra);
    let overlay = Overlay::start(session, &args, &["HEADLESS-1"]);
    let input = session.input();
    input
        .move_to(session.output("HEADLESS-1"), 120.0, 70.0)
        .unwrap();
    if shift {
        input.key_down(keys::KEY_LEFTSHIFT).unwrap();
    }
    input.click(keys::BTN_LEFT).unwrap();
    if shift {
        input.key_up(keys::KEY_LEFTSHIFT).unwrap();
    }
    overlay.finish().png()
}

#[test]
fn shift_click_captures_a_covered_window_whole() {
    let session = Session::start(&[OutputSpec::new(640, 480)]).unwrap();
    let app = session.client().unwrap();
    covered_window(&session, &app);

    let png = click_covered_window(&session, &[], true);
    assert_eq!((png.width, png.height), (200, 120));
    assert_all(&png, GREEN, "the window's own buffer, nothing over it");
}

#[test]
fn window_source_toplevel_makes_a_plain_click_take_the_own_buffer() {
    let session = Session::start(&[OutputSpec::new(640, 480)]).unwrap();
    let app = session.client().unwrap();
    covered_window(&session, &app);

    let png = click_covered_window(
        &session,
        &["--set", "screenshot.window_source=toplevel"],
        false,
    );
    assert_eq!((png.width, png.height), (200, 120));
    assert_all(&png, GREEN, "the window's own buffer");
}

#[test]
fn list_alone_names_every_window_even_on_another_workspace() {
    let session = Session::start(&[OutputSpec::new(640, 480)]).unwrap();
    let app = session.client().unwrap();
    covered_window(&session, &app);
    session.swaymsg(&["workspace 2"]).unwrap();
    app.open_window_coloured("probe.away", BLUE).unwrap();
    session.swaymsg(&["workspace 1"]).unwrap();

    let run = hoarder(&session, &["shot", "window", "--list"]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let listed = String::from_utf8(run.stdout).unwrap();
    let mut lines: Vec<&str> = listed.lines().collect();
    lines.sort();
    assert_eq!(
        lines,
        [
            "probe.above\tprobe.above",
            "probe.away\tprobe.away",
            "probe.below\tprobe.below"
        ]
    );
}

#[test]
fn list_captures_a_window_on_another_workspace() {
    let session = Session::start(&[OutputSpec::new(640, 480)]).unwrap();
    let app = session.client().unwrap();
    let out = session.output("HEADLESS-1");
    app.paint(out, &Pattern::solid(DESKTOP)).unwrap();
    session.swaymsg(&["workspace 2"]).unwrap();
    float(
        &session,
        &app,
        out,
        ("probe.away", BLUE),
        "border none, resize set 160 90, move absolute position 20 20",
        (20, 20, 160, 90),
    );
    session.swaymsg(&["workspace 1"]).unwrap();
    screenshot_when(&session, "HEADLESS-1", |s| bbox(s, BLUE).is_none());

    let run = hoarder(
        &session,
        &["shot", "window", "--list", "AWAY", "--no-copy", "-o", "-"],
    );
    let png = png_of(&run);
    assert_eq!((png.width, png.height), (160, 90));
    assert_all(&png, BLUE, "the hidden window, found by part of its title");
}

/// Sway hands over a toplevel at a fractional scale at its logical size,
/// not in the output's pixels: under `mixed_scale = "upscale"` it is
/// enlarged by nearest neighbour to match a Region of it, with the badge.
#[test]
fn a_logical_size_buffer_on_a_fractional_output_follows_mixed_scale() {
    let session = Session::start(&[OutputSpec::new(800, 600).scale(1.25)]).unwrap();
    let app = session.client().unwrap();
    let out = session.output("HEADLESS-1");
    app.paint(out, &Pattern::solid(DESKTOP)).unwrap();
    // Logical (40, 40) 160×96 is physical (50, 50) 200×120.
    float(
        &session,
        &app,
        out,
        ("probe.scaled", GREEN),
        "border none, resize set 160 96, move absolute position 40 40",
        (50, 50, 200, 120),
    );
    let take = |extra: &[&str]| {
        let mut args = vec!["shot", "window", "--list", "probe.scaled", "--no-copy"];
        args.extend_from_slice(extra);
        args.extend_from_slice(&["-o", "-"]);
        let run = hoarder(&session, &args);
        (
            png_of(&run),
            String::from_utf8_lossy(&run.stderr).into_owned(),
        )
    };

    let (png, stderr) = take(&[]);
    assert_eq!((png.width, png.height), (200, 120), "the output's pixels");
    assert_all(&png, GREEN, "enlarged, never blended");
    assert!(stderr.contains("mixed scale"), "the badge: {stderr}");

    let (png, stderr) = take(&["--set", "capture.mixed_scale=split"]);
    assert_eq!((png.width, png.height), (160, 96), "the buffer as it came");
    assert!(!stderr.contains("mixed scale"), "no badge: {stderr}");
}

#[test]
fn list_refuses_an_unknown_or_ambiguous_title() {
    let session = Session::start(&[OutputSpec::new(640, 480)]).unwrap();
    let app = session.client().unwrap();
    covered_window(&session, &app);

    let unknown = hoarder(&session, &["shot", "window", "--list", "nope", "-o", "-"]);
    assert_eq!(unknown.status.code(), Some(2));
    let ambiguous = hoarder(&session, &["shot", "window", "--list", "probe", "-o", "-"]);
    assert_eq!(ambiguous.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&ambiguous.stderr);
    assert!(
        stderr.contains("probe.above") && stderr.contains("probe.below"),
        "names the candidates: {stderr}"
    );
}
