//! The Picker's filters, keys, preview pane and masking (PLAN.md §6):
//! filter prefixes, Shift+Enter, Ctrl+P, Del, Ctrl+1–9 and Alt, driven
//! through the virtual keyboard and judged by `wl-paste`, `hoarder history`
//! and what grim sees.

mod common;

use std::os::unix::net::UnixStream;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use common::Overlay;
use hoarder_e2e::{Image, OutputSpec, Pattern, Rgb8, Selection, Session, keys};
use serde_json::Value;

const HOARDER: &str = env!("CARGO_BIN_EXE_hoarder");

/// A running `hoarder daemon`, killed when dropped.
struct Daemon(Child);

impl Daemon {
    fn start(session: &Session) -> Daemon {
        let log = std::fs::File::create(session.runtime_dir().join("daemon.log")).unwrap();
        let child = session
            .command(HOARDER)
            .arg("daemon")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("start hoarder daemon");
        let mut daemon = Daemon(child);
        let socket = session.runtime_dir().join("hoarder/hoarder.sock");
        wait_until("the daemon socket", || {
            if let Ok(Some(status)) = daemon.0.try_wait() {
                panic!("daemon exited early ({status})");
            }
            UnixStream::connect(&socket).is_ok()
        });
        daemon
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// `hoarder history list --json`, most recently used first.
fn history(session: &Session) -> Vec<Value> {
    let out = session
        .command(HOARDER)
        .args(["history", "list", "--json"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "history list failed");
    serde_json::from_slice::<Value>(&out.stdout)
        .unwrap()
        .as_array()
        .expect("a JSON array")
        .clone()
}

fn top_id(session: &Session) -> i64 {
    history(session)[0]["id"].as_i64().unwrap()
}

/// `wl-paste --no-newline`, with the type if given.
fn paste(session: &Session, mime: Option<&str>) -> String {
    let mut cmd = session.command("wl-paste");
    cmd.arg("--no-newline");
    if let Some(mime) = mime {
        cmd.args(["--type", mime]);
    }
    let out = cmd.stdin(Stdio::null()).output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// `wl-paste --list-types`.
fn pasteable_types(session: &Session) -> Vec<String> {
    let out = session
        .command("wl-paste")
        .arg("--list-types")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// A session with something painted, so the Picker showing up changes
/// what grim sees.
fn painted_on(spec: OutputSpec) -> (Session, hoarder_e2e::Client) {
    let session = Session::start(&[spec]).unwrap();
    let app = session.client().unwrap();
    app.paint(session.output("HEADLESS-1"), &Pattern::Quadrants)
        .unwrap();
    (session, app)
}

fn painted() -> (Session, hoarder_e2e::Client) {
    painted_on(OutputSpec::new(1280, 720))
}

/// One Clip to put in History before the Daemon starts.
struct Seed<'a> {
    reprs: &'a [(&'a str, &'a [u8])],
    app: Option<&'a str>,
    pinned: bool,
}

fn text(t: &str) -> Vec<(&'static str, &[u8])> {
    vec![("text/plain;charset=utf-8", t.as_bytes())]
}

/// Record `seeds` oldest first, a second apart, and return their ids. The
/// Daemon must not be running yet.
fn seed(session: &Session, seeds: &[Seed]) -> Vec<i64> {
    let dir = session.runtime_dir().join("home/.local/share/hoarder");
    let mut store = hoarder_store::Store::open(&dir).unwrap();
    // Recent, or retention would evict them as soon as the Daemon starts.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        - 60_000;
    seeds
        .iter()
        .enumerate()
        .map(|(n, s)| {
            let id = store
                .record(s.reprs, s.app, now + n as i64 * 1000)
                .unwrap()
                .id;
            if s.pinned {
                store.pin(id, true).unwrap();
            }
            id
        })
        .collect()
}

/// A solid `w`×`h` PNG.
fn png(w: u32, h: u32, colour: [u8; 3]) -> Vec<u8> {
    let image =
        image::RgbaImage::from_pixel(w, h, image::Rgba([colour[0], colour[1], colour[2], 255]));
    let mut out = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

/// Type `text` (lowercase letters, digits, spaces, `:` and `.`) on the
/// virtual keyboard.
fn type_text(session: &Session, text: &str) {
    const LETTERS: [u32; 26] = [
        keys::KEY_A,
        keys::KEY_B,
        keys::KEY_C,
        keys::KEY_D,
        keys::KEY_E,
        keys::KEY_F,
        keys::KEY_G,
        keys::KEY_H,
        keys::KEY_I,
        keys::KEY_J,
        keys::KEY_K,
        keys::KEY_L,
        keys::KEY_M,
        keys::KEY_N,
        keys::KEY_O,
        keys::KEY_P,
        keys::KEY_Q,
        keys::KEY_R,
        keys::KEY_S,
        keys::KEY_T,
        keys::KEY_U,
        keys::KEY_V,
        keys::KEY_W,
        keys::KEY_X,
        keys::KEY_Y,
        keys::KEY_Z,
    ];
    let input = session.input();
    for c in text.chars() {
        match c {
            ':' => input
                .chord(&[keys::KEY_LEFTSHIFT, keys::KEY_SEMICOLON])
                .unwrap(),
            '.' => input.tap(keys::KEY_DOT).unwrap(),
            ' ' => input.tap(keys::KEY_SPACE).unwrap(),
            '0' => input.tap(keys::KEY_0).unwrap(),
            '1'..='9' => input.tap(keys::KEY_1 + (c as u32 - '1' as u32)).unwrap(),
            'a'..='z' => input.tap(LETTERS[(c as u8 - b'a') as usize]).unwrap(),
            _ => panic!("cannot type {c:?}"),
        }
    }
}

fn picker(session: &Session) -> Overlay {
    Overlay::start(session, &["picker"], &["HEADLESS-1"])
}

/// Open the Picker, type `keys_typed`, then press Enter; it must succeed.
fn pick_with(session: &Session, keys_typed: &str) {
    let picker = picker(session);
    type_text(session, keys_typed);
    session.input().tap(keys::KEY_ENTER).unwrap();
    let done = picker.finish();
    assert!(done.status.success(), "{keys_typed:?}: {}", done.stderr);
}

#[test]
fn filter_prefixes_narrow_by_kind_pin_and_app() {
    let (session, _paint) = painted();
    let image = png(40, 30, [255, 0, 255]);
    let ids = seed(
        &session,
        &[
            Seed {
                reprs: &text("plain words"),
                app: Some("kitty"),
                pinned: false,
            },
            Seed {
                reprs: &[("image/png", &image)],
                app: Some("org.gimp.GIMP"),
                pinned: false,
            },
            Seed {
                reprs: &[("text/uri-list", b"file:///tmp/report.pdf\r\n")],
                app: Some("nautilus"),
                pinned: false,
            },
            Seed {
                reprs: &text("pinned note"),
                app: Some("kitty"),
                pinned: true,
            },
            Seed {
                reprs: &text("firefox link"),
                app: Some("firefox"),
                pinned: false,
            },
        ],
    );
    let [plain, img, file, pinned, link] = ids[..] else {
        unreachable!()
    };
    let _daemon = Daemon::start(&session);

    // Each Restore moves its Clip to the top, so every step below would
    // pick the previous step's Clip if its filter did nothing.
    pick_with(&session, "img:");
    assert_eq!(top_id(&session), img);

    pick_with(&session, "txt:");
    assert_eq!(top_id(&session), link);
    assert_eq!(paste(&session, None), "firefox link");

    pick_with(&session, "file:");
    assert_eq!(top_id(&session), file);

    // The newest Clip copied in kitty, matched case-insensitively.
    pick_with(&session, "app:kitty");
    assert_eq!(top_id(&session), pinned);

    pick_with(&session, "img:");
    pick_with(&session, "pin:");
    assert_eq!(top_id(&session), pinned);

    // Filters combine with each other and with a search.
    pick_with(&session, "txt: app:kit plain");
    assert_eq!(top_id(&session), plain);
    assert_eq!(paste(&session, None), "plain words");

    pick_with(&session, "app:org.gimp");
    assert_eq!(top_id(&session), img);
}

#[test]
fn a_filter_that_matches_nothing_restores_nothing() {
    let (session, _paint) = painted();
    seed(
        &session,
        &[Seed {
            reprs: &text("only text"),
            app: None,
            pinned: false,
        }],
    );
    let _daemon = Daemon::start(&session);

    let picker = picker(&session);
    type_text(&session, "img:");
    session.input().tap(keys::KEY_ENTER).unwrap();
    session.input().tap(keys::KEY_ESC).unwrap();
    let done = picker.finish();

    assert_eq!(done.status.code(), Some(1), "{}", done.stderr);
}

// Silence unused-helper warnings until the tests below use them.
#[allow(dead_code)]
fn _unused(session: &Session) -> (Vec<String>, Image, Rgb8, Selection) {
    (
        pasteable_types(session),
        session.screenshot("HEADLESS-1").unwrap(),
        Rgb8(0, 0, 0),
        Selection::Clipboard,
    )
}
