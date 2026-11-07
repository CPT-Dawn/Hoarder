//! The Picker (PLAN.md §6): `hoarder picker` lists History in a centred
//! panel, typing searches it, Enter Restores the selected Clip and Esc
//! closes. Driven through the virtual keyboard and judged by `wl-paste`,
//! exit codes and what grim sees.

mod common;

use std::os::unix::net::UnixStream;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use common::Overlay;
use hoarder_e2e::{Client, OutputSpec, Pattern, Rgb8, Selection, Session, budget, keys};
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

/// `hoarder history list --json`, newest first.
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

/// Copy each text in turn from a fresh app, waiting for the Daemon to
/// record it; the last one ends up on top of History.
fn copy_all(session: &Session, texts: &[&str]) -> Vec<Client> {
    let mut apps = Vec::new();
    for (n, text) in texts.iter().enumerate() {
        let app = session.client().unwrap();
        app.set_selection(Selection::Clipboard, &[("text/plain", text.as_bytes())])
            .unwrap();
        apps.push(app);
        wait_until("the Daemon to record a copy", || {
            history(session).len() == n + 1
        });
    }
    apps
}

/// `wl-paste --no-newline`.
fn paste(session: &Session) -> String {
    let out = session
        .command("wl-paste")
        .arg("--no-newline")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A session with something painted, so the Picker showing up changes what
/// grim sees.
fn painted() -> (Session, Client) {
    let session = Session::start_default().unwrap();
    let app = session.client().unwrap();
    app.paint(session.output("HEADLESS-1"), &Pattern::Quadrants)
        .unwrap();
    (session, app)
}

/// Type `text` (lowercase letters, digits and spaces) on the virtual keyboard.
fn type_text(session: &Session, text: &str) {
    for c in text.chars() {
        let key = match c {
            ' ' => keys::KEY_SPACE,
            '0' => keys::KEY_0,
            '1'..='9' => keys::KEY_1 + (c as u32 - '1' as u32),
            'a'..='z' => {
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
                LETTERS[(c as u8 - b'a') as usize]
            }
            _ => panic!("cannot type {c:?}"),
        };
        session.input().tap(key).unwrap();
    }
}

fn picker(session: &Session) -> Overlay {
    Overlay::start(session, &["picker"], &["HEADLESS-1"])
}

#[test]
fn without_the_daemon_the_picker_says_so_and_shows_nothing() {
    let session = Session::start_default().unwrap();

    let out = session
        .command(HOARDER)
        .arg("picker")
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(4));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Daemon is not running"), "{stderr}");
}

#[test]
fn typing_selecting_and_enter_restores_that_clip() {
    let (session, _paint) = painted();
    let _daemon = Daemon::start(&session);
    let _apps = copy_all(
        &session,
        &["apple pie", "banana bread", "banana split", "cherry tart"],
    );

    let picker = picker(&session);
    // "banana" matches two Clips; the newer one is first, so Down picks
    // "banana bread".
    type_text(&session, "banana");
    session.input().tap(keys::KEY_DOWN).unwrap();
    session.input().tap(keys::KEY_ENTER).unwrap();
    let done = picker.finish();

    assert!(done.status.success(), "{}", done.stderr);
    assert_eq!(paste(&session), "banana bread");
    assert_eq!(history(&session)[0]["preview"], "banana bread");
}

#[test]
fn enter_with_no_query_restores_the_newest_clip() {
    let (session, _paint) = painted();
    let _daemon = Daemon::start(&session);
    let _apps = copy_all(&session, &["older", "newest"]);
    // The clipboard is emptied, so only a Restore can bring "newest" back.
    drop(_apps);
    let cleared = session
        .command(HOARDER)
        .arg("clear")
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(cleared.success());
    assert_eq!(paste(&session), "");

    let picker = picker(&session);
    session.input().tap(keys::KEY_ENTER).unwrap();
    let done = picker.finish();

    assert!(done.status.success(), "{}", done.stderr);
    assert_eq!(paste(&session), "newest");
}

#[test]
fn esc_closes_the_picker_and_leaves_the_clipboard_alone() {
    let (session, _paint) = painted();
    let _daemon = Daemon::start(&session);
    let _apps = copy_all(&session, &["first", "second"]);
    let before = session.screenshot("HEADLESS-1").unwrap();

    let picker = picker(&session);
    session.input().tap(keys::KEY_DOWN).unwrap();
    session.input().tap(keys::KEY_ESC).unwrap();
    let done = picker.finish();

    assert_eq!(done.status.code(), Some(1), "cancelled: {}", done.stderr);
    assert_eq!(paste(&session), "second");
    wait_until("the Picker to leave the screen", || {
        session.screenshot("HEADLESS-1").unwrap() == before
    });
}

