//! Drawing the Picker: a rounded panel with the search field on top, the
//! matching Clips below it and a key hint at the bottom.

use cosmic_text::{
    Attrs, Buffer, Color, Ellipsize, EllipsizeHeightLimit, Family, FontSystem, Metrics, Shaping,
    SwashCache, Wrap,
};
use tiny_skia::{FillRule, Paint, Pixmap, Rect, Transform};

use super::model::Model;
use crate::chrome::{Fonts, installed_family, rounded_rect};
use crate::theme::Theme;

/// The panel's size in logical pixels (PLAN.md §6).
pub(crate) const WIDTH: u32 = 720;
pub(crate) const HEIGHT: u32 = 480;

const PAD: f32 = 14.0;
const SEARCH_H: f32 = 52.0;
const FOOTER_H: f32 = 30.0;
const ROW_H: f32 = 34.0;
const TEXT: f32 = 15.0;
const SMALL: f32 = 12.0;

/// How many rows of Clips the panel has room for.
pub(crate) fn rows() -> usize {
    ((HEIGHT as f32 - SEARCH_H - FOOTER_H - PAD) / ROW_H) as usize
}

fn straight(c: hoarder_config::Colour) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(c.r, c.g, c.b, c.a)
}

fn with_alpha(c: hoarder_config::Colour, a: u8) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(c.r, c.g, c.b, (c.a as u32 * a as u32 / 255) as u8)
}

/// Paint the whole panel into `canvas` (`Argb8888`, `scale` physical
/// pixels per logical pixel).
pub(crate) fn paint(
    canvas: &mut [u8],
    scale: u32,
    model: &mut Model,
    theme: &Theme,
    fonts: &mut Fonts,
) {
    let s = scale as f32;
    let (w, h) = (WIDTH * scale, HEIGHT * scale);
    let mut pixmap = Pixmap::new(w, h).expect("non-empty panel");
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    let fill =
        |pixmap: &mut Pixmap, paint: &mut Paint, colour, x: f32, y: f32, rw: f32, rh: f32| {
            paint.set_color(colour);
            if let Some(r) = Rect::from_xywh(x * s, y * s, rw * s, rh * s) {
                pixmap.fill_rect(r, paint, Transform::identity(), None);
            }
        };

    paint.set_color(straight(theme.surface));
    if let Some(panel) = rounded_rect(w as f32, h as f32, theme.radius * s) {
        pixmap.fill_path(
            &panel,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    let (fonts, cache) = fonts.get();
    let family = installed_family(fonts, &theme.font);
    let mut pen = Pen {
        pixmap: &mut pixmap,
        fonts,
        cache,
        scale: s,
    };

    // The search field, with a caret after the query.
    let query = model.query().to_owned();
    let field_y = (SEARCH_H - TEXT * 1.3) / 2.0;
    let caret_x = if query.is_empty() {
        pen.text(
            "Search History…",
            family,
            PAD + 6.0,
            field_y,
            TEXT,
            WIDTH as f32,
            theme.dim,
        );
        PAD
    } else {
        PAD + pen.text(
            &query,
            family,
            PAD,
            field_y,
            TEXT,
            WIDTH as f32 - 2.0 * PAD,
            theme.text,
        )
    };
    fill(
        pen.pixmap,
        &mut paint,
        straight(theme.accent),
        caret_x + 1.0,
        field_y,
        2.0,
        TEXT * 1.3,
    );
    fill(
        pen.pixmap,
        &mut paint,
        with_alpha(theme.dim, 128),
        0.0,
        SEARCH_H - 1.0,
        WIDTH as f32,
        1.0,
    );

    // The Clips.
    let empty = model.is_empty();
    model.scroll(rows());
    let model: &Model = model;
    let rows = model.on_screen(rows());
    if rows.is_empty() {
        let why = if empty {
            "History is empty"
        } else {
            "No Clip matches"
        };
        pen.text(
            why,
            family,
            PAD + 6.0,
            SEARCH_H + PAD,
            TEXT,
            WIDTH as f32,
            theme.muted,
        );
    }
    for (n, (entry, selected)) in rows.into_iter().enumerate() {
        let model_line = model.line(entry);
        let y = SEARCH_H + PAD / 2.0 + n as f32 * ROW_H;
        if selected {
            paint.set_color(with_alpha(theme.accent, 48));
            let row = rounded_rect(
                (WIDTH as f32 - PAD) * s,
                (ROW_H - 2.0) * s,
                theme.radius / 2.0 * s,
            );
            if let Some(row) = row {
                let at = Transform::from_translate(PAD / 2.0 * s, (y + 1.0) * s);
                pen.pixmap
                    .fill_path(&row, &paint, FillRule::Winding, at, None);
            }
            fill(
                pen.pixmap,
                &mut paint,
                straight(theme.accent),
                PAD / 2.0,
                y + 7.0,
                3.0,
                ROW_H - 14.0,
            );
        }
        let text_y = y + (ROW_H - TEXT * 1.3) / 2.0;
        let tag_y = y + (ROW_H - SMALL * 1.3) / 2.0;
        pen.text(
            kind_tag(&entry.kind),
            family,
            PAD + 6.0,
            tag_y,
            SMALL,
            40.0,
            theme.muted,
        );
        let x = PAD + 50.0;
        pen.text(
            model_line,
            family,
            x,
            text_y,
            TEXT,
            WIDTH as f32 - x - PAD,
            theme.text,
        );
    }

    // The key hint.
    let footer_y = HEIGHT as f32 - FOOTER_H;
    fill(
        pen.pixmap,
        &mut paint,
        with_alpha(theme.dim, 128),
        0.0,
        footer_y,
        WIDTH as f32,
        1.0,
    );
    pen.text(
        "Enter restore    Up/Down select    Esc close",
        family,
        PAD,
        footer_y + (FOOTER_H - SMALL * 1.3) / 2.0,
        SMALL,
        WIDTH as f32,
        theme.muted,
    );

    // tiny-skia is RGBA; Argb8888 is BGRA in memory.
    for (dst, src) in canvas
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(pixmap.data().as_chunks::<4>().0)
    {
        *dst = [src[2], src[1], src[0], src[3]];
    }
}

/// A short label for a Clip's kind.
fn kind_tag(kind: &str) -> &'static str {
    match kind {
        "text" => "TXT",
        "image" => "IMG",
        "files" => "FILE",
        _ => "BIN",
    }
}

/// Writes text onto the panel.
struct Pen<'a> {
    pixmap: &'a mut Pixmap,
    fonts: &'a mut FontSystem,
    cache: &'a mut SwashCache,
    scale: f32,
}

