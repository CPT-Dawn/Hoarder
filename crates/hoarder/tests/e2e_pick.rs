//! `hoarder pick` (PLAN.md §4.3): the same freeze and Overlay, the Loupe
//! always shown, and a click copies the colour under the pointer.

mod bus;
mod common;

use std::process::Stdio;
use std::time::{Duration, Instant};

use bus::Bus;
use common::Overlay;
use hoarder_e2e::{BitDepth, OutputSpec, Pattern, PixelFormat, Rgb8, Rgb10, Session, keys};

const BTN_LEFT: u32 = 0x110;

fn wl_paste(session: &Session) -> std::process::Output {
    session
        .command("wl-paste")
        .args(["--no-newline"])
        .stdin(Stdio::null())
        .output()
        .expect("wl-paste should run")
}

/// Run `hoarder pick ARGS`, click at `(x, y)` on HEADLESS-1, and return what
/// it printed (and so copied).
fn pick_at(session: &Session, args: &[&str], (x, y): (f64, f64)) -> String {
    let mut argv = vec!["pick"];
    argv.extend_from_slice(args);
    let overlay = Overlay::start(session, &argv, &["HEADLESS-1"]);
    let input = session.input();
    input.move_to(session.output("HEADLESS-1"), x, y).unwrap();
    input.click(BTN_LEFT).unwrap();
    let done = overlay.finish();
    assert!(
        done.status.success(),
        "hoarder pick failed ({}): {}",
        done.status,
        done.stderr
    );
    let printed = String::from_utf8(done.stdout).unwrap();
    let value = printed.strip_suffix('\n').expect("one line").to_owned();
    let pasted = wl_paste(session);
    assert_eq!(
        String::from_utf8_lossy(&pasted.stdout),
        value,
        "what is printed is on the clipboard"
    );
    value
}

#[test]
fn a_click_copies_the_pixel_under_the_pointer_as_hex() {
    let session = Session::start(&[OutputSpec::new(640, 480)]).unwrap();
    let app = session.client().unwrap();
    app.paint(session.output("HEADLESS-1"), &Pattern::Ramp)
        .unwrap();

    let value = pick_at(&session, &[], (300.0, 200.0));

    // Pixel 200 * 640 + 300 = 128300 of the ramp: (300, 55, 723) in 10 bits.
    let want = common::as_8bit(Pattern::Ramp.colour_at(300, 200, 640, 480));
    assert_eq!(want, Rgb8(75, 14, 180), "the ramp, rounded to 8 bits");
    assert_eq!(value, "#4b0eb4");
}

/// A session showing one solid orange 640×480 output.
fn orange_session() -> (Session, hoarder_e2e::Client) {
    let session = Session::start(&[OutputSpec::new(640, 480)]).unwrap();
    let app = session.client().unwrap();
    app.paint(
        session.output("HEADLESS-1"),
        &Pattern::solid(Rgb8(255, 128, 0)),
    )
    .unwrap();
    (session, app)
}

#[test]
fn format_or_pick_format_chooses_rgb_or_hsl() {
    let (session, _app) = orange_session();
    let at = (100.0, 100.0);
    assert_eq!(
        pick_at(&session, &["--format", "rgb"], at),
        "rgb(255, 128, 0)"
    );
    assert_eq!(
        pick_at(&session, &["--set", "pick.format=hsl"], at),
        "hsl(30, 100%, 50%)"
    );
    assert_eq!(
        pick_at(
            &session,
            &["--set", "pick.format=hsl", "--format", "hex"],
            at
        ),
        "#ff8000",
        "--format beats the config"
    );
}

