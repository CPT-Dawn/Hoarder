//! `hoarder pick`: freeze, show the Overlay with the Loupe always up, and
//! copy the colour of the clicked pixel (PLAN.md §4.3).

use std::time::Instant;

use hoarder_config::{Config, PickFormat};
use hoarder_ui::{Options, Outcome, Theme, overlay};
use hoarder_wayland::FrozenFrame;

use crate::Exit;

/// A picked colour, exactly as the Frozen Frame holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Picked {
    /// Red, green and blue at `bits` per channel.
    pub rgb: [u16; 3],
    /// 8, or 10 for a 10-bit output.
    pub bits: u32,
}

impl Picked {
    /// Upright physical pixel `(x, y)` of `frame`.
    fn of(frame: &FrozenFrame, x: u32, y: u32) -> Picked {
        Picked {
            rgb: frame.format.rgb(frame.upright_pixel(x, y)),
            bits: frame.format.bits_per_channel(),
        }
    }

    fn max(self) -> u32 {
        (1 << self.bits) - 1
    }

    /// As 8-bit RGB, rounded the way a display shows a 10-bit value.
    pub fn rgb8(self) -> [u8; 3] {
        let max = self.max();
        self.rgb.map(|v| ((v as u32 * 255 + max / 2) / max) as u8)
    }

    /// The colour written as `format`. A 10-bit colour keeps every bit:
    /// `#rrrgggbbb` (three hex digits a channel), `rgb10(r, g, b)`, and HSL
    /// to a tenth.
    pub fn format(self, format: PickFormat) -> String {
        let [r, g, b] = self.rgb;
        let ten = self.bits == 10;
        match format {
            PickFormat::Hex if ten => format!("#{r:03x}{g:03x}{b:03x}"),
            PickFormat::Hex => format!("#{r:02x}{g:02x}{b:02x}"),
            PickFormat::Rgb if ten => format!("rgb10({r}, {g}, {b})"),
            PickFormat::Rgb => format!("rgb({r}, {g}, {b})"),
            PickFormat::Hsl => {
                let (h, s, l) = self.hsl();
                if ten {
                    format!("hsl({h:.1}, {s:.1}%, {l:.1}%)")
                } else {
                    format!("hsl({h:.0}, {s:.0}%, {l:.0}%)")
                }
            }
        }
    }

    /// Hue in degrees, saturation and lightness in percent.
    fn hsl(self) -> (f64, f64, f64) {
        let max = self.max() as f64;
        let [r, g, b] = self.rgb.map(|v| v as f64 / max);
        let hi = r.max(g).max(b);
        let lo = r.min(g).min(b);
        let l = (hi + lo) / 2.0;
        let d = hi - lo;
        if d == 0.0 {
            return (0.0, 0.0, l * 100.0);
        }
        let s = d / (1.0 - (2.0 * l - 1.0).abs());
        let h = if hi == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if hi == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        (h * 60.0, s * 100.0, l * 100.0)
    }
}

/// Pick a colour off the frozen desktop and copy it in `pick.format`.
pub fn run(config: &Config) -> Result<(), (Exit, String)> {
    let started = Instant::now();
    let frozen = crate::shot::freeze()?;
    let options = Options {
        theme: Theme::load(&config.theme),
        pick: true,
        ..Options::default()
    };
    let outcome = overlay::run(&frozen, started, &options).map_err(crate::shot::overlay_error)?;
    let Outcome::Pixel { output, x, y } = outcome else {
        return Err((Exit::Cancelled, "cancelled".into()));
    };
    let frame = frozen
        .frames
        .iter()
        .find(|f| f.output == output)
        .ok_or_else(|| (Exit::Io, format!("output {output} went away")))?;
    let picked = Picked::of(frame, x, y);
    let value = picked.format(config.pick.format);
    crate::clipboard::copy_text(&value)?;
    println!("{value}");
    crate::notify::swatch(&value, picked.rgb8());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_10bit_colour_in_hsl_keeps_a_tenth() {
        // Pure 10-bit blue at half strength: hue 240°, saturated, l = 511/2046.
        let blue = Picked {
            rgb: [0, 0, 511],
            bits: 10,
        };
        assert_eq!(blue.format(PickFormat::Hsl), "hsl(240.0, 100.0%, 25.0%)");
        assert_eq!(blue.rgb8(), [0, 0, 127]);
    }
}
