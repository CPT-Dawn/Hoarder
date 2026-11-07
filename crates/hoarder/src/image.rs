//! Pixels after the freeze: Frozen Frames turned upright without resampling,
//! laid out as one image, and encoded (PLAN.md §4.1 steps 3, 5 and 7).

use hoarder_config::PngLevel;
use hoarder_wayland::{FrozenFrame, Transform};

/// Bits per channel of an [`Image`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    Eight,
    /// 10-bit sources, widened by bit replication so `v16 >> 6` gives back the original.
    Sixteen,
}

/// An upright RGB image. Samples hold 8- or 16-bit values according to `depth`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub depth: Depth,
    samples: Vec<u16>,
}

impl Image {
    pub(crate) fn new(width: u32, height: u32, depth: Depth) -> Self {
        Image {
            width,
            height,
            depth,
            samples: vec![0; width as usize * height as usize * 3],
        }
    }

    fn index(&self, x: u32, y: u32) -> usize {
        (y as usize * self.width as usize + x as usize) * 3
    }

    /// Undo `frame`'s transform and y-invert by moving pixels, nothing else.
    pub fn upright(frame: &FrozenFrame) -> Image {
        let (w, h) = (frame.width, frame.height);
        let (uw, uh) = frame.upright_size();
        let ten_bit = frame.format.bits_per_channel() == 10;
        let depth = if ten_bit {
            Depth::Sixteen
        } else {
            Depth::Eight
        };
        let mut image = Image::new(uw, uh, depth);
        for row in 0..h {
            let by = if frame.y_invert { h - 1 - row } else { row };
            for bx in 0..w {
                let (ux, uy) = upright_position(frame.transform, bx, by, w, h);
                let mut rgb = frame.format.rgb(frame.pixel(bx, row));
                if ten_bit {
                    rgb = rgb.map(|v| (v << 6) | (v >> 4));
                }
                let i = image.index(ux, uy);
                image.samples[i..i + 3].copy_from_slice(&rgb);
            }
        }
        image
    }

    /// All Outputs as one image, each output at its place in the layout.
    ///
    /// Outputs at the highest scale are copied pixel for pixel. Outputs at a
    /// lower scale are enlarged by nearest neighbour (`capture.mixed_scale =
    /// "upscale"`, PLAN.md §4.4), so no pixel is ever blended.
    pub fn all_outputs(frames: &[FrozenFrame]) -> Image {
        let parts: Vec<(Image, &FrozenFrame)> =
            frames.iter().map(|f| (Image::upright(f), f)).collect();
        if let [(image, _)] = parts.as_slice() {
            return image.clone();
        }
        let scale_of = |image: &Image, frame: &FrozenFrame| {
            image.width as f64 / frame.logical.width.max(1) as f64
        };
        let scale = parts
            .iter()
            .map(|(i, f)| scale_of(i, f))
            .fold(0.0_f64, f64::max);
        let depth = if parts.iter().any(|(i, _)| i.depth == Depth::Sixteen) {
            Depth::Sixteen
        } else {
            Depth::Eight
        };
        let min_x = parts.iter().map(|(_, f)| f.logical.x).min().unwrap_or(0);
        let min_y = parts.iter().map(|(_, f)| f.logical.y).min().unwrap_or(0);

        let placed: Vec<(&Image, u32, u32, u32, u32)> = parts
            .iter()
            .map(|(image, frame)| {
                let x = ((frame.logical.x - min_x) as f64 * scale).round() as u32;
                let y = ((frame.logical.y - min_y) as f64 * scale).round() as u32;
                let (w, h) = if (scale_of(image, frame) - scale).abs() < 1e-9 {
                    (image.width, image.height)
                } else {
                    (
                        (frame.logical.width as f64 * scale).round() as u32,
                        (frame.logical.height as f64 * scale).round() as u32,
                    )
                };
                (image, x, y, w, h)
            })
            .collect();
        let width = placed.iter().map(|p| p.1 + p.3).max().unwrap_or(0);
        let height = placed.iter().map(|p| p.2 + p.4).max().unwrap_or(0);

        let mut canvas = Image::new(width, height, depth);
        for (image, x0, y0, w, h) in placed {
            let widen = image.depth == Depth::Eight && depth == Depth::Sixteen;
            for y in 0..h {
                let sy = (y as u64 * image.height as u64 / h as u64) as u32;
                for x in 0..w {
                    let sx = (x as u64 * image.width as u64 / w as u64) as u32;
                    let s = image.index(sx, sy);
                    let mut rgb = [image.samples[s], image.samples[s + 1], image.samples[s + 2]];
                    if widen {
                        rgb = rgb.map(|v| (v << 8) | v);
                    }
                    let d = canvas.index(x0 + x, y0 + y);
                    canvas.samples[d..d + 3].copy_from_slice(&rgb);
                }
            }
        }
        canvas
    }

