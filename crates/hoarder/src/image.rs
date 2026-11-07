//! Pixels after the freeze: Frozen Frames turned upright without resampling,
//! laid out as one image, and encoded (PLAN.md §4.1 steps 3, 5 and 7).

use hoarder_config::PngLevel;
use hoarder_ui::Region;
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

    /// `frames` as one image, each output at its place in the layout.
    ///
    /// Outputs at the highest scale among `frames` are copied pixel for pixel.
    /// Outputs at a lower scale are enlarged by nearest neighbour
    /// (`capture.mixed_scale = "upscale"`, PLAN.md §4.4), so no pixel is
    /// ever blended.
    pub fn all_outputs(frames: &[&FrozenFrame]) -> Image {
        let parts: Vec<(Image, &FrozenFrame)> =
            frames.iter().map(|&f| (Image::upright(f), f)).collect();
        if let [(image, _)] = parts.as_slice() {
            return image.clone();
        }
        let Layout {
            scale,
            min_x,
            min_y,
        } = Layout::of(frames);
        let depth = if parts.iter().any(|(i, _)| i.depth == Depth::Sixteen) {
            Depth::Sixteen
        } else {
            Depth::Eight
        };

        let placed: Vec<(Image, u32, u32)> = parts
            .into_iter()
            .map(|(image, frame)| {
                let x = ((frame.logical.x - min_x) as f64 * scale).round() as u32;
                let y = ((frame.logical.y - min_y) as f64 * scale).round() as u32;
                let image = if (scale_of(frame) - scale).abs() < 1e-9 {
                    image
                } else {
                    image.upscaled(
                        (frame.logical.width as f64 * scale).round() as u32,
                        (frame.logical.height as f64 * scale).round() as u32,
                    )
                };
                (image, x, y)
            })
            .collect();
        let width = placed.iter().map(|p| p.1 + p.0.width).max().unwrap_or(0);
        let height = placed.iter().map(|p| p.2 + p.0.height).max().unwrap_or(0);

        let mut canvas = Image::new(width, height, depth);
        for (image, x0, y0) in placed {
            let widen = image.depth == Depth::Eight && depth == Depth::Sixteen;
            for y in 0..image.height {
                let s = image.index(0, y);
                let d = canvas.index(x0, y0 + y);
                let row = &image.samples[s..s + image.width as usize * 3];
                let dest = &mut canvas.samples[d..d + row.len()];
                if widen {
                    for (d, &v) in dest.iter_mut().zip(row) {
                        *d = (v << 8) | v;
                    }
                } else {
                    dest.copy_from_slice(row);
                }
            }
        }
        canvas
    }

    /// The Region cut from `frames` laid out as by
    /// [`all_outputs`](Self::all_outputs): every pixel exactly as frozen,
    /// at the highest scale among `frames`.
    pub fn region(frames: &[&FrozenFrame], region: &Region) -> Image {
        let all = Image::all_outputs(frames);
        let Layout {
            scale,
            min_x,
            min_y,
        } = Layout::of(frames);
        let to_px = |v: f64, min: i32, limit: u32| {
            ((v - min as f64) * scale).round().clamp(0.0, limit as f64) as u32
        };
        let x0 = to_px(region.x, min_x, all.width);
        let y0 = to_px(region.y, min_y, all.height);
        let x1 = to_px(region.x + region.width, min_x, all.width);
        let y1 = to_px(region.y + region.height, min_y, all.height);
        all.crop(x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
    }

    /// Enlarged to `width`×`height` by nearest neighbour: every pixel is a
    /// copy of a source pixel, none is blended. Also for buffers that come
    /// at a lower scale than their output, such as a toplevel on Sway at a
    /// fractional scale.
    pub fn upscaled(&self, width: u32, height: u32) -> Image {
        let mut out = Image::new(width, height, self.depth);
        for y in 0..height {
            let sy = (y as u64 * self.height as u64 / height.max(1) as u64) as u32;
            for x in 0..width {
                let sx = (x as u64 * self.width as u64 / width.max(1) as u64) as u32;
                let s = self.index(sx, sy);
                let d = out.index(x, y);
                out.samples[d..d + 3].copy_from_slice(&self.samples[s..s + 3]);
            }
        }
        out
    }

    /// The same image at 8 bits per channel, each sample rounded to the
    /// nearest 8-bit value (`capture.depth = 8`).
    pub fn into_eight_bit(self) -> Image {
        match self.depth {
            Depth::Eight => self,
            Depth::Sixteen => Image {
                depth: Depth::Eight,
                samples: self.samples.iter().map(|&v| narrow(v)).collect(),
                ..self
            },
        }
    }

    fn crop(&self, x0: u32, y0: u32, width: u32, height: u32) -> Image {
        let mut out = Image::new(width, height, self.depth);
        let row = width as usize * 3;
        for y in 0..height {
            let s = self.index(x0, y0 + y);
            let d = out.index(0, y);
            out.samples[d..d + row].copy_from_slice(&self.samples[s..s + row]);
        }
        out
    }

    /// Encode as an RGB PNG: 8-bit, or 16-bit for 10-bit sources, with the
    /// fast compression the clipboard wants.
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

    /// Packed 8-bit RGB; 16-bit samples are rounded to the nearest 8-bit value.
    pub fn rgb8(&self) -> Vec<u8> {
        match self.depth {
            Depth::Eight => self.samples.iter().map(|&v| v as u8).collect(),
            Depth::Sixteen => self.samples.iter().map(|&v| narrow(v) as u8).collect(),
        }
    }
}

/// A 16-bit sample rounded to the nearest 8-bit value.
fn narrow(v: u16) -> u16 {
    ((v as u32 * 255 + 32767) / 65535) as u16
}

/// Physical pixels per logical pixel of `frame`'s output.
pub fn scale_of(frame: &FrozenFrame) -> f64 {
    frame.upright_size().0 as f64 / frame.logical.width.max(1) as f64
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

/// How All Outputs map onto one image: logical layout coordinates times the
/// highest output scale, with the top-left output corner at the origin.
#[derive(Debug, Clone, Copy)]
struct Layout {
    scale: f64,
    min_x: i32,
    min_y: i32,
}

impl Layout {
    fn of(frames: &[&FrozenFrame]) -> Layout {
        let scale = frames.iter().map(|f| scale_of(f)).fold(0.0_f64, f64::max);
        Layout {
            scale,
            min_x: frames.iter().map(|f| f.logical.x).min().unwrap_or(0),
            min_y: frames.iter().map(|f| f.logical.y).min().unwrap_or(0),
        }
    }
}
