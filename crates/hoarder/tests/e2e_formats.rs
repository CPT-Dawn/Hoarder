//! Saved formats (PLAN.md §4.5): every format `hoarder shot` can save decodes
//! again, lossless ones byte for byte, and `--format`, `-o`, `--no-save` and
//! the `screenshot.*` encoder settings pick what is written.

use std::path::{Path, PathBuf};
use std::process::Output;

use hoarder_e2e::{Client, OutputSpec, Pattern, Session};

const SIZE: (u32, u32) = (320, 240);

fn hoarder(session: &Session, args: &[&str]) -> Output {
    session
        .command(env!("CARGO_BIN_EXE_hoarder"))
        .args(args)
        .output()
        .expect("hoarder should run")
}

fn assert_ok(out: &Output) {
    assert!(
        out.status.success(),
        "hoarder failed ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn home(session: &Session) -> PathBuf {
    session.runtime_dir().join("home")
}

fn write_config(session: &Session, text: &str) {
    let dir = home(session).join(".config/hoarder");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), text).unwrap();
}

/// A session with one `SIZE` output showing `pattern`, painted by the client
/// returned alongside (the paint lasts as long as the client).
fn painted(pattern: &Pattern) -> (Session, Client) {
    let session = Session::start(&[OutputSpec::new(SIZE.0, SIZE.1)]).unwrap();
    let client = session.client().unwrap();
    client.paint(session.output("HEADLESS-1"), pattern).unwrap();
    (session, client)
}

/// Run `hoarder` and return the one file it saved (the path it printed).
fn shot_saved(session: &Session, args: &[&str]) -> PathBuf {
    let out = hoarder(session, args);
    assert_ok(&out);
    let path = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim_end());
    assert!(path.is_file(), "printed path is a file: {path:?}");
    path
}

/// Decode with the `image` crate, independently of how hoarder encoded.
fn decode(bytes: &[u8], format: image::ImageFormat) -> image::RgbImage {
    image::load_from_memory_with_format(bytes, format)
        .unwrap_or_else(|e| panic!("a valid {format:?}: {e}"))
        .to_rgb8()
}

/// What `pattern` looks like in 8 bits on the `SIZE` output.
fn expected(pattern: &Pattern) -> image::RgbImage {
    image::RgbImage::from_fn(SIZE.0, SIZE.1, |x, y| {
        let c = pattern.colour_at(x, y, SIZE.0, SIZE.1);
        image::Rgb([(c.0 >> 2) as u8, (c.1 >> 2) as u8, (c.2 >> 2) as u8])
    })
}

fn assert_byte_exact(got: &image::RgbImage, pattern: &Pattern, what: &str) {
    assert_eq!(got.dimensions(), SIZE, "{what}: size");
    let want = expected(pattern);
    let wrong: Vec<_> = got
        .enumerate_pixels()
        .filter(|(x, y, p)| *p != want.get_pixel(*x, *y))
        .take(5)
        .map(|(x, y, p)| format!("({x}, {y}): got {p:?}, want {:?}", want.get_pixel(x, y)))
        .collect();
    assert!(wrong.is_empty(), "{what}: pixels differ: {wrong:#?}");
}

fn extension(path: &Path) -> &str {
    path.extension().and_then(|e| e.to_str()).unwrap_or("")
}

#[test]
fn format_qoi_saves_a_byte_exact_qoi_file() {
    let pattern = Pattern::Checker { cell: 1 };
    let (session, _client) = painted(&pattern);

    let path = shot_saved(&session, &["shot", "all", "--format", "qoi"]);

    assert_eq!(extension(&path), "qoi");
    let got = decode(&std::fs::read(&path).unwrap(), image::ImageFormat::Qoi);
    assert_byte_exact(&got, &pattern, "qoi");
}

#[test]
fn webp_is_lossless_and_byte_exact_by_default() {
    let pattern = Pattern::Checker { cell: 1 };
    let (session, _client) = painted(&pattern);
    write_config(&session, "[screenshot]\nformat = \"webp\"\n");

    let path = shot_saved(&session, &["shot", "all"]);

    assert_eq!(extension(&path), "webp");
    let got = decode(&std::fs::read(&path).unwrap(), image::ImageFormat::WebP);
    assert_byte_exact(&got, &pattern, "webp lossless");
}

#[test]
fn every_png_level_is_byte_exact_and_best_beats_none() {
    let pattern = Pattern::Checker { cell: 1 };
    let (session, _client) = painted(&pattern);

    let mut sizes = Vec::new();
    for level in ["0", "1", "9", "\"fast\"", "\"best\""] {
        let set = format!("screenshot.png.level={level}");
        let path = shot_saved(&session, &["--set", &set, "shot", "all"]);
        let bytes = std::fs::read(&path).unwrap();
        let got = decode(&bytes, image::ImageFormat::Png);
        assert_byte_exact(&got, &pattern, &format!("png level {level}"));
        sizes.push(bytes.len());
    }
    // A checkerboard compresses to almost nothing; level 0 stores it raw.
    assert!(sizes[4] * 10 < sizes[0], "best vs none: {sizes:?}");
}

/// Mean absolute difference per channel between `got` and `pattern`.
fn mean_error(got: &image::RgbImage, pattern: &Pattern) -> f64 {
    assert_eq!(got.dimensions(), SIZE, "size");
    let want = expected(pattern);
    let total: u64 = got
        .as_raw()
        .iter()
        .zip(want.as_raw())
        .map(|(&a, &b)| a.abs_diff(b) as u64)
        .sum();
    total as f64 / got.as_raw().len() as f64
}

