//! App icons for the window label (PLAN.md §4.2), found the freedesktop way:
//! the app's `.desktop` file names its icon, looked up in the `hicolor`
//! theme, then `pixmaps`.
//!
//! Only PNG icons are drawn. An SVG renderer (resvg) would add more to the
//! binary than the rest of the Overlay's drawing together, so an app with
//! only an SVG icon gets a letter instead ([`Fonts::window_label`]).
//!
//! [`Fonts::window_label`]: crate::chrome::Fonts::window_label

use std::path::{Path, PathBuf};

use tiny_skia::Pixmap;

/// Sizes to look in, best first: a little larger than the label, so the
/// icon is only ever scaled down.
const SIZES: [&str; 8] = [
    "32x32", "48x48", "64x64", "24x24", "128x128", "256x256", "22x22", "16x16",
];

/// The icon of the app with `app_id` (or X11 class), premultiplied RGBA,
/// or `None` if no PNG icon is found.
pub(crate) fn lookup(app_id: &str) -> Option<Pixmap> {
    if app_id.is_empty() {
        return None;
    }
    let bases = data_dirs();
    let name = desktop_icon(&bases, app_id).unwrap_or_else(|| app_id.to_owned());
    let path = if Path::new(&name).is_absolute() {
        Some(PathBuf::from(&name))
    } else {
        find(&bases, &name).or_else(|| find(&bases, &name.to_ascii_lowercase()))
    };
    decode(&std::fs::read(path?).ok()?)
}

/// `$XDG_DATA_HOME` and `$XDG_DATA_DIRS`, in that order.
fn data_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map_or_else(|| home.join(".local/share"), PathBuf::from);
    let dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    std::iter::once(data_home)
        .chain(dirs.split(':').map(PathBuf::from))
        .collect()
}

/// The `Icon=` of `app_id`'s desktop file, if there is one.
fn desktop_icon(bases: &[PathBuf], app_id: &str) -> Option<String> {
    let names = [app_id.to_owned(), app_id.to_ascii_lowercase()];
    bases
        .iter()
        .flat_map(|b| {
            names
                .iter()
                .map(move |n| b.join(format!("applications/{n}.desktop")))
        })
        .find_map(|path| {
            let text = std::fs::read_to_string(path).ok()?;
            text.lines()
                .find_map(|l| l.strip_prefix("Icon="))
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        })
}

/// `name`'s PNG in the hicolor theme or in `pixmaps`.
fn find(bases: &[PathBuf], name: &str) -> Option<PathBuf> {
    let themed = bases.iter().flat_map(|base| {
        SIZES
            .iter()
            .map(move |size| base.join(format!("icons/hicolor/{size}/apps/{name}.png")))
    });
    let pixmaps = bases
        .iter()
        .map(|base| base.join(format!("pixmaps/{name}.png")));
    themed.chain(pixmaps).find(|p| p.is_file())
}

/// A PNG as a premultiplied pixmap; `None` if it does not decode.
fn decode(bytes: &[u8]) -> Option<Pixmap> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    let pixels = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => pixels
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    let premultiplied = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let a = u32::from(p[3]);
            let pre = |v: u8| (u32::from(v) * a / 255) as u8;
            [pre(p[0]), pre(p[1]), pre(p[2]), p[3]]
        })
        .collect();
    Pixmap::from_vec(premultiplied, tiny_skia::IntSize::from_wh(w, h)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_desktop_file_names_the_icon_and_hicolor_has_it() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().to_owned();
        std::fs::create_dir_all(base.join("applications")).unwrap();
        std::fs::write(
            base.join("applications/org.example.Editor.desktop"),
            "[Desktop Entry]\nName=Editor\nIcon=editor-icon\n",
        )
        .unwrap();
        let icons = base.join("icons/hicolor/48x48/apps");
        std::fs::create_dir_all(&icons).unwrap();
        std::fs::write(icons.join("editor-icon.png"), b"not really").unwrap();

        let bases = [base.clone()];
        let name = desktop_icon(&bases, "org.example.Editor").unwrap();
        assert_eq!(name, "editor-icon");
        assert_eq!(find(&bases, &name), Some(icons.join("editor-icon.png")));
        assert_eq!(find(&bases, "missing"), None);
        assert!(decode(b"not really").is_none());
    }
}
