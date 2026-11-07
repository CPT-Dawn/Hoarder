//! `hoarder shot region`: the Overlay, driven by the virtual pointer and
//! keyboard, yields a Region cut byte for byte from the Frozen Frames.

mod common;

use common::{Overlay, as_8bit};
use hoarder_e2e::{OutputSpec, Pattern, Rgb8, Session, keys};

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