    /// Encode as an RGB PNG: 8-bit, or 16-bit for 10-bit sources, with the
    /// fast compression the clipboard wants.
    #[allow(dead_code, reason = "the clipboard copy (#60) encodes with it")]
    pub fn write_png(&self, out: impl std::io::Write) -> Result<(), png::EncodingError> {
        self.write_png_with(out, PngLevel::Fast)
    }

    /// [`Image::write_png`] at a chosen `screenshot.png.level`.
    pub fn write_png_with(
        &self,
        out: impl std::io::Write,
        level: PngLevel,
    ) -> Result<(), png::EncodingError> {
        let mut encoder = png::Encoder::new(out, self.width, self.height);
        encoder.set_color(png::ColorType::Rgb);
        match level {
            PngLevel::Fast => encoder.set_compression(png::Compression::Fast),
            PngLevel::Best => encoder.set_compression(png::Compression::High),
            PngLevel::Level(0) => encoder.set_compression(png::Compression::NoCompression),
            PngLevel::Level(n) => {
                encoder.set_compression(png::Compression::Balanced);
                encoder.set_deflate_compression(png::DeflateCompression::Level(n));
            }
        }
        let bytes: Vec<u8> = match self.depth {
            Depth::Eight => {
                encoder.set_depth(png::BitDepth::Eight);
                self.samples.iter().map(|&v| v as u8).collect()
            }
            Depth::Sixteen => {
                encoder.set_depth(png::BitDepth::Sixteen);
                self.samples.iter().flat_map(|v| v.to_be_bytes()).collect()
            }
        };
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&bytes)?;
        writer.finish()
    }

    /// Packed 8-bit RGB; 16-bit samples keep their top 8 bits.
    pub fn rgb8(&self) -> Vec<u8> {
        match self.depth {
            Depth::Eight => self.samples.iter().map(|&v| v as u8).collect(),
            Depth::Sixteen => self.samples.iter().map(|&v| (v >> 8) as u8).collect(),
        }
    }
}

/// Where buffer pixel `(bx, by)` of a `w`×`h` buffer lands once upright:
/// rotate clockwise by the angle, then mirror horizontally for `Flipped*`
/// (the compositor flipped, then rotated counter-clockwise).
fn upright_position(transform: Transform, bx: u32, by: u32, w: u32, h: u32) -> (u32, u32) {
    let (x, y, upright_width) = match transform {
        Transform::Normal | Transform::Flipped => (bx, by, w),
        Transform::Rotate90 | Transform::Flipped90 => (h - 1 - by, bx, h),
        Transform::Rotate180 | Transform::Flipped180 => (w - 1 - bx, h - 1 - by, w),
        Transform::Rotate270 | Transform::Flipped270 => (by, w - 1 - bx, h),
    };
    let flipped = matches!(
        transform,
        Transform::Flipped | Transform::Flipped90 | Transform::Flipped180 | Transform::Flipped270
    );
    if flipped {
        (upright_width - 1 - x, y)
    } else {
        (x, y)
    }
}
