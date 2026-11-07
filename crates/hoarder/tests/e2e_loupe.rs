//! The Overlay's helpers: the Loupe (an 8× grid of the Frozen Frame around
//! the pointer) and the hint bar, which retires after 20 uses.

mod common;

use common::{Overlay, as_8bit};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use hoarder_e2e::{Client, Image, OutputSpec, Pattern, Rgb8, Session, keys};

/// The Loupe's grid: 15×15 cells of 8 logical px, framed by a 1 px border,
/// with its top-left corner 24 logical px right of and below the pointer.
const CELL: u32 = 8;
const CELLS: u32 = 15;
const OFFSET: u32 = 24;

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

/// The top-left pixel of the Loupe's grid cell `(cx, cy)` for a pointer at
/// `(px, py)` on a scale-1 output.
fn cell_origin(px: u32, py: u32, cx: u32, cy: u32) -> (u32, u32) {
    (px + OFFSET + 1 + cx * CELL, py + OFFSET + 1 + cy * CELL)
}

/// Every cell of the Loupe shows its Frozen Frame pixel, magnified; checks the
/// inside of each cell (the crosshair may draw on the edges).
fn loupe_errors(seen: &Image, pattern: &Pattern, size: (u32, u32), at: (u32, u32)) -> Vec<String> {
    let mut wrong = Vec::new();
    for cy in 0..CELLS {
        for cx in 0..CELLS {
            let (sx, sy) = (at.0 + cx - CELLS / 2, at.1 + cy - CELLS / 2);
            let want = as_8bit(pattern.colour_at(sx, sy, size.0, size.1));
            let (x0, y0) = cell_origin(at.0, at.1, cx, cy);
            for (dx, dy) in [(2, 2), (5, 2), (2, 5), (5, 5)] {
                let got = seen.pixel(x0 + dx, y0 + dy);
                if got != want && wrong.len() < 5 {
                    wrong.push(format!("cell ({cx}, {cy}) shows {got:?}, want {want:?}"));
                }
            }
        }
    }
    wrong
}

#[test]
fn the_loupe_shows_the_exact_colour_under_the_pointer_after_300ms_of_stillness() {
    let session = Session::start(&[OutputSpec::new(640, 480)]).unwrap();
    let app = session.client().unwrap();
    let out = session.output("HEADLESS-1");
    // Every pixel differs from its neighbours.
    let pattern = Pattern::Ramp;
    app.paint(out, &pattern).unwrap();

    let overlay = Overlay::start(&session, &["shot", "region", "-o", "-"], &["HEADLESS-1"]);
    let input = session.input();
    input.move_to(out, 200.0, 150.0).unwrap();
    input.move_to(out, 300.0, 200.0).unwrap();
    let centre = cell_origin(300, 200, CELLS / 2, CELLS / 2);
    let want = as_8bit(pattern.colour_at(300, 200, 640, 480));
    let seen = screenshot_when(&session, "HEADLESS-1", |s| {
        s.pixel(centre.0 + 3, centre.1 + 3) == want
    });
    let wrong = loupe_errors(&seen, &pattern, (640, 480), (300, 200));
    assert!(
        wrong.is_empty(),
        "the Loupe magnifies the frame: {wrong:#?}"
    );

    input.tap(keys::KEY_ESC).unwrap();
    assert_eq!(overlay.finish().status.code(), Some(1));
}

#[test]
fn holding_alt_shows_the_loupe_while_the_pointer_moves() {
    let session = Session::start(&[OutputSpec::new(640, 480)]).unwrap();
    let app = session.client().unwrap();
    let out = session.output("HEADLESS-1");
    let pattern = Pattern::Ramp;
    app.paint(out, &pattern).unwrap();

    let overlay = Overlay::start(&session, &["shot", "region", "-o", "-"], &["HEADLESS-1"]);
    let input = session.input();
    input.key_down(keys::KEY_LEFTALT).unwrap();
    // Keep moving, never still for 300 ms, between two spots.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut found = None;
    let mut step = 0u32;
    while found.is_none() && Instant::now() < deadline {
        let x = 300 + step % 2;
        input.move_to(out, x as f64, 200.0).unwrap();
        let seen = session.screenshot("HEADLESS-1").unwrap();
        if loupe_errors(&seen, &pattern, (640, 480), (x, 200)).is_empty() {
            found = Some(x);
        }
        step += 1;
    }
    input.key_up(keys::KEY_LEFTALT).unwrap();
    assert!(found.is_some(), "the Loupe shows up while Alt is held");

    input.tap(keys::KEY_ESC).unwrap();
    assert_eq!(overlay.finish().status.code(), Some(1));
}

