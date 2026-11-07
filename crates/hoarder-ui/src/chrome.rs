//! The Overlay chrome: everything drawn on top of a Frozen Frame (dimming,
//! selection border, size label), painted into an `Argb8888` buffer in
//! physical pixels and only where it changed.

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache};

use crate::geom::Rect;

/// `Argb8888` pixels as little-endian bytes (premultiplied).
const CLEAR: [u8; 4] = [0, 0, 0, 0];
/// Black at 45% opacity: what the Overlay dims the unselected frame with.
pub(crate) const DIM: [u8; 4] = [0, 0, 0, 115];
/// The accent colour of the selection border, #3d8eff.
const ACCENT: [u8; 4] = [0xff, 0x8e, 0x3d, 0xff];

/// What one output's chrome shows, in that output's physical pixels.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
    /// The selected pixels: shown undimmed with a border just outside.
    pub selection: Option<Rect>,
    pub label: Option<Label>,
    /// Refine's resize handles, filled with the accent colour.
    pub handles: Vec<Rect>,
}

impl Scene {
    /// Every pixel that differs between `self` and `next` lies in one of these.
    pub fn damage(&self, next: &Scene) -> Vec<Rect> {
        let mut out = Vec::new();
        match (self.selection, next.selection) {
            (a, b) if a == b => {}
            (Some(a), Some(b)) => {
                out.extend(a.subtract(&b));
                out.extend(b.subtract(&a));
                out.extend(a.ring());
                out.extend(b.ring());
            }
            (Some(r), None) | (None, Some(r)) => out.push(r.inflate(1)),
            (None, None) => {}
        }
        if self.label != next.label {
            out.extend(self.label.as_ref().map(|l| l.rect));
            out.extend(next.label.as_ref().map(|l| l.rect));
        }
        if self.handles != next.handles {
            out.extend(&self.handles);
            out.extend(&next.handles);
        }
        out.retain(|r| !r.is_empty());
        out
    }

    /// Repaint `area` of `canvas` (`width` pixels per row, stride `4 * width`).
    pub fn paint(&self, canvas: &mut [u8], width: u32, height: u32, area: Rect) {
        let area = area.intersect(&Rect::new(0, 0, width as i32, height as i32));
        if area.is_empty() {
            return;
        }
        let stride = width as usize * 4;
        for y in area.y..area.bottom() {
            let row = &mut canvas[y as usize * stride..(y as usize + 1) * stride];
            for (x0, x1, colour) in self.row_spans(y) {
                let (x0, x1) = (x0.max(area.x), x1.min(area.right()));
                if x0 < x1 {
                    row[x0 as usize * 4..x1 as usize * 4]
                        .as_chunks_mut::<4>()
                        .0
                        .fill(colour);
                }
            }
        }
        for handle in &self.handles {
            let r = handle.intersect(&area);
            for y in r.y..r.bottom() {
                let row = &mut canvas[y as usize * stride..(y as usize + 1) * stride];
                row[r.x as usize * 4..r.right() as usize * 4]
                    .as_chunks_mut::<4>()
                    .0
                    .fill(ACCENT);
            }
        }
        if let Some(label) = &self.label {
            label.blend_into(canvas, width, area);
        }
    }

    /// Row `y` as `(start, end, colour)` spans covering every x.
    fn row_spans(&self, y: i32) -> Vec<(i32, i32, [u8; 4])> {
        let Some(s) = self.selection else {
            return vec![(i32::MIN, i32::MAX, DIM)];
        };
        let ring = s.inflate(1);
        if y < ring.y || y >= ring.bottom() {
            vec![(i32::MIN, i32::MAX, DIM)]
        } else if y == ring.y || y == ring.bottom() - 1 {
            vec![
                (i32::MIN, ring.x, DIM),
                (ring.x, ring.right(), ACCENT),
                (ring.right(), i32::MAX, DIM),
            ]
        } else {
            vec![
                (i32::MIN, ring.x, DIM),
                (ring.x, s.x, ACCENT),
                (s.x, s.right(), CLEAR),
                (s.right(), ring.right(), ACCENT),
                (ring.right(), i32::MAX, DIM),
            ]
        }
    }
}

/// A rendered size pill, e.g. `1280 × 720`.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub text: String,
    /// Where it sits, in the output's physical pixels.
    pub rect: Rect,
    /// Premultiplied `Argb8888` bytes, `rect.w * 4` per row.
    pixels: Vec<u8>,
}

/// Fonts for labels, loaded off the critical path (a full fontconfig scan
/// takes tens of milliseconds).
#[derive(Debug)]
pub struct Fonts {
    loading: Option<std::thread::JoinHandle<FontSystem>>,
    ready: Option<(FontSystem, SwashCache)>,
}

