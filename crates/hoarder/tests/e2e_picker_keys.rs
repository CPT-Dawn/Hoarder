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

fn open_picker(session: &Session) -> Overlay {
    Overlay::start(session, &["picker"], &["HEADLESS-1"])
}

/// Open the Picker, type `keys_typed`, then press Enter; it must succeed.
fn pick_with(session: &Session, keys_typed: &str) {
    let picker = open_picker(session);
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

    let picker = open_picker(&session);
    type_text(&session, "img:");
    session.input().tap(keys::KEY_ENTER).unwrap();
    session.input().tap(keys::KEY_ESC).unwrap();
    let done = picker.finish();

    assert_eq!(done.status.code(), Some(1), "{}", done.stderr);
}

#[test]
fn shift_enter_restores_only_the_plain_text() {
    let (session, _paint) = painted();
    seed(
        &session,
        &[Seed {
            reprs: &[
                ("text/html", b"<b>bold</b> move"),
                ("text/plain;charset=utf-8", b"bold move"),
            ],
            app: None,
            pinned: false,
        }],
    );
    let _daemon = Daemon::start(&session);

    let picker = open_picker(&session);
    session
        .input()
        .chord(&[keys::KEY_LEFTSHIFT, keys::KEY_ENTER])
        .unwrap();
    let done = picker.finish();

    assert!(done.status.success(), "{}", done.stderr);
    assert_eq!(pasteable_types(&session), ["text/plain;charset=utf-8"]);
    assert_eq!(paste(&session, None), "bold move");
}

#[test]
fn shift_enter_on_a_clip_without_text_restores_nothing() {
    let (session, _paint) = painted();
    let image = png(4, 4, [0, 0, 255]);
    seed(
        &session,
        &[Seed {
            reprs: &[("image/png", &image)],
            app: None,
            pinned: false,
        }],
    );
    let _daemon = Daemon::start(&session);

    let picker = open_picker(&session);
    session
        .input()
        .chord(&[keys::KEY_LEFTSHIFT, keys::KEY_ENTER])
        .unwrap();
    let done = picker.finish();

    assert!(!done.status.success());
    assert!(done.stderr.contains("no plain text"), "{}", done.stderr);
    assert!(pasteable_types(&session).is_empty());
}

#[test]
fn ctrl_p_pins_the_selected_clip_and_unpins_a_pinned_one() {
    let (session, _paint) = painted();
    let ids = seed(
        &session,
        &[
            Seed {
                reprs: &text("older"),
                app: None,
                pinned: false,
            },
            Seed {
                reprs: &text("newer"),
                app: None,
                pinned: false,
            },
        ],
    );
    let _daemon = Daemon::start(&session);
    let pinned = |id: i64| {
        history(&session).iter().find(|c| c["id"] == id).unwrap()["pinned"]
            .as_bool()
            .unwrap()
    };

    // The pin shows at once: `pin:` then finds it.
    let picker = open_picker(&session);
    session.input().tap(keys::KEY_DOWN).unwrap();
    session
        .input()
        .chord(&[keys::KEY_LEFTCTRL, keys::KEY_P])
        .unwrap();
    type_text(&session, "pin:");
    session.input().tap(keys::KEY_ENTER).unwrap();
    let done = picker.finish();
    assert!(done.status.success(), "{}", done.stderr);
    assert!(pinned(ids[0]));
    assert!(!pinned(ids[1]));
    assert_eq!(paste(&session, None), "older");

    let picker = open_picker(&session);
    session
        .input()
        .chord(&[keys::KEY_LEFTCTRL, keys::KEY_P])
        .unwrap();
    session.input().tap(keys::KEY_ESC).unwrap();
    picker.finish();
    assert!(!pinned(ids[0]));
}

