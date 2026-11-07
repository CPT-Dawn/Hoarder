//! Drawing the Picker (PLAN.md §6): a rounded panel with the search field
//! on top, the matching Clips on the left, the selected one's preview on
//! the right and a key hint at the bottom.

use cosmic_text::{
    Attrs, Buffer, Color, Ellipsize, EllipsizeHeightLimit, Family, FontSystem, Metrics, Shaping,
    SwashCache, Wrap,
};
use tiny_skia::{FillRule, Paint, Pixmap, PixmapPaint, Rect, Transform};

use super::model::{MASK, Model};
use super::{Detail, Kind};
use crate::chrome::{Fonts, installed_family, rounded_rect};
use crate::theme::Theme;

/// The panel's size in logical pixels (PLAN.md §6).
pub(crate) const WIDTH: u32 = 720;
pub(crate) const HEIGHT: u32 = 480;

const PAD: f32 = 14.0;
const SEARCH_H: f32 = 52.0;
const FOOTER_H: f32 = 30.0;
const ROW_H: f32 = 34.0;
/// The list's width; the preview pane takes the rest.
const LIST_W: f32 = 360.0;
/// Room for a row's source app and age.
const META_W: f32 = 92.0;
const CHIP_H: f32 = 44.0;
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

/// What the panel shows besides the model: the selected Clip's detail (if
/// it has arrived) and the time now, for the rows' ages.
pub(crate) struct Extra<'a> {
    pub(crate) detail: Option<&'a Detail>,
    pub(crate) now_ms: i64,
    /// How far an image too big for the preview pane is scrolled, in its
    /// own pixels from the top left.
    pub(crate) scroll: (u32, u32),
}

/// The preview pane, in logical pixels.
fn preview_pane() -> Pane {
    let footer_y = HEIGHT as f32 - FOOTER_H;
    Pane {
        x: LIST_W + PAD,
        y: SEARCH_H + PAD,
        w: WIDTH as f32 - LIST_W - 2.0 * PAD,
        h: footer_y - SEARCH_H - 2.0 * PAD - SMALL * 1.3 - 8.0,
    }
}

/// How far an image `width`×`height` can scroll in the preview pane at
/// `scale`: its size less what shows, in its own pixels.
pub(crate) fn scroll_room(width: u32, height: u32, scale: u32) -> (u32, u32) {
    let pane = preview_pane();
    let s = scale as f32;
    let (pw, ph) = ((pane.w * s) as u32, (pane.h * s) as u32);
    (width.saturating_sub(pw), height.saturating_sub(ph))
}

