//! The Loupe (PLAN.md §4.2): an 8× grid of the 15×15 Frozen Frame pixels
//! around the pointer, a crosshair on the pixel under it, and its
//! coordinates and colour. Each cell shows its pixel's exact colour.

use hoarder_wayland::FrozenFrame;

use crate::chrome::{Fonts, argb};
use crate::geom::Rect;
use crate::theme::Theme;

/// Cells per side; the pointer's pixel is the middle one.
pub(crate) const CELLS: u32 = 15;
/// Logical pixels per cell.
const CELL: f64 = 8.0;
/// Logical distance from the pointer to the Loupe's nearest corner.
pub(crate) const OFFSET: i32 = 24;

/// A rendered Loupe: premultiplied `Argb8888`, `4 * size.0` bytes per row.
#[derive(Debug, Clone)]
pub(crate) struct Picture {
    /// Physical pixels.
    pub size: (u32, u32),
    pub pixels: Vec<u8>,
}

/// The colour of a Frozen Frame pixel as 8-bit RGB, rounded like a display
/// would show a 10-bit value.
pub(crate) fn rgb8(frame: &FrozenFrame, x: u32, y: u32) -> [u8; 3] {
    let rgb = frame.format.rgb(frame.upright_pixel(x, y));
    if frame.format.bits_per_channel() == 10 {
        rgb.map(|v| ((v as u32 * 255 + 511) / 1023) as u8)
    } else {
        rgb.map(|v| v as u8)
    }
}

/// The text under the grid: physical coordinates, then the exact colour
/// (`#rrggbb`, or the 10-bit values of a 10-bit output).
pub(crate) fn caption(frame: &FrozenFrame, x: u32, y: u32) -> String {
    let [r, g, b] = frame.format.rgb(frame.upright_pixel(x, y));
    if frame.format.bits_per_channel() == 10 {
        format!("{x}, {y}  rgb10({r}, {g}, {b})")
    } else {
        format!("{x}, {y}  #{r:02x}{g:02x}{b:02x}")
    }
}

/// Draw the Loupe for physical pixel `(px, py)` of `frame` at `scale`
/// physical pixels per logical pixel: framed in the theme's surface (made
/// opaque), the crosshair in its text colour, cells past the output's edge
/// in its muted colour, the caption in its mono font.
pub(crate) fn render(
    frame: &FrozenFrame,
    (px, py): (u32, u32),
    scale: f64,
    fonts: &mut Fonts,
    theme: &Theme,
) -> Picture {
    let opaque = |c: hoarder_config::Colour| argb(hoarder_config::Colour { a: 0xff, ..c });
    let (frame_colour, crosshair, outside) = (
        opaque(theme.surface),
        opaque(theme.text),
        opaque(theme.muted),
    );
    let caption_theme = Theme {
        font: theme.mono_font.clone(),
        ..theme.clone()
    };
    let (fw, fh) = frame.upright_size();
    let cell = (CELL * scale).round().max(1.0) as u32;
    let border = scale.round().max(1.0) as u32;
    let grid = CELLS * cell;
    let side = grid + 2 * border;
    let mut label = fonts.label(&caption(frame, px, py), scale, 0, 0, &caption_theme);
    let gap = (4.0 * scale).round() as u32;
    let width = side.max(label.rect.w as u32);
    let height = side + gap + label.rect.h as u32;
    let mut pixels = vec![0u8; (width * height * 4) as usize];

    let mut fill = |r: Rect, colour: [u8; 4]| {
        let r = r.intersect(&Rect::new(0, 0, width as i32, height as i32));
        for y in r.y..r.bottom() {
            let row = (y as u32 * width) as usize * 4;
            pixels[row + r.x as usize * 4..row + r.right() as usize * 4]
                .as_chunks_mut::<4>()
                .0
                .fill(colour);
        }
    };
    fill(Rect::new(0, 0, side as i32, side as i32), frame_colour);
    let half = (CELLS / 2) as i64;
    for cy in 0..CELLS {
        for cx in 0..CELLS {
            let (sx, sy) = (px as i64 + cx as i64 - half, py as i64 + cy as i64 - half);
            let colour = if sx < 0 || sy < 0 || sx >= fw as i64 || sy >= fh as i64 {
                outside
            } else {
                let [r, g, b] = rgb8(frame, sx as u32, sy as u32);
                [b, g, r, 0xff]
            };
            let x = (border + cx * cell) as i32;
            let y = (border + cy * cell) as i32;
            fill(Rect::new(x, y, cell as i32, cell as i32), colour);
        }
    }
    // The crosshair: a ring just outside the middle cell, so the cell itself
    // keeps its exact colour.
    let mid = (border + (CELLS / 2) * cell) as i32;
    let b = border as i32;
    // Two rings, light inside dark, so it shows on any colour.
    let middle = Rect::new(mid, mid, cell as i32, cell as i32);
    for (grow, colour) in [(2 * b, frame_colour), (b, crosshair)] {
        let ring = middle.inflate(grow);
        for r in [
            Rect::new(ring.x, ring.y, ring.w, b),
            Rect::new(ring.x, ring.bottom() - b, ring.w, b),
            Rect::new(ring.x, ring.y, b, ring.h),
            Rect::new(ring.right() - b, ring.y, b, ring.h),
        ] {
            fill(r, colour);
        }
    }
    label.rect.x = 0;
    label.rect.y = (side + gap) as i32;
    label.blend_into(
        &mut pixels,
        width,
        Rect::new(0, 0, width as i32, height as i32),
    );
    Picture {
        size: (width, height),
        pixels,
    }
}

/// Where a Loupe `size` logical pixels big goes for a pointer at `at`, on an
/// output `output` logical pixels big: below and right of the pointer, or
/// flipped to whichever side has room.
pub(crate) fn place(at: (f64, f64), size: (i32, i32), output: (f64, f64)) -> (i32, i32) {
    let along = |p: f64, len: i32, limit: f64| {
        let p = p.floor() as i32;
        let after = p + OFFSET;
        if (after + len) as f64 <= limit {
            after
        } else {
            (p - OFFSET - len).max(0)
        }
    };
    (along(at.0, size.0, output.0), along(at.1, size.1, output.1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoarder_wayland::{LogicalRect, ShmFormat, Transform};

    fn frame(format: ShmFormat, pixel: u32) -> FrozenFrame {
        FrozenFrame {
            output: "T".into(),
            logical: LogicalRect {
                x: 0,
                y: 0,
                width: 4,
                height: 2,
            },
            format,
            width: 4,
            height: 2,
            stride: 16,
            transform: Transform::Normal,
            y_invert: false,
            hdr: false,
            data: pixel.to_le_bytes().repeat(8),
            cursor: None,
        }
    }

    #[test]
    fn the_caption_names_the_exact_colour() {
        let f = frame(ShmFormat::Xrgb8888, 0x00a1_b2c3);
        assert_eq!(caption(&f, 3, 1), "3, 1  #a1b2c3");
        let ten = frame(ShmFormat::Xrgb2101010, (1023 << 20) | (512 << 10) | 1);
        assert_eq!(caption(&ten, 0, 0), "0, 0  rgb10(1023, 512, 1)");
        assert_eq!(rgb8(&ten, 0, 0), [255, 128, 0]);
    }

    #[test]
    fn the_loupe_flips_to_the_side_with_room() {
        assert_eq!(
            place((100.0, 100.0), (122, 150), (640.0, 480.0)),
            (124, 124)
        );
        assert_eq!(
            place((600.0, 400.0), (122, 150), (640.0, 480.0)),
            (454, 226)
        );
    }
}
