//! `capture.mixed_scale` and `capture.depth` (PLAN.md §4.4), on a 1×
//! output next to a 2× one, and on a 10-bit output.

mod common;

use std::path::{Path, PathBuf};

use common::{Overlay, Png, as_8bit};
use hoarder_e2e::{BitDepth, Client, OutputSpec, Pattern, PixelFormat, Rgb8, Session, keys};

const LEFT: &str = "HEADLESS-1";
const RIGHT: &str = "HEADLESS-2";

/// A 320×240 1× output with a ramp, left of a 640×480 2× output (320×240
/// logical) with a one-pixel checkerboard.
fn one_by_two() -> (Session, Client) {
    let session = Session::start(&[
        OutputSpec::new(320, 240),
        OutputSpec::new(640, 480).scale(2.0),
    ])
    .unwrap();
    let app = session.client().unwrap();
    app.paint(session.output(LEFT), &Pattern::Ramp).unwrap();
    app.paint(session.output(RIGHT), &Pattern::Checker { cell: 1 })
        .unwrap();
    (session, app)
}

/// Asserts `png` is `pattern` as painted on an 8-bit output of physical
/// `size`, from physical pixel `origin`, each pixel enlarged `factor` times.
fn assert_cut_from(
    png: &Png,
    pattern: &Pattern,
    size: (u32, u32),
    origin: (u32, u32),
    factor: u32,
    what: &str,
) {
    assert_eq!(png.depth, 8, "{what}: an 8-bit output gives an 8-bit PNG");
    let mut wrong = Vec::new();
    for y in 0..png.height {
        for x in 0..png.width {
            let Rgb8(r, g, b) = as_8bit(pattern.colour_at(
                origin.0 + x / factor,
                origin.1 + y / factor,
                size.0,
                size.1,
            ));
            let want = [r as u16, g as u16, b as u16];
            let got = png.rgb(x, y);
            if got != want && wrong.len() < 5 {
                wrong.push(format!("({x}, {y}): got {got:?}, want {want:?}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{what}: pixels differ: {wrong:#?}");
}

/// Drag from logical (300, 100) on the left output to (30, 160) on the right
/// one: 20 logical columns on each side of the edge, then 30 more.
fn drag_across(session: &Session) {
    let input = session.input();
    let (left, right) = (session.output(LEFT), session.output(RIGHT));
    input.move_to(left, 300.0, 100.0).unwrap();
    input.button(keys::BTN_LEFT, true).unwrap();
    input.move_to(left, 319.0, 120.0).unwrap();
    input.move_to(right, 30.0, 160.0).unwrap();
    input.button(keys::BTN_LEFT, false).unwrap();
}

fn start(session: &Session, args: &[&str]) -> Overlay {
    Overlay::start(session, args, &[LEFT, RIGHT])
}

#[test]
fn upscale_enlarges_the_1x_part_by_nearest_neighbour_with_a_badge() {
    let (session, _painter) = one_by_two();
    let overlay = start(&session, &["shot", "region", "-o", "-"]);
    drag_across(&session);
    let done = overlay.finish();
    let png = done.png();

    // 50×60 logical at the highest scale, 2.
    assert_eq!((png.width, png.height), (100, 120));
    assert_cut_from(
        &png.crop(0, 0, 40, 120),
        &Pattern::Ramp,
        (320, 240),
        (300, 100),
        2,
        "1× part",
    );
    assert_cut_from(
        &png.crop(40, 0, 60, 120),
        &Pattern::Checker { cell: 1 },
        (640, 480),
        (0, 200),
        1,
        "2× part",
    );
    assert!(
        done.stderr.contains("mixed scale"),
        "the mixed scale badge: {}",
        done.stderr
    );
}

#[test]
fn a_region_on_the_1x_output_alone_stays_at_1x_without_a_badge() {
    let (session, _painter) = one_by_two();
    let overlay = start(&session, &["shot", "region", "-o", "-"]);
    let input = session.input();
    let left = session.output(LEFT);
    input.move_to(left, 10.0, 20.0).unwrap();
    input.button(keys::BTN_LEFT, true).unwrap();
    input.move_to(left, 110.0, 70.0).unwrap();
    input.button(keys::BTN_LEFT, false).unwrap();
    let done = overlay.finish();
    let png = done.png();

    assert_eq!((png.width, png.height), (100, 50));
    assert_cut_from(&png, &Pattern::Ramp, (320, 240), (10, 20), 1, "1× region");
    assert!(
        !done.stderr.contains("mixed scale"),
        "no badge: {}",
        done.stderr
    );
}

#[test]
fn clamp_keeps_the_region_on_the_output_where_the_drag_started() {
    let (session, _painter) = one_by_two();
    let overlay = start(
        &session,
        &[
            "shot",
            "region",
            "-o",
            "-",
            "--set",
            "capture.mixed_scale=clamp",
        ],
    );
    drag_across(&session);
    let done = overlay.finish();
    let png = done.png();

    // Logical (300, 100)-(320, 160): the right edge of the left output, at 1×.
    assert_eq!((png.width, png.height), (20, 60));
    assert_cut_from(&png, &Pattern::Ramp, (320, 240), (300, 100), 1, "clamped");
    assert!(!done.stderr.contains("mixed scale"), "{}", done.stderr);
}

fn saved(stdout: &[u8]) -> Vec<PathBuf> {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(PathBuf::from)
        .collect()
}

fn read_png(path: &Path) -> Png {
    Png::decode(&std::fs::read(path).unwrap())
}

#[test]
fn split_saves_one_file_per_output_each_at_its_own_scale() {
    let (session, _painter) = one_by_two();
    let dir = session.runtime_dir().join("shots");
    let dir_setting = format!("screenshot.dir={}", dir.display());
    let overlay = start(
        &session,
        &[
            "shot",
            "region",
            "--set",
            "capture.mixed_scale=split",
            "--set",
            &dir_setting,
            "--set",
            "screenshot.name=cut",
        ],
    );
    drag_across(&session);
    let done = overlay.finish();
    assert!(done.status.success(), "{}", done.stderr);

    let files = saved(&done.stdout);
    let names: Vec<_> = files
        .iter()
        .map(|p| p.file_name().unwrap().to_str().unwrap())
        .collect();
    assert_eq!(names, ["cut_HEADLESS-1.png", "cut_HEADLESS-2.png"]);
    assert!(files.iter().all(|f| f.starts_with(&dir)));

    let left = read_png(&files[0]);
    assert_eq!((left.width, left.height), (20, 60));
    assert_cut_from(&left, &Pattern::Ramp, (320, 240), (300, 100), 1, "left");
    let right = read_png(&files[1]);
    assert_eq!((right.width, right.height), (60, 120));
    assert_cut_from(
        &right,
        &Pattern::Checker { cell: 1 },
        (640, 480),
        (0, 200),
        1,
        "right",
    );
    assert!(!done.stderr.contains("mixed scale"), "{}", done.stderr);
}

#[test]
fn split_writes_all_outputs_next_to_the_o_path() {
    let (session, _painter) = one_by_two();
    let path = session.runtime_dir().join("all.png");
    let out = session
        .command(env!("CARGO_BIN_EXE_hoarder"))
        .args(["shot", "all", "--set", "capture.mixed_scale=split", "-o"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let left = read_png(&session.runtime_dir().join("all_HEADLESS-1.png"));
    assert_cut_from(&left, &Pattern::Ramp, (320, 240), (0, 0), 1, "left");
    let right = read_png(&session.runtime_dir().join("all_HEADLESS-2.png"));
    assert_eq!((right.width, right.height), (640, 480));
    assert_cut_from(
        &right,
        &Pattern::Checker { cell: 1 },
        (640, 480),
        (0, 0),
        1,
        "right",
    );
    assert!(!path.exists(), "no combined file");
}

#[test]
fn shot_all_upscales_by_default() {
    let (session, _painter) = one_by_two();
    let out = session
        .command(env!("CARGO_BIN_EXE_hoarder"))
        .args(["shot", "all", "-o", "-"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    let png = Png::decode(&out.stdout);
    assert_eq!((png.width, png.height), (1280, 480));
    assert_cut_from(
        &png.crop(0, 0, 640, 480),
        &Pattern::Ramp,
        (320, 240),
        (0, 0),
        2,
        "1× output",
    );
    assert!(stderr.contains("mixed scale"), "{stderr}");
}

#[test]
fn depth_8_turns_a_10bit_output_into_an_8bit_png() {
    let session = Session::start(&[OutputSpec::new(640, 480).depth(BitDepth::Ten)]).unwrap();
    let app = session.client().unwrap();
    app.paint_as(
        session.output(LEFT),
        &Pattern::Ramp,
        PixelFormat::Xrgb2101010,
    )
    .unwrap();

    let run = |depth: &str| {
        let out = session
            .command(env!("CARGO_BIN_EXE_hoarder"))
            .args(["shot", "all", "-o", "-", "--set"])
            .arg(format!("capture.depth={depth}"))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        Png::decode(&out.stdout)
    };
    assert_eq!(run("auto").depth, 16, "auto keeps 10-bit data at 16 bits");
    let png = run("8");
    assert_eq!((png.width, png.height), (640, 480));
    // Each 10-bit value rounded to the nearest 8-bit one.
    assert_cut_from(&png, &Pattern::Ramp, (640, 480), (0, 0), 1, "8-bit");
}