fn state_file(session: &Session) -> PathBuf {
    session
        .runtime_dir()
        .join("home/.local/state/hoarder/state.toml")
}

/// White, dimmed to 55% by the Overlay.
fn is_dimmed_white(p: Rgb8) -> bool {
    (138..=142).contains(&p.0) && p.0 == p.1 && p.1 == p.2
}

/// How many pixels of the band where the hint bar sits (centred, near the
/// bottom) are not the dimmed white frame.
fn hint_pixels(seen: &Image) -> usize {
    let (w, h) = seen.size();
    (h - 60..h - 20)
        .flat_map(|y| (w / 4..w * 3 / 4).map(move |x| (x, y)))
        .filter(|&(x, y)| !is_dimmed_white(seen.pixel(x, y)))
        .count()
}

fn hint_shown(seen: &Image) -> bool {
    hint_pixels(seen) > 500
}

fn hint_gone(seen: &Image) -> bool {
    hint_pixels(seen) == 0
}

/// A white output with the pointer in its top-left corner, out of the way.
fn white_session() -> (Session, Client) {
    let session = Session::start(&[OutputSpec::new(640, 480)]).unwrap();
    let app = session.client().unwrap();
    let out = session.output("HEADLESS-1");
    app.paint(out, &Pattern::solid(Rgb8(255, 255, 255)))
        .unwrap();
    session.input().move_to(out, 5.0, 5.0).unwrap();
    (session, app)
}

#[test]
fn the_hint_bar_fades_after_1_5s_and_counts_a_use() {
    let (session, _app) = white_session();
    let started = Instant::now();
    let overlay = Overlay::start(&session, &["shot", "region", "-o", "-"], &["HEADLESS-1"]);
    screenshot_when(&session, "HEADLESS-1", hint_shown);
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "the hint bar shows as the Overlay opens"
    );
    screenshot_when(&session, "HEADLESS-1", hint_gone);
    assert!(
        started.elapsed() >= Duration::from_millis(1500),
        "the hint bar stays 1.5 s"
    );

    session.input().tap(keys::KEY_ESC).unwrap();
    overlay.finish();
    let state = std::fs::read_to_string(state_file(&session)).unwrap();
    assert!(state.contains("hint_bar_uses = 1"), "state.toml: {state}");
}

#[test]
fn the_hint_bar_retires_after_20_uses() {
    let (session, _app) = white_session();
    let path = state_file(&session);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "# kept\nhint_bar_uses = 19\n").unwrap();

    // The 20th use still shows it.
    let overlay = Overlay::start(&session, &["shot", "region", "-o", "-"], &["HEADLESS-1"]);
    screenshot_when(&session, "HEADLESS-1", hint_shown);
    session.input().tap(keys::KEY_ESC).unwrap();
    overlay.finish();
    let state = std::fs::read_to_string(&path).unwrap();
    assert!(state.contains("hint_bar_uses = 20"), "state.toml: {state}");
    assert!(state.contains("# kept"), "comments survive: {state}");

    // After that, never again.
    let overlay = Overlay::start(&session, &["shot", "region", "-o", "-"], &["HEADLESS-1"]);
    let until = Instant::now() + Duration::from_millis(600);
    while Instant::now() < until {
        let seen = session.screenshot("HEADLESS-1").unwrap();
        assert!(hint_gone(&seen), "no hint bar after 20 uses");
    }
    session.input().tap(keys::KEY_ESC).unwrap();
    overlay.finish();
}