impl Fonts {
    /// Start loading the system fonts on a background thread.
    pub fn load() -> Fonts {
        Fonts {
            loading: Some(std::thread::spawn(FontSystem::new)),
            ready: None,
        }
    }

    fn get(&mut self) -> &mut (FontSystem, SwashCache) {
        self.ready.get_or_insert_with(|| {
            let fonts = self
                .loading
                .take()
                .and_then(|h| h.join().ok())
                .unwrap_or_else(FontSystem::new);
            (fonts, SwashCache::new())
        })
    }

    /// Render `text` as a pill at `scale` physical pixels per logical pixel,
    /// with its top-left corner at `(x, y)`.
    pub fn label(&mut self, text: &str, scale: f64, x: i32, y: i32) -> Label {
        let (fonts, cache) = self.get();
        let px = |v: f64| (v * scale).round().max(1.0) as f32;
        let (size, pad_x, pad_y) = (px(13.0), px(6.0), px(3.0));
        let mut buffer = Buffer::new(fonts, Metrics::new(size, (size * 1.3).ceil()));
        buffer.set_size(None, None);
        buffer.set_text(
            text,
            &Attrs::new().family(Family::SansSerif),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(fonts, false);
        let text_w = buffer
            .layout_runs()
            .map(|r| r.line_w)
            .fold(0.0_f32, f32::max)
            .ceil();
        let w = (text_w + 2.0 * pad_x) as u32;
        let h = ((size * 1.3).ceil() + 2.0 * pad_y) as u32;
        let mut pixmap = tiny_skia::Pixmap::new(w.max(1), h.max(1)).expect("non-empty label");
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(20, 20, 24, 217);
        paint.anti_alias = true;
        if let Some(path) = rounded_rect(w as f32, h as f32, pad_y * 1.5) {
            pixmap.fill_path(
                &path,
                &paint,
                tiny_skia::FillRule::Winding,
                tiny_skia::Transform::identity(),
                None,
            );
        }
        let (pw, ph) = (pixmap.width() as i32, pixmap.height() as i32);
        let data = pixmap.data_mut();
        buffer.draw(
            fonts,
            cache,
            Color::rgb(255, 255, 255),
            |gx, gy, gw, gh, c| {
                for dy in 0..gh as i32 {
                    for dx in 0..gw as i32 {
                        let (x, y) = (gx + dx + pad_x as i32, gy + dy + pad_y as i32);
                        if x < 0 || y < 0 || x >= pw || y >= ph {
                            continue;
                        }
                        let i = ((y * pw + x) * 4) as usize;
                        let a = c.a() as u32;
                        for (k, v) in [c.r(), c.g(), c.b()].into_iter().enumerate() {
                            let v = v as u32 * a / 255;
                            data[i + k] = (v + data[i + k] as u32 * (255 - a) / 255) as u8;
                        }
                        data[i + 3] = (a + data[i + 3] as u32 * (255 - a) / 255) as u8;
                    }
                }
            },
        );
        // tiny-skia is RGBA; Argb8888 is BGRA in memory.
        let mut pixels = pixmap.take();
        for p in pixels.as_chunks_mut::<4>().0 {
            p.swap(0, 2);
        }
        Label {
            text: text.to_owned(),
            rect: Rect::new(x, y, w as i32, h as i32),
            pixels,
        }
    }
}

impl Label {
    /// Blend over what `canvas` holds inside `area` (source over).
    fn blend_into(&self, canvas: &mut [u8], width: u32, area: Rect) {
        let r = self.rect.intersect(&area);
        for y in r.y..r.bottom() {
            for x in r.x..r.right() {
                let s = (((y - self.rect.y) * self.rect.w + (x - self.rect.x)) * 4) as usize;
                let d = ((y as u32 * width + x as u32) * 4) as usize;
                let a = self.pixels[s + 3] as u32;
                for k in 0..4 {
                    canvas[d + k] =
                        (self.pixels[s + k] as u32 + canvas[d + k] as u32 * (255 - a) / 255) as u8;
                }
            }
        }
    }
}

fn rounded_rect(w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(r, 0.0);
    pb.line_to(w - r, 0.0);
    pb.quad_to(w, 0.0, w, r);
    pb.line_to(w, h - r);
    pb.quad_to(w, h, w - r, h);
    pb.line_to(r, h);
    pb.quad_to(0.0, h, 0.0, h - r);
    pb.line_to(0.0, r);
    pb.quad_to(0.0, 0.0, r, 0.0);
    pb.close();
    pb.finish()
}
