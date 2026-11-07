//! Window Targets taken from the window's own buffer (PLAN.md §4.1): the
//! ext foreign-toplevel capture source, used for Shift+click, for
//! `screenshot.window_source = "toplevel"` and always for
//! `hoarder shot window --list`.
//!
//! The toplevel list names the windows; the window-geometry provider, where
//! there is one, adds where each is and at what scale, and whether its
//! buffer may be stale.

use hoarder_config::{Capture, Depth, MixedScale};
use hoarder_wayland::toplevel::{self, Toplevel, ToplevelFrame};
use hoarder_wayland::{CaptureError, FrozenFrame, LogicalRect, Window};

use crate::Exit;
use crate::cut::{Badges, Cut, Part};
use crate::image::Image;

/// `hoarder shot window --list`: with no `title`, print the windows and
/// return `None`; with one, capture the window it names.
pub fn by_title(title: Option<&str>, settings: &Capture) -> Result<Option<Cut>, (Exit, String)> {
    let toplevels = toplevel::list().map_err(capture_error)?;
    let windows = hoarder_wayland::windows::detect()
        .all_windows()
        .unwrap_or_else(|e| {
            eprintln!("hoarder: windows listed without their geometry: {e}");
            Vec::new()
        });
    let known = |t: &Toplevel| window_of(t, &windows);
    let Some(title) = title else {
        for t in &toplevels {
            println!("{}", listing(t, known(t)));
        }
        return Ok(None);
    };
    let chosen = pick(&toplevels, title, known)?;
    let frame = toplevel::capture(&chosen.identifier).map_err(capture_error)?;
    Ok(Some(cut(frame, known(chosen), settings)))
}

/// Shift+click (or `window_source = "toplevel"`): `window`'s own buffer.
pub fn own_buffer(window: &Window, settings: &Capture) -> Result<Cut, CaptureError> {
    let identifier = match &window.toplevel {
        Some(id) => id.clone(),
        None => {
            let toplevels = toplevel::list()?;
            toplevel_of(window, &toplevels)
                .ok_or_else(|| {
                    CaptureError::Failed(format!(
                        "cannot tell which toplevel is the window `{}`",
                        window.title
                    ))
                })?
                .identifier
                .clone()
        }
    };
    Ok(cut(toplevel::capture(&identifier)?, Some(window), settings))
}

/// The provider's window for toplevel `t`: by identifier, by Hyprland
/// address, or else the only one with its title and app_id.
fn window_of<'a>(t: &Toplevel, windows: &'a [Window]) -> Option<&'a Window> {
    windows
        .iter()
        .find(|w| w.toplevel.as_deref() == Some(&t.identifier))
        .or_else(|| {
            let address = t.hyprland_address?;
            windows.iter().find(|w| w.address == Some(address))
        })
        .or_else(|| only(windows.iter().filter(|w| same_names(w, t))))
}

/// The toplevel for the provider's `window`, as [`window_of`] pairs them.
fn toplevel_of<'a>(window: &Window, toplevels: &'a [Toplevel]) -> Option<&'a Toplevel> {
    toplevels
        .iter()
        .find(|t| window.address.is_some() && t.hyprland_address == window.address)
        .or_else(|| only(toplevels.iter().filter(|t| same_names(window, t))))
}

fn same_names(w: &Window, t: &Toplevel) -> bool {
    w.title == t.title && w.app_id == t.app_id
}

fn only<T>(mut it: impl Iterator<Item = T>) -> Option<T> {
    match (it.next(), it.next()) {
        (Some(one), None) => Some(one),
        _ => None,
    }
}

/// One line of the window list: the title, the app_id and, for a window
/// whose buffer may be old, a note, separated by tabs. The whole line, or
/// just the title, picks the window again.
fn listing(t: &Toplevel, window: Option<&Window>) -> String {
    let stale = window.is_some_and(|w| w.may_be_stale);
    format!(
        "{}\t{}{}",
        t.title,
        t.app_id,
        if stale { "\tmay be stale" } else { "" }
    )
}

/// The window `wanted` names: a title (optionally followed by a tab and
/// the app_id, as listed), matched exactly, else as a case-insensitive part
/// of the title. Ambiguous or unknown names are usage errors.
fn pick<'a, 'w>(
    toplevels: &'a [Toplevel],
    wanted: &str,
    known: impl Fn(&Toplevel) -> Option<&'w Window>,
) -> Result<&'a Toplevel, (Exit, String)> {
    let mut fields = wanted.split('\t');
    let title = fields.next().unwrap_or_default();
    let app_id = fields.next();
    let app_ok = |t: &Toplevel| app_id.is_none_or(|a| t.app_id == a);
    let exact: Vec<&Toplevel> = toplevels
        .iter()
        .filter(|t| t.title == title && app_ok(t))
        .collect();
    let found = if exact.is_empty() {
        let needle = title.to_lowercase();
        toplevels
            .iter()
            .filter(|t| t.title.to_lowercase().contains(&needle) && app_ok(t))
            .collect()
    } else {
        exact
    };
    match found.as_slice() {
        [one] => Ok(one),
        [] => Err((Exit::Usage, format!("no window is titled `{wanted}`"))),
        many => Err((
            Exit::Usage,
            format!(
                "`{wanted}` could be any of these windows; pick one by its full line:\n{}",
                many.iter()
                    .map(|t| listing(t, known(t)))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        )),
    }
}

/// The Cut of a window's buffer, under the policies a Region of it would
/// follow. A buffer smaller than the window in its output's pixels (Sway
/// hands over toplevels at a fractional scale at their logical size) is
/// enlarged by nearest neighbour under `mixed_scale = "upscale"`, with the
/// badge; `clamp` and `split` keep it as it came.
fn cut(frame: ToplevelFrame, window: Option<&Window>, settings: &Capture) -> Cut {
    let frozen = FrozenFrame {
        output: String::new(),
        logical: LogicalRect {
            x: 0,
            y: 0,
            width: frame.width,
            height: frame.height,
        },
        format: frame.format,
        width: frame.width,
        height: frame.height,
        stride: frame.stride,
        transform: frame.transform,
        y_invert: false,
        hdr: false,
        data: frame.data,
        cursor: None,
    };
    let mut image = Image::upright(&frozen);
    let mut badges = Badges {
        stale: window.is_some_and(|w| w.may_be_stale),
        ..Badges::default()
    };
    let native = window.and_then(|w| {
        let scale = w.scale?;
        let px = |v: u32| (v as f64 * scale).round() as u32;
        Some((px(w.rect.width), px(w.rect.height)))
    });
    if let Some((width, height)) = native
        && settings.mixed_scale == MixedScale::Upscale
        && image.width < width
        && image.height < height
    {
        image = image.upscaled(width, height);
        badges.mixed_scale = true;
    }
    let image = match settings.depth {
        Depth::Auto => image,
        Depth::Eight => image.into_eight_bit(),
    };
    Cut {
        parts: vec![Part {
            output: None,
            image,
        }],
        badges,
    }
}

fn capture_error(e: CaptureError) -> (Exit, String) {
    let code = match e {
        CaptureError::NoWayland(_) | CaptureError::Unsupported(_) => Exit::Unsupported,
        CaptureError::Failed(_) | CaptureError::Io(_) => Exit::Io,
    };
    (code, e.to_string())
}