impl Pen<'_> {
    /// Draw one line of `text` with its top-left at `(x, y)` (logical),
    /// cut short with an ellipsis past `max_w`. Returns its width.
    #[allow(clippy::too_many_arguments)]
    fn text(
        &mut self,
        text: &str,
        family: Family<'_>,
        x: f32,
        y: f32,
        size: f32,
        max_w: f32,
        colour: hoarder_config::Colour,
    ) -> f32 {
        let s = self.scale;
        let line = (size * 1.3 * s).ceil();
        let mut buffer = Buffer::new(self.fonts, Metrics::new(size * s, line));
        buffer.set_wrap(Wrap::None);
        buffer.set_ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)));
        buffer.set_size(Some(max_w * s), Some(line));
        // One line: newlines and tabs would only confuse it.
        let flat: String = text
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        buffer.set_text(&flat, &Attrs::new().family(family), Shaping::Advanced, None);
        buffer.shape_until_scroll(self.fonts, false);
        let width = buffer
            .layout_runs()
            .map(|r| r.line_w)
            .fold(0.0_f32, f32::max);
        let (ox, oy) = ((x * s).round() as i32, (y * s).round() as i32);
        let (pw, ph) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        let data = self.pixmap.data_mut();
        let colour = Color::rgba(colour.r, colour.g, colour.b, colour.a);
        buffer.draw(self.fonts, self.cache, colour, |gx, gy, gw, gh, c| {
            let a = c.a() as u32;
            if a == 0 {
                return;
            }
            for dy in 0..gh as i32 {
                for dx in 0..gw as i32 {
                    let (px, py) = (ox + gx + dx, oy + gy + dy);
                    if px < 0 || py < 0 || px >= pw || py >= ph {
                        continue;
                    }
                    let i = ((py * pw + px) * 4) as usize;
                    for (k, v) in [c.r(), c.g(), c.b()].into_iter().enumerate() {
                        let v = v as u32 * a / 255;
                        data[i + k] = (v + data[i + k] as u32 * (255 - a) / 255) as u8;
                    }
                    data[i + 3] = (a + data[i + 3] as u32 * (255 - a) / 255) as u8;
                }
            }
        });
        width / s
    }
}