#[test]
fn del_deletes_the_selected_clip_and_selects_the_next() {
    let (session, _paint) = painted();
    seed(
        &session,
        &[
            Seed {
                reprs: &text("first"),
                app: None,
                pinned: false,
            },
            Seed {
                reprs: &text("second"),
                app: None,
                pinned: false,
            },
            Seed {
                reprs: &text("third"),
                app: None,
                pinned: false,
            },
        ],
    );
    let _daemon = Daemon::start(&session);

    let picker = open_picker(&session);
    session.input().tap(keys::KEY_DOWN).unwrap();
    session.input().tap(keys::KEY_DELETE).unwrap();
    session.input().tap(keys::KEY_ENTER).unwrap();
    let done = picker.finish();

    assert!(done.status.success(), "{}", done.stderr);
    let previews: Vec<_> = history(&session)
        .iter()
        .map(|c| c["preview"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(previews, ["first", "third"]);
    assert_eq!(paste(&session, None), "first");
}

#[test]
fn ctrl_digits_restore_that_row_at_once() {
    let (session, _paint) = painted();
    let texts: Vec<String> = (0..10).map(|n| format!("clip {n}")).collect();
    let reprs: Vec<_> = texts.iter().map(|t| text(t)).collect();
    let seeds: Vec<_> = reprs
        .iter()
        .map(|r| Seed {
            reprs: r,
            app: None,
            pinned: false,
        })
        .collect();
    seed(&session, &seeds);
    let _daemon = Daemon::start(&session);

    let quick = |digit: u32| {
        let picker = open_picker(&session);
        session.input().chord(&[keys::KEY_LEFTCTRL, digit]).unwrap();
        let done = picker.finish();
        assert!(done.status.success(), "{}", done.stderr);
        paste(&session, None)
    };

    // Rows, newest first: clip 9, clip 8, clip 7, …
    assert_eq!(quick(keys::KEY_3), "clip 7");
    // Now clip 7, clip 9, clip 8, clip 6, …, clip 1, then clip 1 on top.
    assert_eq!(quick(keys::KEY_9), "clip 1");
    assert_eq!(quick(keys::KEY_1), "clip 1");
    assert_eq!(quick(keys::KEY_5), "clip 6");
}

#[test]
fn ctrl_digits_count_the_rows_a_query_left() {
    let (session, _paint) = painted();
    seed(
        &session,
        &[
            Seed {
                reprs: &text("apple one"),
                app: None,
                pinned: false,
            },
            Seed {
                reprs: &text("berry"),
                app: None,
                pinned: false,
            },
            Seed {
                reprs: &text("apple two"),
                app: None,
                pinned: false,
            },
        ],
    );
    let _daemon = Daemon::start(&session);

    let picker = open_picker(&session);
    type_text(&session, "apple");
    session
        .input()
        .chord(&[keys::KEY_LEFTCTRL, keys::KEY_2])
        .unwrap();
    let done = picker.finish();

    assert!(done.status.success(), "{}", done.stderr);
    assert_eq!(paste(&session, None), "apple one");
}

/// Two AWS documentation example key ids: Concealed, of equal length, and
/// split so no scanner takes them for real ones.
const SECRET_A: &str = concat!("AKIA", "IOSFODNN7EXAMPLE");
const SECRET_B: &str = concat!("AKIA", "I44QH8DHBEXAMPLE");

/// The Daemon's `status` over its socket.
fn status(session: &Session) -> Value {
    use std::io::{BufRead as _, BufReader, Write as _};
    let socket = session.runtime_dir().join("hoarder/hoarder.sock");
    let mut stream = UnixStream::connect(socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    writeln!(stream, r#"{{"v":1,"id":1,"op":"status"}}"#).unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str::<Value>(&line).unwrap()["ok"].clone()
}

/// Copy `text` from a fresh app and wait for the Daemon to judge it.
fn copy_and_settle(session: &Session, text: &str, sensitivity: &str) -> hoarder_e2e::Client {
    let app = session.client().unwrap();
    app.set_selection(Selection::Clipboard, &[("text/plain", text.as_bytes())])
        .unwrap();
    wait_until("the Daemon to judge a copy", || {
        let clipboard = status(session)["clipboard"].clone();
        clipboard["reading"] == false
            && clipboard["owner"] == "app"
            && clipboard["types"] == serde_json::json!(["text/plain"])
            && clipboard["sensitivity"] == sensitivity
    });
    app
}

/// What grim sees once the screen has stopped changing.
fn settled_shot(session: &Session) -> Image {
    let mut last = session.screenshot("HEADLESS-1").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        std::thread::sleep(Duration::from_millis(150));
        let now = session.screenshot("HEADLESS-1").unwrap();
        if now == last {
            return now;
        }
        assert!(Instant::now() < deadline, "the screen never settled");
        last = now;
    }
}

/// A settled shot once the screen has changed from `before`: what a key
/// just pressed did, never the frame from before it took effect.
fn shot_after(session: &Session, before: &Image) -> Image {
    shot_until(session, |now| now != before)
}

/// A settled shot that satisfies `wanted`, polled until a deadline.
fn shot_until(session: &Session, wanted: impl Fn(&Image) -> bool) -> Image {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let now = settled_shot(session);
        if wanted(&now) {
            return now;
        }
        assert!(
            Instant::now() < deadline,
            "the screen never changed as expected"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The Picker's panel in a shot of the 1280×720 output: (280, 120) to
/// (1000, 600).
fn panel(shot: &Image) -> Vec<Rgb8> {
    (120..600)
        .flat_map(|y| (280..1000).map(move |x| (x, y)))
        .map(|(x, y)| shot.pixel(x, y))
        .collect()
}

#[test]
fn concealed_clips_stay_masked_until_alt_is_held() {
    let (session, _paint) = painted();
    let _daemon = Daemon::start(&session);

    // The same Picker over two different secrets of one length: masked,
    // they must look exactly alike.
    let app = copy_and_settle(&session, SECRET_A, "concealed");
    let picker = open_picker(&session);
    let masked_a = panel(&settled_shot(&session));
    session.input().tap(keys::KEY_ESC).unwrap();
    picker.finish();
    let cleared = session
        .command(HOARDER)
        .arg("clear")
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(cleared.success());
    drop(app);

    let _app = copy_and_settle(&session, SECRET_B, "concealed");
    let picker = open_picker(&session);
    let masked_b = panel(&settled_shot(&session));
    assert!(
        masked_a == masked_b,
        "a Concealed Clip's text showed without Alt"
    );

    // Alt reveals it, and letting go hides it again.
    let before = settled_shot(&session);
    session.input().key_down(keys::KEY_LEFTALT).unwrap();
    let revealed_shot = shot_after(&session, &before);
    let revealed = panel(&revealed_shot);
    session.input().key_up(keys::KEY_LEFTALT).unwrap();
    let hidden = panel(&shot_after(&session, &revealed_shot));
    session.input().tap(keys::KEY_ESC).unwrap();
    let done = picker.finish();

    assert_eq!(done.status.code(), Some(1), "{}", done.stderr);
    assert!(revealed != masked_b, "Alt revealed nothing");
    assert!(hidden == masked_b, "letting go of Alt left it revealed");
    // Concealed Clips never reach History.
    assert!(history(&session).is_empty());
}

#[test]
fn a_concealed_clip_restores_and_deletes_like_any_other() {
    let (session, _paint) = painted();
    let _daemon = Daemon::start(&session);
    let _secret = copy_and_settle(&session, SECRET_A, "concealed");
    let _later = copy_and_settle(&session, "later", "normal");
    wait_until("the Daemon to record a copy", || {
        history(&session).len() == 1
    });

    // Rows: "later", then the Concealed Clip.
    let picker = open_picker(&session);
    session.input().tap(keys::KEY_DOWN).unwrap();
    session.input().tap(keys::KEY_ENTER).unwrap();
    let done = picker.finish();
    assert!(done.status.success(), "{}", done.stderr);
    assert_eq!(paste(&session, None), SECRET_A);
    assert_eq!(status(&session)["clipboard"]["sensitivity"], "concealed");

    // Deleted, it is gone from the Picker and from the clipboard.
    let picker = open_picker(&session);
    session.input().tap(keys::KEY_DOWN).unwrap();
    session.input().tap(keys::KEY_DELETE).unwrap();
    session.input().tap(keys::KEY_ESC).unwrap();
    picker.finish();
    assert_eq!(paste(&session, None), "");
    let picker = open_picker(&session);
    session.input().tap(keys::KEY_DOWN).unwrap();
    session.input().tap(keys::KEY_ENTER).unwrap();
    let done = picker.finish();
    assert!(done.status.success(), "{}", done.stderr);
    assert_eq!(paste(&session, None), "later");
}

/// Where `colour` shows in `shot` within `area` (x0, y0, x1, y1): how many
/// pixels, and their bounding box's size.
fn find(shot: &Image, area: (u32, u32, u32, u32), colour: Rgb8) -> (usize, (u32, u32)) {
    let (mut n, mut min, mut max) = (0, (u32::MAX, u32::MAX), (0, 0));
    for y in area.1..area.3 {
        for x in area.0..area.2 {
            if shot.pixel(x, y) == colour {
                n += 1;
                min = (min.0.min(x), min.1.min(y));
                max = (max.0.max(x), max.1.max(y));
            }
        }
    }
    let size = if n == 0 {
        (0, 0)
    } else {
        (max.0 + 1 - min.0, max.1 + 1 - min.1)
    };
    (n, size)
}

#[test]
fn the_preview_pane_shows_an_image_at_its_real_pixel_size() {
    // Scale 2: a 1:1 image takes half the logical room a scaled one would.
    let (session, _paint) = painted_on(OutputSpec::new(2560, 1440).scale(2.0));
    let image = png(40, 30, [1, 254, 3]);
    seed(
        &session,
        &[
            Seed {
                reprs: &[("image/png", &image)],
                app: Some("org.gimp.GIMP"),
                pinned: false,
            },
            Seed {
                reprs: &text("newer text"),
                app: None,
                pinned: false,
            },
        ],
    );
    let _daemon = Daemon::start(&session);
    let panel = (560, 240, 2000, 1200);

    let picker = open_picker(&session);
    let on_text = settled_shot(&session);
    session.input().tap(keys::KEY_DOWN).unwrap();
    let on_image = shot_until(&session, |shot| find(shot, panel, Rgb8(1, 254, 3)).0 > 0);
    session.input().tap(keys::KEY_ESC).unwrap();
    picker.finish();

    assert_eq!(find(&on_text, panel, Rgb8(1, 254, 3)).0, 0);
    assert_eq!(find(&on_image, panel, Rgb8(1, 254, 3)), (40 * 30, (40, 30)));
}

#[test]
fn the_preview_pane_shows_the_whole_text_over_many_lines() {
    let (session, _paint) = painted();
    let config = session
        .runtime_dir()
        .join("home/.config/hoarder/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "[theme]\nsurface = \"#123456\"\n").unwrap();
    let many: String = (1..=12).map(|n| format!("line number {n}\n")).collect();
    seed(
        &session,
        &[
            Seed {
                reprs: &text(&many),
                app: None,
                pinned: false,
            },
            Seed {
                reprs: &text("one line"),
                app: None,
                pinned: false,
            },
        ],
    );
    let _daemon = Daemon::start(&session);
    // The right half of the panel, below the search field and above the
    // footer, lower half only: no single-line preview reaches it.
    let lower_right = (660, 380, 990, 500);
    let ink = |shot: &Image| {
        let (x0, y0, x1, y1) = lower_right;
        (y0..y1)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .filter(|&(x, y)| shot.pixel(x, y) != Rgb8(0x12, 0x34, 0x56))
            .count()
    };

    let picker = open_picker(&session);
    let one = settled_shot(&session);
    session.input().tap(keys::KEY_DOWN).unwrap();
    let twelve = shot_after(&session, &one);
    session.input().tap(keys::KEY_ESC).unwrap();
    picker.finish();

    assert_eq!(ink(&one), 0);
    assert!(ink(&twelve) > 200, "only {} inked pixels", ink(&twelve));
}

#[test]
fn the_preview_pane_shows_files_as_chips() {
    let (session, _paint) = painted();
    let config = session
        .runtime_dir()
        .join("home/.config/hoarder/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "[theme]\nsurface = \"#123456\"\n").unwrap();
    seed(
        &session,
        &[
            Seed {
                reprs: &[(
                    "text/uri-list",
                    b"file:///tmp/report.pdf\r\nfile:///home/me/notes%20today.md\r\n",
                )],
                app: None,
                pinned: false,
            },
            Seed {
                reprs: &text("x"),
                app: None,
                pinned: false,
            },
        ],
    );
    let _daemon = Daemon::start(&session);
    // The preview pane's first two chip rows.
    let chips = (660, 180, 990, 260);

    let picker = open_picker(&session);
    let on_text = settled_shot(&session);
    session.input().tap(keys::KEY_DOWN).unwrap();
    let on_files = shot_after(&session, &on_text);
    session.input().tap(keys::KEY_ESC).unwrap();
    picker.finish();

    let differs = |a: &Image, b: &Image| {
        let (x0, y0, x1, y1) = chips;
        (y0..y1)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .filter(|&(x, y)| a.pixel(x, y) != b.pixel(x, y))
            .count()
    };
    assert!(differs(&on_text, &on_files) > 200);
    // A chip is a filled block, not bare text on the panel: somewhere a
    // long run of one colour that is not the panel's. Text never makes one.
    let (x0, y0, x1, y1) = chips;
    let longest_fill = (y0..y1)
        .map(|y| {
            let mut best = 0;
            let mut run = 0;
            for x in x0..x1 {
                let p = on_files.pixel(x, y);
                let same = x > x0 && p == on_files.pixel(x - 1, y);
                run = if same { run + 1 } else { 1 };
                if p != Rgb8(0x12, 0x34, 0x56) {
                    best = best.max(run);
                }
            }
            best
        })
        .max()
        .unwrap();
    assert!(longest_fill >= 100, "no chip: longest fill {longest_fill}");
}
