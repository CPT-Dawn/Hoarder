//! Helpers shared by the Overlay end-to-end tests.

#![allow(dead_code)]

use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use hoarder_e2e::{Image, Rgb8, Rgb10, Session};

/// A decoded PNG: size, bit depth and RGB samples (8- or 16-bit values).
#[derive(Debug)]
pub struct Png {
    pub width: u32,
    pub height: u32,
    pub depth: u8,
    pub samples: Vec<u16>,
}

impl Png {
    pub fn decode(bytes: &[u8]) -> Png {
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

    pub fn rgb(&self, x: u32, y: u32) -> [u16; 3] {
        let i = ((y * self.width + x) * 3) as usize;
        [self.samples[i], self.samples[i + 1], self.samples[i + 2]]
    }

    /// The `width`×`height` part starting at `(x0, y0)`.
    pub fn crop(&self, x0: u32, y0: u32, width: u32, height: u32) -> Png {
        let samples = (y0..y0 + height)
            .flat_map(|y| (x0..x0 + width).flat_map(move |x| self.rgb(x, y)))
            .collect();
        Png {
            width,
            height,
            depth: self.depth,
            samples,
        }
    }
}

/// What a painted 10-bit colour looks like on an 8-bit output: the helper
/// client rounds each channel to the nearest 8-bit value.
pub fn as_8bit(Rgb10(r, g, b): Rgb10) -> Rgb8 {
    let n = |v: u16| ((v as u32 * 255 + 511) / 1023) as u8;
    Rgb8(n(r), n(g), n(b))
}

/// A running `hoarder shot …` whose Overlay is up.
#[derive(Debug)]
pub struct Overlay {
    child: Option<Child>,
}

impl Overlay {
    /// Start `hoarder` with `args` and return once its Overlay has changed
    /// what every output in `outputs` shows (as seen by grim), and has
    /// finished fading in.
    pub fn start(session: &Session, args: &[&str], outputs: &[&str]) -> Overlay {
        Self::start_with(session, args, outputs, |_| {})
    }

    pub fn start_with(
        session: &Session,
        args: &[&str],
        outputs: &[&str],
        tweak: impl FnOnce(&mut Command),
    ) -> Overlay {
        let before: Vec<Image> = outputs
            .iter()
            .map(|o| session.screenshot(o).unwrap())
            .collect();
        let mut cmd = session.command(env!("CARGO_BIN_EXE_hoarder"));
        cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        tweak(&mut cmd);
        let mut child = cmd.spawn().expect("hoarder should start");
        let deadline = Instant::now() + Duration::from_secs(10);
        for (name, before) in outputs.iter().zip(before) {
            while session.screenshot(name).unwrap() == before {
                if let Some(status) = child.try_wait().unwrap() {
                    let out = child.wait_with_output().unwrap();
                    panic!(
                        "hoarder exited ({status}) before showing the Overlay: {}",
                        String::from_utf8_lossy(&out.stderr)
                    );
                }
                assert!(
                    Instant::now() < deadline,
                    "the Overlay never appeared on {name}"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        // The Overlay fades in over 100 ms: wait until the dimming is done,
        // so what a test looks at next is the Overlay at rest.
        std::thread::sleep(Duration::from_millis(120));
        let settle = Instant::now() + Duration::from_secs(1);
        for name in outputs {
            let mut last = session.screenshot(name).unwrap();
            while Instant::now() < settle {
                std::thread::sleep(Duration::from_millis(40));
                let now = session.screenshot(name).unwrap();
                if now == last {
                    break;
                }
                last = now;
            }
        }
        Overlay { child: Some(child) }
    }

    /// Wait (at most 10 s) for hoarder to exit. Its output is drained
    /// meanwhile, so a PNG bigger than a pipe buffer cannot stall it.
    pub fn finish(mut self) -> Finished {
        let mut child = self.child.take().expect("still running");
        let drain = |r: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                if let Some(mut r) = r {
                    let _ = r.read_to_end(&mut buf);
                }
                buf
            })
        };
        let stdout = drain(child.stdout.take().map(|r| Box::new(r) as _));
        let stderr = drain(child.stderr.take().map(|r| Box::new(r) as _));
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "hoarder did not exit: {}",
                    String::from_utf8_lossy(&stderr.join().unwrap())
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        Finished {
            status,
            stdout: stdout.join().unwrap(),
            stderr: String::from_utf8_lossy(&stderr.join().unwrap()).into_owned(),
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// How a `hoarder` run ended.
#[derive(Debug)]
pub struct Finished {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Finished {
    /// The PNG written to stdout by a successful run.
    pub fn png(&self) -> Png {
        assert!(
            self.status.success(),
            "hoarder failed ({}): {}",
            self.status,
            self.stderr
        );
        Png::decode(&self.stdout)
    }
}
