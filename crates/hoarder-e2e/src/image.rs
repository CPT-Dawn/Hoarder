//! Screenshots taken by `grim`, an oracle independent of Hoarder.

use crate::{Error, Result, Rgb8, Session};

/// An 8-bit RGB image as the user sees it: physical pixels, transform applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

impl Image {
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn pixel(&self, x: u32, y: u32) -> Rgb8 {
        assert!(
            x < self.width && y < self.height,
            "({x}, {y}) is outside {}x{}",
            self.width,
            self.height
        );
        let i = ((y * self.width + x) * 3) as usize;
        Rgb8(self.rgb[i], self.rgb[i + 1], self.rgb[i + 2])
    }

    /// Parse a binary PPM (`P6`, maxval 255).
    pub fn from_ppm(data: &[u8]) -> Result<Self> {
        let bad = |why: &str| Error::Command(format!("bad PPM: {why}"));
        let mut fields = Vec::with_capacity(4);
        let mut i = 0;
        while fields.len() < 4 {
            while i < data.len() && (data[i].is_ascii_whitespace() || data[i] == b'#') {
                if data[i] == b'#' {
                    while i < data.len() && data[i] != b'\n' {
                        i += 1;
                    }
                }
                i += 1;
            }
            let start = i;
            while i < data.len() && !data[i].is_ascii_whitespace() {
                i += 1;
            }
            if start == i {
                return Err(bad("truncated header"));
            }
            fields.push(std::str::from_utf8(&data[start..i]).map_err(|_| bad("header"))?);
        }
        i += 1; // the single whitespace byte after maxval
        if fields[0] != "P6" || fields[3] != "255" {
            return Err(bad("not an 8-bit P6"));
        }
        let width: u32 = fields[1].parse().map_err(|_| bad("width"))?;
        let height: u32 = fields[2].parse().map_err(|_| bad("height"))?;
        let rgb = data.get(i..).ok_or_else(|| bad("no pixels"))?.to_vec();
        if rgb.len() != (width * height * 3) as usize {
            return Err(bad("pixel data length"));
        }
        Ok(Image { width, height, rgb })
    }
}

impl Session {
    /// Capture one output with `grim`, at its native resolution.
    pub fn screenshot(&self, output: &str) -> Result<Image> {
        let out = self
            .command("grim")
            .args(["-t", "ppm", "-o", output, "-"])
            .output()
            .map_err(|e| Error::Setup(format!("cannot run grim (is it installed?): {e}")))?;
        if !out.status.success() {
            return Err(Error::Command(format!(
                "grim -o {output} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        Image::from_ppm(&out.stdout)
    }
}
