//! Known patterns the helper client can paint, and the pixel formats it paints them in.

/// An 8-bit-per-channel colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb8(pub u8, pub u8, pub u8);

/// A 10-bit-per-channel colour (each channel `0..=1023`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb10(pub u16, pub u16, pub u16);

impl From<Rgb8> for Rgb10 {
    /// Widen by bit replication, so 0 and 255 map to 0 and 1023 exactly.
    fn from(Rgb8(r, g, b): Rgb8) -> Self {
        let w = |v: u8| ((v as u16) << 2) | ((v as u16) >> 6);
        Rgb10(w(r), w(g), w(b))
    }
}

/// What to paint over a whole output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    /// One colour everywhere.
    Solid(Rgb10),
    /// Four quadrants, split at the middle physical pixel:
    /// red top-left, green top-right, blue bottom-left, white bottom-right.
    Quadrants,
    /// A black-and-white checkerboard of `cell`×`cell` physical pixels,
    /// white in the top-left cell. With `cell: 1` any resampling shows.
    Checker { cell: u32 },
    /// Every 10-bit value of every channel, row by row: pixel `i` (counting
    /// left to right, top to bottom) is `(i, 7i + 3, 1023 - i)`, each mod 1024.
    /// Needs at least 1024 pixels to cover them all.
    Ramp,
}

impl Pattern {
    /// A solid 8-bit colour.
    pub fn solid(c: Rgb8) -> Self {
        Pattern::Solid(c.into())
    }

    /// The colour at physical pixel `(x, y)` of a `w`×`h` surface.
    pub fn colour_at(&self, x: u32, y: u32, w: u32, h: u32) -> Rgb10 {
        match self {
            Pattern::Solid(c) => *c,
            Pattern::Quadrants => {
                let c = match (x >= w / 2, y >= h / 2) {
                    (false, false) => Rgb8(255, 0, 0),
                    (true, false) => Rgb8(0, 255, 0),
                    (false, true) => Rgb8(0, 0, 255),
                    (true, true) => Rgb8(255, 255, 255),
                };
                c.into()
            }
            Pattern::Checker { cell } => {
                let cell = (*cell).max(1);
                if (x / cell + y / cell).is_multiple_of(2) {
                    Rgb8(255, 255, 255).into()
                } else {
                    Rgb8(0, 0, 0).into()
                }
            }
            Pattern::Ramp => {
                let i = y as u64 * w as u64 + x as u64;
                let v = |n: u64| (n % 1024) as u16;
                Rgb10(v(i), v(7 * i + 3), 1023 - v(i))
            }
        }
    }
}

/// Pixel format of the buffers the helper client attaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// `wl_shm` `xrgb8888`.
    Xrgb8888,
    /// `wl_shm` `xrgb2101010`.
    Xrgb2101010,
}

impl PixelFormat {
    /// Encode one pixel as the little-endian `u32` both formats use.
    pub(crate) fn encode(self, Rgb10(r, g, b): Rgb10) -> u32 {
        match self {
            PixelFormat::Xrgb8888 => {
                let n = |v: u16| ((v as u32 * 255 + 511) / 1023) & 0xff;
                (n(r) << 16) | (n(g) << 8) | n(b)
            }
            PixelFormat::Xrgb2101010 => {
                let n = |v: u16| (v as u32) & 0x3ff;
                (n(r) << 20) | (n(g) << 10) | n(b)
            }
        }
    }

    /// Render `pattern` into a tightly packed `w`×`h` buffer (stride `4 * w`).
    pub(crate) fn render(self, pattern: &Pattern, w: u32, h: u32) -> Vec<u8> {
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                buf.extend_from_slice(&self.encode(pattern.colour_at(x, y, w, h)).to_le_bytes());
            }
        }
        buf
    }
}
