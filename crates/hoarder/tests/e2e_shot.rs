//! `hoarder shot all -o …`: All Outputs frozen and written as a PNG, compared
//! pixel for pixel against the pattern the helper client painted.

use hoarder_e2e::{OutputSpec, Pattern, Session, Transform};

/// A decoded PNG: size, bit depth and RGB samples (8- or 16-bit values).
#[derive(Debug)]
struct Png {
    width: u32,
    height: u32,
    depth: u8,
    samples: Vec<u16>,
}

impl Png {
    fn decode(bytes: &[u8]) -> Png {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::IDENTITY);
        let mut reader = decoder.read_info().expect("a valid PNG header");
        let mut buf = vec![0; reader.output_buffer_size().expect("sane size")];
        let info = reader.next_frame(&mut buf).expect("a valid PNG image");
        assert_eq!(info.color_type, png::ColorType::Rgb, "PNG colour type");
        let depth = info.bit_depth as u8;
        let samples = match depth {
            8 => buf[..info.buffer_size()]
                .iter()
                .map(|&v| v as u16)
                .collect(),
            16 => buf[..info.buffer_size()]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&c| u16::from_be_bytes(c))
                .collect(),
            d => panic!("unexpected bit depth {d}"),
        };
        Png {
            width: info.width,
            height: info.height,
            depth,
            samples,
        }
    }

    fn rgb(&self, x: u32, y: u32) -> [u16; 3] {
        let i = ((y * self.width + x) * 3) as usize;
        [self.samples[i], self.samples[i + 1], self.samples[i + 2]]
    }
}

/// Run `hoarder shot all -o -` in `session` and decode what it printed.
fn shot_all(session: &Session) -> Png {
    let out = session
        .command(env!("CARGO_BIN_EXE_hoarder"))
        .args(["shot", "all", "-o", "-"])
        .output()
        .expect("hoarder should run");
    assert!(
        out.status.success(),
        "hoarder shot all failed ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    Png::decode(&out.stdout)
}

/// Asserts `png` is exactly `pattern` as painted on a `w`×`h` physical surface,
/// starting at `(x0, 0)` in the image, in 8 bits per channel.
fn assert_pattern_8bit(png: &Png, pattern: &Pattern, x0: u32, (w, h): (u32, u32), what: &str) {
    assert_eq!(png.depth, 8, "an 8-bit output gives an 8-bit PNG");
    let mut wrong = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let c = pattern.colour_at(x, y, w, h);
            // Every colour the patterns use is exact in 8 bits (bit-replicated).
            let want = [c.0 >> 2, c.1 >> 2, c.2 >> 2];
            let got = png.rgb(x0 + x, y);
            if got != want && wrong.len() < 5 {
                wrong.push(format!("({x}, {y}): got {got:?}, want {want:?}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{what}: pixels differ: {wrong:#?}");
}

#[test]
fn shot_all_writes_the_painted_pixels_of_one_output_to_stdout() {
    let session = Session::start(&[OutputSpec::new(640, 480)]).unwrap();
    let client = session.client().unwrap();
    let pattern = Pattern::Checker { cell: 1 };
    client
        .paint(session.output("HEADLESS-1"), &pattern)
        .unwrap();

    let png = shot_all(&session);

    assert_eq!((png.width, png.height), (640, 480));
    assert_pattern_8bit(&png, &pattern, 0, (640, 480), "HEADLESS-1");
}

/// Paints a one-pixel checkerboard on a single `spec` output and expects it back
/// byte for byte at native resolution: any resampling would smear it grey.
fn assert_byte_exact(spec: OutputSpec) {
    let session = Session::start(&[spec]).unwrap();
    let client = session.client().unwrap();
    let pattern = Pattern::Checker { cell: 1 };
    client
        .paint(session.output("HEADLESS-1"), &pattern)
        .unwrap();

    let png = shot_all(&session);

    let size = spec.physical_size();
    assert_eq!((png.width, png.height), size, "native buffer size");
    assert_pattern_8bit(&png, &pattern, 0, size, &format!("scale {}", spec.scale));
}

#[test]
fn shot_all_is_byte_exact_at_fractional_scale_1_25() {
    assert_byte_exact(OutputSpec::new(600, 480).scale(1.25));
}

#[test]
fn shot_all_is_byte_exact_at_fractional_scale_1_5() {
    assert_byte_exact(OutputSpec::new(600, 480).scale(1.5));
}

#[test]
fn shot_all_is_byte_exact_at_scale_2() {
    assert_byte_exact(OutputSpec::new(600, 480).scale(2.0));
}

/// Paints quadrants (every corner a different colour) on a rotated or flipped
/// output and expects them back upright, exactly as the user sees them.
fn assert_upright(transform: Transform) {
    let spec = OutputSpec::new(640, 480).transform(transform);
    let session = Session::start(&[spec]).unwrap();
    let client = session.client().unwrap();
    client
        .paint(session.output("HEADLESS-1"), &Pattern::Quadrants)
        .unwrap();

    let png = shot_all(&session);

    let size = spec.physical_size();
    assert_eq!(
        (png.width, png.height),
        size,
        "upright size for {transform:?}"
    );
    assert_pattern_8bit(
        &png,
        &Pattern::Quadrants,
        0,
        size,
        &format!("{transform:?}"),
    );
}

#[test]
fn shot_all_turns_a_90_degree_output_upright() {
    assert_upright(Transform::Rotate90);
}

#[test]
fn shot_all_turns_a_270_degree_output_upright() {
    assert_upright(Transform::Rotate270);
}

#[test]
fn shot_all_turns_a_180_degree_output_upright() {
    assert_upright(Transform::Rotate180);
}

#[test]
fn shot_all_unflips_flipped_outputs() {
    for transform in [
        Transform::Flipped,
        Transform::Flipped90,
        Transform::Flipped180,
        Transform::Flipped270,
    ] {
        assert_upright(transform);
    }
}