/// Paint the whole panel into `canvas` (`Argb8888`, `scale` physical
/// pixels per logical pixel).
pub(crate) fn paint(
    canvas: &mut [u8],
    scale: u32,
    model: &mut Model,
    extra: &Extra<'_>,
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
    let mono = installed_family(fonts, &theme.mono_font);
    let mut pen = Pen {
        pixmap: &mut pixmap,
        fonts,
        cache,
        scale: s,
    };

    // The search field, with a caret after the query and the filters on
    // the right.
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
            WIDTH as f32 - 2.0 * PAD - 150.0,
            theme.text,
        )
    };
    pen.text(
        "img: txt: file: pin: app:",
        family,
        WIDTH as f32 - PAD - 150.0,
        (SEARCH_H - SMALL * 1.3) / 2.0,
        SMALL,
        150.0,
        theme.dim,
    );
    fill(
        pen.pixmap,
        &mut paint,
        straight(theme.accent),
        caret_x + 1.0,
        field_y,
        2.0,
        TEXT * 1.3,
    );
    let rule = with_alpha(theme.dim, 128);
    fill(
        pen.pixmap,
        &mut paint,
        rule,
        0.0,
        SEARCH_H - 1.0,
        WIDTH as f32,
        1.0,
    );
    let footer_y = HEIGHT as f32 - FOOTER_H;
    fill(
        pen.pixmap,
        &mut paint,
        rule,
        LIST_W,
        SEARCH_H,
        1.0,
        footer_y - SEARCH_H,
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
            LIST_W - PAD,
            theme.muted,
        );
    }
    let mut selected_entry = None;
    for (n, (clip, selected)) in rows.into_iter().enumerate() {
        let y = SEARCH_H + PAD / 2.0 + n as f32 * ROW_H;
        if selected {
            selected_entry = Some(clip);
            paint.set_color(with_alpha(theme.accent, 48));
            let row = rounded_rect(
                (LIST_W - PAD) * s,
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
        let tag_colour = if clip.pinned {
            theme.accent
        } else {
            theme.muted
        };
        pen.text(
            kind_tag(clip.kind, clip.concealed),
            family,
            PAD + 6.0,
            tag_y,
            SMALL,
            36.0,
            tag_colour,
        );
        let x = PAD + 44.0;
        pen.text(
            model.line(clip),
            family,
            x,
            text_y,
            TEXT,
            LIST_W - META_W - x,
            theme.text,
        );
        pen.text(
            &meta(clip.app.as_deref(), clip.at_ms, extra.now_ms),
            family,
            LIST_W - META_W + 4.0,
            tag_y,
            SMALL,
            META_W - PAD,
            theme.muted,
        );
    }

    // The preview pane.
    if let Some(clip) = selected_entry {
        let pane = preview_pane();
        let detail = extra.detail;
        if clip.concealed {
            match model.revealed(clip.id) {
                Some(text) => pen.block(text, mono, &pane, theme.text),
                None => {
                    pen.text(MASK, family, pane.x, pane.y, TEXT, pane.w, theme.text);
                }
            }
        } else {
            match detail {
                Some(Detail::Text(text)) => {
                    let face = if code_like(text) { mono } else { family };
                    pen.block(text, face, &pane, theme.text);
                }
                Some(Detail::Image {
                    width,
                    height,
                    rgba,
                }) => pen.image(*width, *height, rgba, &pane, extra.scroll),
                Some(Detail::Files(paths)) => {
                    let mut y = pane.y;
                    for path in paths {
                        if y + CHIP_H > pane.y + pane.h {
                            break;
                        }
                        paint.set_color(with_alpha(theme.accent, 40));
                        if let Some(chip) =
                            rounded_rect(pane.w * s, (CHIP_H - 6.0) * s, theme.radius / 2.0 * s)
                        {
                            let at = Transform::from_translate(pane.x * s, y * s);
                            pen.pixmap
                                .fill_path(&chip, &paint, FillRule::Winding, at, None);
                        }
                        let name = path.rsplit('/').find(|p| !p.is_empty()).unwrap_or(path);
                        pen.text(
                            name,
                            family,
                            pane.x + 10.0,
                            y + 3.0,
                            TEXT - 1.0,
                            pane.w - 20.0,
                            theme.text,
                        );
                        pen.text(
                            path,
                            family,
                            pane.x + 10.0,
                            y + 3.0 + (TEXT - 1.0) * 1.3,
                            SMALL - 1.0,
                            pane.w - 20.0,
                            theme.muted,
                        );
                        y += CHIP_H;
                    }
                }
                // Not arrived yet, or nothing to show beyond the preview.
                Some(Detail::Nothing) | None => {
                    pen.block(&clip.preview, family, &pane, theme.text);
                }
            }
        }
        let mut about = meta(clip.app.as_deref(), clip.at_ms, extra.now_ms);
        if clip.pinned {
            about.push_str(" · pinned");
        }
        if clip.concealed {
            about.push_str(match model.revealed(clip.id) {
                Some(_) => " · concealed",
                None => " · concealed, hold Alt to reveal",
            });
        }
        pen.text(
            &about,
            family,
            pane.x,
            footer_y - PAD - SMALL * 1.3,
            SMALL,
            pane.w,
            theme.muted,
        );
    }

    // The key hint.
    fill(
        pen.pixmap,
        &mut paint,
        rule,
        0.0,
        footer_y,
        WIDTH as f32,
        1.0,
    );
    pen.text(
        "Enter restore   Shift+Enter plain   Ctrl+P pin   Del delete   Ctrl+1–9 quick   Esc close",
        family,
        PAD,
        footer_y + (FOOTER_H - SMALL * 1.3) / 2.0,
        SMALL,
        WIDTH as f32 - 2.0 * PAD,
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
fn kind_tag(kind: Kind, concealed: bool) -> &'static str {
    match kind {
        _ if concealed => "HIDE",
        Kind::Text => "TXT",
        Kind::Image => "IMG",
        Kind::Files => "FILE",
        Kind::Other => "BIN",
    }
}

/// `app · age`, the app by the last part of its id (`firefox` for
/// `org.mozilla.firefox`).
fn meta(app: Option<&str>, at_ms: Option<i64>, now_ms: i64) -> String {
    let app = app.map(|a| a.rsplit('.').next().unwrap_or(a));
    let age = at_ms.map(|at| ago(at, now_ms));
    match (app, age) {
        (Some(app), Some(age)) => format!("{app} · {age}"),
        (Some(one), None) => one.to_owned(),
        (None, Some(one)) => one,
        (None, None) => String::new(),
    }
}

/// How long ago `at_ms` was, shortly: `now`, `5m`, `3h`, `2d`.
pub(crate) fn ago(at_ms: i64, now_ms: i64) -> String {
    let secs = (now_ms - at_ms).max(0) / 1000;
    match secs {
        0..60 => "now".into(),
        60..3600 => format!("{}m", secs / 60),
        3600..86400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

/// Whether `text` looks like code, to show it in the monospace font.
fn code_like(text: &str) -> bool {
    text.lines().take(50).any(|l| {
        l.starts_with("    ")
            || l.starts_with('\t')
            || l.trim_end().ends_with(['{', '}', ';'])
            || l.contains("=>")
            || l.contains("::")
    })
}

/// A rectangle of the panel, in logical pixels.
struct Pane {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// Writes text and images onto the panel.
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
        let clip = (x * s, y * s, (x + max_w) * s, y * s + line);
        self.blit(&mut buffer, x, y, colour, clip);
        width / s
    }

    /// Draw `text` wrapped to fill `pane`, cut off at its bottom.
    fn block(
        &mut self,
        text: &str,
        family: Family<'_>,
        pane: &Pane,
        colour: hoarder_config::Colour,
    ) {
        let s = self.scale;
        let line = (TEXT * 1.3 * s).ceil();
        let mut buffer = Buffer::new(self.fonts, Metrics::new(TEXT * s, line));
        buffer.set_wrap(Wrap::WordOrGlyph);
        buffer.set_size(Some(pane.w * s), Some(pane.h * s));
        let text = text.replace('\t', "    ");
        buffer.set_text(&text, &Attrs::new().family(family), Shaping::Advanced, None);
        buffer.shape_until_scroll(self.fonts, false);
        let clip = (
            pane.x * s,
            pane.y * s,
            (pane.x + pane.w) * s,
            (pane.y + pane.h) * s,
        );
        self.blit(&mut buffer, pane.x, pane.y, colour, clip);
    }

    /// Draw `buffer` with its top-left at `(x, y)` (logical), only within
    /// `clip` (physical `x0, y0, x1, y1`).
    fn blit(
        &mut self,
        buffer: &mut Buffer,
        x: f32,
        y: f32,
        colour: hoarder_config::Colour,
        clip: (f32, f32, f32, f32),
    ) {
        let s = self.scale;
        let (ox, oy) = ((x * s).round() as i32, (y * s).round() as i32);
        let (pw, ph) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        let x0 = (clip.0.floor() as i32).max(0);
        let y0 = (clip.1.floor() as i32).max(0);
        let x1 = (clip.2.ceil() as i32).min(pw);
        let y1 = (clip.3.ceil() as i32).min(ph);
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
                    if px < x0 || py < y0 || px >= x1 || py >= y1 {
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
    }

    /// Draw an image (straight RGBA) at its real pixel size, one image
    /// pixel per physical pixel: centred in `pane` if it fits, otherwise
    /// its top-left corner, cut to the pane.
    /// `rgba` at its real pixel size, centred in `pane`; one bigger than
    /// the pane shows the part `scroll` (in its own pixels) brings into view.
    fn image(&mut self, width: u32, height: u32, rgba: &[u8], pane: &Pane, scroll: (u32, u32)) {
        let s = self.scale;
        let (px, py) = ((pane.x * s).round() as i32, (pane.y * s).round() as i32);
        let (pw, ph) = ((pane.w * s) as u32, (pane.h * s) as u32);
        let (cw, ch) = (width.min(pw), height.min(ph));
        if cw == 0 || ch == 0 || rgba.len() < (width * height * 4) as usize {
            return;
        }
        let (ox, oy) = (scroll.0.min(width - cw), scroll.1.min(height - ch));
        // Premultiplied, as tiny-skia wants, and cut to what shows.
        let mut data = Vec::with_capacity((cw * ch * 4) as usize);
        for row in oy..oy + ch {
            let start = ((row * width + ox) * 4) as usize;
            for p in rgba[start..start + (cw * 4) as usize].as_chunks::<4>().0 {
                let a = p[3] as u32;
                let pre = |v: u8| (v as u32 * a / 255) as u8;
                data.extend_from_slice(&[pre(p[0]), pre(p[1]), pre(p[2]), p[3]]);
            }
        }
        let Some(size) = tiny_skia::IntSize::from_wh(cw, ch) else {
            return;
        };
        let Some(image) = Pixmap::from_vec(data, size) else {
            return;
        };
        let x = px + ((pw - cw) / 2) as i32;
        let y = py + ((ph - ch) / 2) as i32;
        self.pixmap.draw_pixmap(
            x,
            y,
            image.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_read_shortly() {
        let now = 10_000_000_000;
        assert_eq!(ago(now - 59_000, now), "now");
        assert_eq!(ago(now - 5 * 60_000, now), "5m");
        assert_eq!(ago(now - 3 * 3_600_000, now), "3h");
        assert_eq!(ago(now - 2 * 86_400_000, now), "2d");
        // A clock that went backwards is not the future.
        assert_eq!(ago(now + 5_000, now), "now");
    }

    #[test]
    fn a_row_names_its_app_by_the_last_part_of_its_id() {
        let now = 10_000_000_000;
        assert_eq!(
            meta(Some("org.mozilla.firefox"), Some(now - 120_000), now),
            "firefox · 2m"
        );
        assert_eq!(meta(None, Some(now), now), "now");
        assert_eq!(meta(Some("kitty"), None, now), "kitty");
    }

    #[test]
    fn code_reads_as_code() {
        assert!(code_like("fn main() {\n    hoard();\n}"));
        assert!(!code_like("Just a sentence, with a comma."));
    }
}