#[test]
fn long_text_is_found_past_its_preview() {
    let (session, _paint) = painted();
    let _daemon = Daemon::start(&session);
    let long = format!("{} haystack needle", "lorem ipsum ".repeat(40));
    let _apps = copy_all(&session, &[&long, "something newer"]);
    assert!(
        !history(&session)[1]["preview"]
            .as_str()
            .unwrap()
            .contains("needle"),
        "the needle must lie past the preview for this test to mean anything"
    );

    let picker = picker(&session);
    type_text(&session, "needle");
    session.input().tap(keys::KEY_ENTER).unwrap();
    let done = picker.finish();

    assert!(done.status.success(), "{}", done.stderr);
    assert_eq!(paste(&session), long);
}

#[test]
fn a_query_that_matches_nothing_restores_nothing() {
    let (session, _paint) = painted();
    let _daemon = Daemon::start(&session);
    let _apps = copy_all(&session, &["alpha", "beta"]);

    let picker = picker(&session);
    type_text(&session, "zzzz");
    session.input().tap(keys::KEY_ENTER).unwrap();
    session.input().tap(keys::KEY_ESC).unwrap();
    let done = picker.finish();

    assert_eq!(done.status.code(), Some(1), "{}", done.stderr);
    assert_eq!(paste(&session), "beta");
}

/// PLAN.md §12: the Picker's first frame within 50 ms, with 1,000 Clips in
/// History. Only enforced in optimised builds:
/// `cargo test --release -p hoarder --test e2e_picker budget -- --nocapture`.
#[test]
fn budget_picker_first_frame_with_1000_clips() {
    let (session, _paint) = painted();
    {
        let dir = session.runtime_dir().join("home/.local/share/hoarder");
        let mut store = hoarder_store::Store::open(&dir).unwrap();
        // Recent, or retention would evict them as soon as the Daemon starts.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            - 60_000;
        for n in 0..1000 {
            let text = format!("Clip number {n}: {}", "some words to fill a row ".repeat(4));
            store
                .record(&[("text/plain", text.as_bytes())], Some("kitty"), now + n)
                .unwrap();
        }
    }
    let _daemon = Daemon::start(&session);
    assert_eq!(history(&session).len(), 1000);

    let mut samples = Vec::new();
    for _ in 0..5 {
        let picker = Overlay::start_with(&session, &["picker"], &["HEADLESS-1"], |cmd| {
            cmd.env("HOARDER_TIMINGS", "1");
        });
        session.input().tap(keys::KEY_ESC).unwrap();
        let done = picker.finish();
        eprintln!("{}", done.stderr.trim_end());
        let ms: f64 = done
            .stderr
            .lines()
            .find_map(|l| l.split("picker_visible_ms=").nth(1))
            .unwrap_or_else(|| panic!("no timing printed: {}", done.stderr))
            .trim()
            .parse()
            .unwrap();
        samples.push(ms);
    }
    samples.sort_by(f64::total_cmp);
    let median = samples[samples.len() / 2];
    eprintln!("Picker first frame with 1,000 Clips, samples (ms): {samples:?}");
    budget::assert_within(
        "Picker first frame with 1,000 Clips, median of 5",
        Duration::from_secs_f64(median / 1000.0),
        Duration::from_millis(50),
    );
}

#[test]
fn the_picker_wears_the_theme_and_sits_centred() {
    let session = Session::start(&[OutputSpec::new(1280, 720)]).unwrap();
    let paint = session.client().unwrap();
    paint
        .paint(session.output("HEADLESS-1"), &Pattern::Quadrants)
        .unwrap();
    let config = session
        .runtime_dir()
        .join("home/.config/hoarder/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        "[theme]\nname = \"gruvbox-dark\"\nsurface = \"#123456\"\n",
    )
    .unwrap();
    let _daemon = Daemon::start(&session);
    let _apps = copy_all(&session, &["one", "two"]);

    let picker = picker(&session);
    let seen = session.screenshot("HEADLESS-1").unwrap();
    session.input().tap(keys::KEY_ESC).unwrap();
    picker.finish();

    // The 720×480 panel is centred, (280, 120) to (1000, 600). Below its
    // two rows lies bare panel, in the overridden (opaque) surface colour.
    assert_eq!(seen.pixel(640, 120 + 400), Rgb8(0x12, 0x34, 0x56));
    assert_eq!(seen.pixel(285, 300), Rgb8(0x12, 0x34, 0x56));
    // Around it, the desktop shows untouched.
    assert_eq!(seen.pixel(275, 300), Rgb8(255, 0, 0));
    assert_eq!(seen.pixel(1005, 300), Rgb8(0, 255, 0));
    assert_eq!(seen.pixel(400, 115), Rgb8(255, 0, 0));
}
