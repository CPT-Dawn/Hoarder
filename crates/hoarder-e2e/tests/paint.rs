//! The helper client paints known patterns that a user (here: `grim`) sees
//! exactly as painted, whatever the output's depth, scale or rotation.

use hoarder_e2e::{BitDepth, OutputSpec, Pattern, PixelFormat, Rgb8, Rgb10, Session, Transform};

const RED: Rgb8 = Rgb8(255, 0, 0);
const GREEN: Rgb8 = Rgb8(0, 255, 0);
const BLUE: Rgb8 = Rgb8(0, 0, 255);
const WHITE: Rgb8 = Rgb8(255, 255, 255);
const BLACK: Rgb8 = Rgb8(0, 0, 0);

/// Asserts the quadrants pattern (red, green / blue, white) fills a `w`×`h`
/// image, with each boundary exactly on the middle pixel row and column.
fn assert_quadrants(session: &Session, output: &str, (w, h): (u32, u32)) {
    let img = session
        .screenshot(output)
        .expect("grim should capture the output");
    assert_eq!(img.size(), (w, h), "screenshot size of {output}");
    let (mx, my) = (w / 2, h / 2);
    for (x, y, want) in [
        (0, 0, RED),
        (mx - 1, my - 1, RED),
        (mx, 0, GREEN),
        (w - 1, my - 1, GREEN),
        (0, my, BLUE),
        (mx - 1, h - 1, BLUE),
        (mx, my, WHITE),
        (w - 1, h - 1, WHITE),
    ] {
        assert_eq!(img.pixel(x, y), want, "{output} pixel ({x}, {y})");
    }
}

#[test]
fn paints_quadrants_on_an_8bit_output() {
    let session = Session::start(&[OutputSpec::new(640, 480)]).unwrap();
    let client = session.client().unwrap();

    client
        .paint(session.output("HEADLESS-1"), &Pattern::Quadrants)
        .unwrap();

    assert_quadrants(&session, "HEADLESS-1", (640, 480));
}

#[test]
fn paints_physical_pixels_one_to_one_on_a_scaled_output() {
    let session = Session::start(&[OutputSpec::new(640, 480).scale(2.0)]).unwrap();
    let client = session.client().unwrap();

    client
        .paint(session.output("HEADLESS-1"), &Pattern::Quadrants)
        .unwrap();

    assert_quadrants(&session, "HEADLESS-1", (640, 480));

    // A one-pixel checkerboard survives only if nothing was resampled.
    client
        .paint(session.output("HEADLESS-1"), &Pattern::Checker { cell: 1 })
        .unwrap();
    let img = session.screenshot("HEADLESS-1").unwrap();
    for (x, y, want) in [
        (0, 0, WHITE),
        (1, 0, BLACK),
        (0, 1, BLACK),
        (1, 1, WHITE),
        (639, 479, WHITE),
        (638, 479, BLACK),
        (321, 240, BLACK),
    ] {
        assert_eq!(img.pixel(x, y), want, "pixel ({x}, {y})");
    }
}

#[test]
fn paints_on_a_10bit_output() {
    let session = Session::start(&[OutputSpec::new(640, 480).depth(BitDepth::Ten)]).unwrap();
    let client = session.client().unwrap();
    let output = session.output("HEADLESS-1");

    client
        .paint_as(output, &Pattern::Quadrants, PixelFormat::Xrgb2101010)
        .unwrap();
    assert_quadrants(&session, "HEADLESS-1", (640, 480));

    // Values with no 8-bit equivalent come back bit-exact from the output buffer.
    let colour = Rgb10(0x201, 0x0ff, 0x3fe);
    client
        .paint_as(output, &Pattern::Solid(colour), PixelFormat::Xrgb2101010)
        .unwrap();
    let frame = client.capture(output).unwrap();
    assert_eq!(
        frame.bits_per_channel(),
        10,
        "frame format {:?}",
        frame.format()
    );
    assert_eq!(frame.size(), (640, 480));
    assert_eq!(frame.pixel(0, 0), colour);
    assert_eq!(frame.pixel(639, 479), colour);
}

#[test]
fn paints_upright_on_rotated_outputs() {
    let session = Session::start(&[
        OutputSpec::new(640, 480).transform(Transform::Rotate90),
        OutputSpec::new(640, 480).transform(Transform::Rotate270),
        OutputSpec::new(640, 480).transform(Transform::Flipped180),
    ])
    .unwrap();
    let client = session.client().unwrap();

    for output in session.outputs() {
        client.paint(output, &Pattern::Quadrants).unwrap();
    }

    assert_quadrants(&session, "HEADLESS-1", (480, 640));
    assert_quadrants(&session, "HEADLESS-2", (480, 640));
    assert_quadrants(&session, "HEADLESS-3", (640, 480));
}
