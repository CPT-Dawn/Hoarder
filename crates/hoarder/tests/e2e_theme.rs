//! Theming (PLAN.md §8): the Overlay takes its colours from the active theme,
//! following Zenith's when it is installed, with `[theme]` overrides on top.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::Overlay;
use hoarder_e2e::{Client, Image, OutputSpec, Pattern, Rgb8, Session, keys};

fn config_home(session: &Session) -> PathBuf {
    session.runtime_dir().join("home/.config")
}

fn write(session: &Session, file: &str, text: &str) {
    let path = config_home(session).join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// The colour of the selection border on a fresh Overlay.
fn border_colour(session: &Session) -> Rgb8 {
    let seen = overlay_with_selection(session);
    seen.pixel(39, 80)
}

/// Drag out a selection on a fresh Overlay, screenshot it once the border
/// shows, then cancel.
fn overlay_with_selection(session: &Session) -> Image {
    let overlay = Overlay::start(session, &["shot", "region", "-o", "-"], &["HEADLESS-1"]);
    let out = session.output("HEADLESS-1");
    let input = session.input();
    input.move_to(out, 40.0, 40.0).unwrap();
    input.button(keys::BTN_LEFT, true).unwrap();
    input.move_to(out, 200.0, 120.0).unwrap();
    // The selection is (40, 40)-(200, 120); its border runs just outside it.
    // Before the drag shows, (39, 80) is dimmed red: (140, 0, 0).
    let deadline = Instant::now() + Duration::from_secs(5);
    let seen = loop {
        let seen = session.screenshot("HEADLESS-1").unwrap();
        let (border, top) = (seen.pixel(39, 80), seen.pixel(100, 39));
        if border != Rgb8(140, 0, 0) && border == top {
            break seen;
        }
        assert!(Instant::now() < deadline, "no border showed: {border:?}");
        std::thread::sleep(Duration::from_millis(20));
    };
    input.tap(keys::KEY_ESC).unwrap();
    let done = overlay.finish();
    assert_eq!(done.status.code(), Some(1), "cancelled: {}", done.stderr);
    seen
}

/// A 600×480 session painted with quadrants (red top-left). The client
/// keeps the painting up for as long as it lives.
fn session() -> (Session, Client) {
    let session = Session::start(&[OutputSpec::new(600, 480)]).unwrap();
    let app = session.client().unwrap();
    app.paint(session.output("HEADLESS-1"), &Pattern::Quadrants)
        .unwrap();
    (session, app)
}

#[test]
fn the_overlay_wears_the_active_zenith_theme() {
    let (session, _app) = session();
    write(
        &session,
        "zenith-bar/config.toml",
        "[bar]\nheight = 30\n\n[theme]\nname = \"tokyo-night\"\n",
    );

    assert_eq!(border_colour(&session), Rgb8(0x7a, 0xa2, 0xf7));
}

#[test]
fn changing_the_zenith_theme_restyles_the_next_overlay() {
    let (session, _app) = session();
    write(
        &session,
        "zenith-bar/config.toml",
        "[theme]\nname = \"tokyo-night\"\n",
    );
    assert_eq!(border_colour(&session), Rgb8(0x7a, 0xa2, 0xf7));

    write(
        &session,
        "zenith-bar/config.toml",
        "[theme]\nname = \"catppuccin-mocha\"\n",
    );

    assert_eq!(border_colour(&session), Rgb8(0xcb, 0xa6, 0xf7));
}

#[test]
fn zenith_style_css_beats_zenith_overrides_which_beat_its_theme() {
    let (session, _app) = session();
    write(
        &session,
        "zenith-bar/config.toml",
        "[theme]\nname = \"gruvbox-dark\"\naccent = \"#112233\"\n",
    );
    assert_eq!(border_colour(&session), Rgb8(0x11, 0x22, 0x33));

    write(
        &session,
        "zenith-bar/style.css",
        "/* mine */\n.zb-bar { --zb-accent: #000000; }\n:root {\n  --zb-pink: rgb(255 0 170);\n  --zb-accent: var(--zb-pink); /* follow the pink */\n}\n",
    );

    assert_eq!(border_colour(&session), Rgb8(0xff, 0x00, 0xaa));
}

#[test]
fn hoarder_overrides_beat_every_theme_value() {
    let (session, _app) = session();
    write(
        &session,
        "zenith-bar/config.toml",
        "[theme]\nname = \"tokyo-night\"\n",
    );
    write(
        &session,
        "zenith-bar/style.css",
        ":root { --zb-accent: #ff0000; }\n",
    );
    write(
        &session,
        "hoarder/config.toml",
        "[theme]\naccent = \"#00ff7f\"\n",
    );
    assert_eq!(border_colour(&session), Rgb8(0x00, 0xff, 0x7f));

    write(
        &session,
        "hoarder/config.toml",
        "[theme]\nname = \"gruvbox-dark\"\naccent = \"rgb(1 2 3)\"\n",
    );
    assert_eq!(border_colour(&session), Rgb8(1, 2, 3));
}

#[test]
fn a_built_in_theme_ignores_zenith() {
    let (session, _app) = session();
    write(
        &session,
        "zenith-bar/config.toml",
        "[theme]\nname = \"tokyo-night\"\n",
    );
    write(
        &session,
        "hoarder/config.toml",
        "[theme]\nname = \"gruvbox-dark\"\n",
    );

    assert_eq!(border_colour(&session), Rgb8(0x8e, 0xc0, 0x7c));
}

#[test]
fn the_size_pill_wears_the_surface_and_radius_tokens() {
    let (session, _app) = session();
    write(
        &session,
        "zenith-bar/style.css",
        ":root { --zb-surface-color: rgb(255 0 0); --zb-surface-opacity: 1; }\n",
    );
    write(&session, "hoarder/config.toml", "[theme]\nradius = 0\n");

    // The pill hangs 6 px below the selection's bottom border, from its left
    // edge: (40, 127) onwards, 23 px tall. Square corners, opaque red.
    let seen = overlay_with_selection(&session);
    for (x, y) in [(41, 138), (60, 128), (60, 148)] {
        assert_eq!(seen.pixel(x, y), Rgb8(255, 0, 0), "pill pixel ({x}, {y})");
    }
    // A rounded corner would leave the dimmed red quadrant, (140, 0, 0),
    // showing here; a square one is the pill's red (bar some anti-aliasing).
    let Rgb8(r, g, b) = seen.pixel(40, 127);
    assert!(r > 200 && g == 0 && b < 30, "square corner: {r} {g} {b}");
}

/// Midnight Bloom, Zenith's own default.
const MIDNIGHT_BLOOM: Rgb8 = Rgb8(0x8d, 0xe3, 0xdc);

#[test]
fn without_zenith_the_overlay_falls_back_to_midnight_bloom() {
    let (session, _app) = session();

    assert_eq!(border_colour(&session), MIDNIGHT_BLOOM);
}

#[test]
fn a_broken_zenith_config_falls_back_cleanly() {
    let (session, _app) = session();
    write(
        &session,
        "zenith-bar/config.toml",
        "[theme\nname = = tokyo\n",
    );
    write(
        &session,
        "zenith-bar/style.css",
        ":root { --zb-accent: color-mix(in srgb, red 50%, blue); \n",
    );

    assert_eq!(border_colour(&session), MIDNIGHT_BLOOM);
}