#[test]
fn a_10bit_output_reports_the_exact_10bit_value() {
    let spec = OutputSpec::new(640, 480).depth(BitDepth::Ten);
    let session = Session::start(&[spec]).unwrap();
    let app = session.client().unwrap();
    app.paint_as(
        session.output("HEADLESS-1"),
        &Pattern::Ramp,
        PixelFormat::Xrgb2101010,
    )
    .unwrap();
    let at = (300.0, 200.0);

    // Pixel 128300 of the ramp is (300, 55, 723): no 8-bit value says that.
    assert_eq!(
        Pattern::Ramp.colour_at(300, 200, 640, 480),
        Rgb10(300, 55, 723)
    );
    assert_eq!(
        pick_at(&session, &["--format", "rgb"], at),
        "rgb10(300, 55, 723)"
    );
    assert_eq!(pick_at(&session, &[], at), "#12c0372d3");
}

#[test]
fn a_scaled_output_reports_the_physical_pixel_under_the_pointer() {
    // 800×600 physical at 1.25: logical (300, 200) is physical (375, 250).
    let spec = OutputSpec::new(800, 600).scale(1.25);
    let session = Session::start(&[spec]).unwrap();
    let app = session.client().unwrap();
    app.paint(session.output("HEADLESS-1"), &Pattern::Ramp)
        .unwrap();

    let value = pick_at(&session, &["--format", "rgb"], (300.0, 200.0));

    // Pixel 250 * 800 + 375 = 200375 is (695, 772, 328) in 10 bits.
    assert_eq!(
        Pattern::Ramp.colour_at(375, 250, 800, 600),
        Rgb10(695, 772, 328)
    );
    assert_eq!(value, "rgb(173, 192, 82)");
}

#[test]
fn esc_cancels_with_exit_code_1_and_copies_nothing() {
    let (session, _app) = orange_session();
    let overlay = Overlay::start(&session, &["pick"], &["HEADLESS-1"]);
    session.input().tap(keys::KEY_ESC).unwrap();
    let done = overlay.finish();
    assert_eq!(done.status.code(), Some(1), "stderr: {}", done.stderr);
    assert!(done.stdout.is_empty(), "nothing printed");
    assert!(!wl_paste(&session).status.success(), "nothing copied");
}

#[test]
fn the_pick_is_toasted_with_a_swatch_of_its_colour() {
    let (session, _app) = orange_session();
    let bus = Bus::start(&session);

    let value = pick_at(&session, &[], (100.0, 100.0));

    let n = bus.notified(1);
    assert_eq!(n.app_name, "Hoarder");
    assert_eq!(n.summary, value, "the toast names the colour");
    let image = n.image.expect("a swatch in the image-data hint");
    assert_eq!((image.channels, image.has_alpha), (3, false));
    assert_eq!(image.data.len(), (image.rowstride * image.height) as usize);
    assert!(
        image.data.chunks(3).all(|p| p == [255, 128, 0]),
        "the swatch is the picked colour"
    );
}

#[test]
fn the_loupe_shows_at_once_while_the_pointer_moves() {
    let session = Session::start(&[OutputSpec::new(640, 480)]).unwrap();
    let app = session.client().unwrap();
    let out = session.output("HEADLESS-1");
    app.paint(out, &Pattern::Ramp).unwrap();

    let overlay = Overlay::start(&session, &["pick"], &["HEADLESS-1"]);
    let input = session.input();
    // Never still for the 300 ms `shot` waits: pick shows the Loupe anyway.
    // Its middle cell (8 px, behind a 1 px border, 24 px right of and below
    // the pointer) holds the pointer's pixel.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut step = 0u32;
    let shown = loop {
        let x = 300 + step % 2;
        input.move_to(out, x as f64, 200.0).unwrap();
        let seen = session.screenshot("HEADLESS-1").unwrap();
        let want = common::as_8bit(Pattern::Ramp.colour_at(x, 200, 640, 480));
        let centre = (x + 24 + 1 + 7 * 8 + 4, 200 + 24 + 1 + 7 * 8 + 4);
        if seen.pixel(centre.0, centre.1) == want {
            break true;
        }
        if Instant::now() > deadline {
            break false;
        }
        step += 1;
    };
    assert!(shown, "the Loupe follows the moving pointer");

    input.tap(keys::KEY_ESC).unwrap();
    assert_eq!(overlay.finish().status.code(), Some(1));
}
