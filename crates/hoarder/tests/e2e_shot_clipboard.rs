//! `hoarder shot` puts a lossless PNG on the clipboard that outlives it, and
//! says once that History is off when no Daemon is there to keep it.

use std::process::Stdio;

use hoarder_e2e::{OutputSpec, Pattern, Session};

const HOARDER: &str = env!("CARGO_BIN_EXE_hoarder");

/// Decode an 8-bit RGB PNG into its size and samples.
fn decode_rgb8(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::IDENTITY);
    let mut reader = decoder.read_info().expect("a valid PNG header");
    let mut buf = vec![0; reader.output_buffer_size().expect("sane size")];
    let info = reader.next_frame(&mut buf).expect("a valid PNG image");
    assert_eq!(info.color_type, png::ColorType::Rgb);
    assert_eq!(info.bit_depth, png::BitDepth::Eight);
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

fn wl_paste(session: &Session, args: &[&str]) -> std::process::Output {
    session
        .command("wl-paste")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("wl-paste should run")
}

/// Run `hoarder shot all` with stdio detached from any pipe a serving child
/// could hold open, returning its exit status and stderr.
fn shot_all(session: &Session) -> (std::process::ExitStatus, String) {
    let err = session.runtime_dir().join("shot.stderr");
    let status = session
        .command(HOARDER)
        .args(["shot", "all"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&err).unwrap())
        .status()
        .expect("hoarder should run");
    (status, std::fs::read_to_string(err).unwrap())
}

#[test]
fn a_capture_is_pasteable_after_shot_has_exited() {
    let session = Session::start(&[OutputSpec::new(320, 240)]).unwrap();
    let painter = session.client().unwrap();
    painter
        .paint(session.output("HEADLESS-1"), &Pattern::Quadrants)
        .unwrap();

    let (status, stderr) = shot_all(&session);
    assert!(status.success(), "hoarder shot all failed: {stderr}");

    let types = wl_paste(&session, &["--list-types"]);
    assert_eq!(String::from_utf8_lossy(&types.stdout), "image/png\n");
    let pasted = wl_paste(&session, &["--type", "image/png"]);
    assert!(pasted.status.success());
    let (w, h, samples) = decode_rgb8(&pasted.stdout);
    assert_eq!((w, h), (320, 240));
    for (x, y) in [(10, 10), (300, 10), (10, 230), (300, 230), (160, 120)] {
        let c = Pattern::Quadrants.colour_at(x, y, w, h);
        let i = ((y * w + x) * 3) as usize;
        assert_eq!(
            &samples[i..i + 3],
            &[(c.0 >> 2) as u8, (c.1 >> 2) as u8, (c.2 >> 2) as u8],
            "pixel ({x}, {y})"
        );
    }
}

#[test]
fn shot_warns_once_that_history_is_off_without_a_daemon() {
    let session = Session::start_default().unwrap();
    let (status, stderr) = shot_all(&session);
    assert!(status.success(), "hoarder shot all failed: {stderr}");
    let warnings: Vec<&str> = stderr
        .lines()
        .filter(|l| l.contains("History is off"))
        .collect();
    assert_eq!(warnings.len(), 1, "stderr was {stderr:?}");
    assert_eq!(
        stderr.lines().count(),
        1,
        "nothing else is printed: {stderr:?}"
    );
}

#[test]
fn no_copy_leaves_the_clipboard_alone() {
    let session = Session::start_default().unwrap();
    let out = session
        .command(HOARDER)
        .args(["shot", "all", "--no-copy", "-o", "-"])
        .stdin(Stdio::null())
        .output()
        .expect("hoarder should run");
    assert!(out.status.success());
    assert!(out.stderr.is_empty(), "no warning without a copy");
    assert!(!wl_paste(&session, &[]).status.success(), "nothing copied");
}
