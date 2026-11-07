//! `hoarder shot`: freeze, cut the Target, deliver (PLAN.md §4.1).

use std::io::Write as _;
use std::path::Path;
use std::time::Instant;

use clap::ValueEnum;
use hoarder_config::Config;
use hoarder_ui::{Outcome, OverlayError, Region, overlay};
use hoarder_wayland::{CaptureError, Window};

use crate::Exit;
use crate::image::Image;

/// What a Capture covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Target {
    /// A rectangle dragged out on the Overlay; it may span outputs.
    Region,
    /// Every output, as one image laid out like the desktop.
    All,
}

impl Target {
    /// The name used in file name templates.
    fn name(self) -> &'static str {
        match self {
            Target::Region => "region",
            Target::All => "all",
        }
    }
}

/// Capture `target`. `output` (`-o`) replaces saving to `screenshot.dir`.
pub fn run(target: Target, output: Option<&Path>, config: &Config) -> Result<(), (Exit, String)> {
    let started = Instant::now();
    // Ask where the windows are while the outputs freeze, so both describe
    // the same moment.
    let windows = (target == Target::Region).then(|| std::thread::spawn(windows));
    let frozen = hoarder_wayland::freeze().map_err(|e| {
        let code = match e {
            CaptureError::NoWayland(_) | CaptureError::Unsupported(_) => Exit::Unsupported,
            CaptureError::Failed(_) | CaptureError::Io(_) => Exit::Io,
        };
        (code, e.to_string())
    })?;
    let windows = windows.and_then(|t| t.join().ok()).unwrap_or_default();
    let (image, target_name) = match target {
        Target::All => (Image::all_outputs(&frozen.frames), target.name()),
        Target::Region => match overlay::run(&frozen, &windows, started).map_err(|e| {
            let code = match e {
                OverlayError::Unsupported(_) => Exit::Unsupported,
                OverlayError::Failed(_) => Exit::Io,
            };
            (code, e.to_string())
        })? {
            Outcome::Region(region) => (Image::region(&frozen.frames, &region), "region"),
            Outcome::Window(window) => {
                let r = window.rect;
                let region = Region {
                    x: r.x as f64,
                    y: r.y as f64,
                    width: r.width as f64,
                    height: r.height as f64,
                };
                (Image::region(&frozen.frames, &region), "window")
            }
            Outcome::Cancelled => return Err((Exit::Cancelled, "cancelled".into())),
        },
    };
    if config.screenshot.copy {
        let mut png = Vec::new();
        image
            .write_png(&mut png)
            .map_err(|e| (Exit::Io, format!("cannot encode the Capture: {e}")))?;
        crate::clipboard::copy_capture(&png)?;
    }
    match output {
        Some(output) => deliver(&image, output)
            .map_err(|e| (Exit::Io, format!("cannot write {}: {e}", output.display()))),
        None if config.screenshot.save => {
            let path = crate::save::save(&image, target_name, None, &config.screenshot)?;
            println!("{}", path.display());
            Ok(())
        }
        None => Ok(()),
    }
}

/// The windows the Overlay can snap to. Without a provider there are none;
/// if the compositor cannot tell, say why and carry on without them.
fn windows() -> Vec<Window> {
    hoarder_wayland::windows::detect()
        .windows()
        .unwrap_or_else(|e| {
            eprintln!("hoarder: no window snapping: {e}");
            Vec::new()
        })
}

fn deliver(image: &Image, output: &Path) -> std::io::Result<()> {
    let mut buf = Vec::new();
    image.write_png(&mut buf).map_err(std::io::Error::other)?;
    if output == Path::new("-") {
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(&buf)?;
        stdout.flush()
    } else {
        std::fs::write(output, buf)
    }
}
