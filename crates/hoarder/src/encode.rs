//! Encoding a Capture in the saved file's format (PLAN.md §4.5).
//!
//! PNG keeps 16 bits per channel for 10-bit sources; the other formats are
//! 8-bit. PNG, lossless WebP and QOI are byte-exact.

use std::io::Write;

use hoarder_config::{Format, Screenshot};
use image::{ExtendedColorType, ImageEncoder};

use crate::image::{Depth, Image};

/// Encode `image` as `format`, with that format's options from `settings`.
pub fn encode(
    image: &Image,
    format: Format,
    settings: &Screenshot,
    mut out: impl Write,
) -> Result<(), String> {
    let (w, h) = (image.width, image.height);
    let failed = |e: &dyn std::fmt::Display| format!("cannot encode {}: {e}", format.extension());
    if format == Format::Png {
        return image
            .write_png_with(out, settings.png.level)
            .map_err(|e| failed(&e));
    }
    let rgb = image.rgb8();
    let rgb8 = ExtendedColorType::Rgb8;
    let result = match format {
        Format::Png => unreachable!("handled above"),
        Format::Webp if settings.webp.lossless => {
            image::codecs::webp::WebPEncoder::new_lossless(out).write_image(&rgb, w, h, rgb8)
        }
        Format::Webp => {
            // `image` only encodes lossless WebP; libwebp does the lossy kind.
            let quality = settings.webp.quality.get() as f32;
            let encoded = webp::Encoder::from_rgb(&rgb, w, h).encode(quality);
            return out.write_all(&encoded).map_err(|e| failed(&e));
        }
        Format::Jpeg => image::codecs::jpeg::JpegEncoder::new_with_quality(
            &mut out,
            settings.jpeg.quality.get() as u8,
        )
        .write_image(&rgb, w, h, rgb8),
        Format::Avif => image::codecs::avif::AvifEncoder::new_with_speed_quality(
            out,
            settings.avif.speed.get() as u8,
            settings.avif.quality.get() as u8,
        )
        .write_image(&rgb, w, h, rgb8),
        Format::Qoi => image::codecs::qoi::QoiEncoder::new(out).write_image(&rgb, w, h, rgb8),
    };
    result.map_err(|e| failed(&e))
}

/// Whether saving `image` as `format` drops bits: only PNG holds 16 bits.
pub fn narrows(image: &Image, format: Format) -> bool {
    image.depth == Depth::Sixteen && format != Format::Png
}