/// A lossy `format` (with `quality_key` its quality setting) stays close to the
/// picture, and a lower quality gives a smaller file on a detailed one.
fn assert_lossy(format: &str, quality_key: &str, decode: impl Fn(&Path) -> image::RgbImage) {
    let flat = Pattern::Quadrants;
    let (session, _client) = painted(&flat);
    let path = shot_saved(&session, &["shot", "all", "--format", format]);
    let ext = if format == "jpeg" { "jpg" } else { format };
    assert_eq!(extension(&path), ext);
    let error = mean_error(&decode(&path), &flat);
    assert!(error < 4.0, "{format}: mean error {error}");

    let detailed = Pattern::Ramp;
    let (session, _client) = painted(&detailed);
    let size_at = |quality: &str| {
        let set = format!("{quality_key}={quality}");
        let path = shot_saved(
            &session,
            &["--set", &set, "shot", "all", "--format", format],
        );
        std::fs::metadata(path).unwrap().len()
    };
    let (low, high) = (size_at("10"), size_at("95"));
    assert!(
        low < high,
        "{format}: quality 10 is {low} bytes, 95 is {high}"
    );
}

#[test]
fn lossy_webp_follows_its_quality() {
    let lossy = "screenshot.webp.lossless=false";
    let (session, _client) = painted(&Pattern::Quadrants);
    let path = shot_saved(
        &session,
        &["--set", lossy, "shot", "all", "--format", "webp"],
    );
    let bytes = std::fs::read(path).unwrap();
    // Really lossy WebP (a VP8 chunk), not the lossless kind (VP8L).
    assert_eq!(&bytes[12..16], b"VP8 ", "lossy chunk");
    assert!(
        mean_error(
            &decode(&bytes, image::ImageFormat::WebP),
            &Pattern::Quadrants
        ) < 4.0
    );

    let (session, _client) = painted(&Pattern::Ramp);
    let size_at = |quality: &str| {
        let set = format!("screenshot.webp.quality={quality}");
        let args = [
            "--set", lossy, "--set", &set, "shot", "all", "--format", "webp",
        ];
        std::fs::metadata(shot_saved(&session, &args))
            .unwrap()
            .len()
    };
    let (low, high) = (size_at("10"), size_at("95"));
    assert!(low < high, "webp: quality 10 is {low} bytes, 95 is {high}");
}

#[test]
fn jpeg_follows_its_quality() {
    assert_lossy("jpeg", "screenshot.jpeg.quality", |path| {
        decode(&std::fs::read(path).unwrap(), image::ImageFormat::Jpeg)
    });
}

/// AVIF decoded by libavif's `avifdec`, an independent decoder.
fn avifdec(path: &Path) -> image::RgbImage {
    let png = path.with_extension("decoded.png");
    let out = std::process::Command::new("avifdec")
        .arg(path)
        .arg(&png)
        .output()
        .expect("avifdec (libavif) should be installed");
    assert!(out.status.success(), "avifdec: {out:?}");
    decode(&std::fs::read(png).unwrap(), image::ImageFormat::Png)
}

#[test]
fn avif_follows_its_quality() {
    assert_lossy("avif", "screenshot.avif.quality", avifdec);
}

#[test]
fn o_takes_the_format_from_its_extension_and_format_overrides_it() {
    let pattern = Pattern::Checker { cell: 1 };
    let (session, _client) = painted(&pattern);
    let qoi = home(&session).join("cap.qoi");
    let webp_named_png = home(&session).join("odd.png");

    assert_ok(&hoarder(
        &session,
        &["shot", "all", "-o", qoi.to_str().unwrap()],
    ));
    let webp_args = [
        "shot",
        "all",
        "--format",
        "webp",
        "-o",
        webp_named_png.to_str().unwrap(),
    ];
    assert_ok(&hoarder(&session, &webp_args));
    let stdout = hoarder(&session, &["shot", "all", "--format", "qoi", "-o", "-"]);
    assert_ok(&stdout);

    let read = |p: &Path| std::fs::read(p).unwrap();
    let qoi_got = decode(&read(&qoi), image::ImageFormat::Qoi);
    assert_byte_exact(&qoi_got, &pattern, "-o x.qoi");
    let webp_got = decode(&read(&webp_named_png), image::ImageFormat::WebP);
    assert_byte_exact(&webp_got, &pattern, "--format webp -o x.png");
    let stdout_got = decode(&stdout.stdout, image::ImageFormat::Qoi);
    assert_byte_exact(&stdout_got, &pattern, "--format qoi -o -");
    assert!(
        !home(&session).join("Pictures").exists(),
        "-o saves nothing else"
    );
}

#[test]
fn no_save_saves_nothing_even_when_the_config_says_save() {
    let (session, _client) = painted(&Pattern::Quadrants);
    write_config(&session, "[screenshot]\nsave = true\n");

    let out = hoarder(&session, &["shot", "all", "--no-save", "--no-copy"]);

    assert_ok(&out);
    assert!(out.stdout.is_empty(), "no path printed: {out:?}");
    assert!(!home(&session).join("Pictures").exists(), "nothing saved");
}

#[test]
fn an_unknown_format_is_a_usage_error() {
    let (session, _client) = painted(&Pattern::Quadrants);

    let out = hoarder(&session, &["shot", "all", "--format", "gif"]);

    assert_eq!(out.status.code(), Some(2), "{out:?}");
}
