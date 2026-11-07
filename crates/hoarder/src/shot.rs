//! `hoarder shot`: freeze, cut the Target, deliver (PLAN.md §4.1).

use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use clap::ValueEnum;
use hoarder_config::{Config, Screenshot, State, StoredRegion};
use hoarder_ui::{Options, Outcome, OverlayError, Region, overlay};
use hoarder_wayland::{CaptureError, Window};

use crate::Exit;
use crate::after::Delivered;
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
    let (image, name) = match target {
        Target::All => (Image::all_outputs(&frozen.frames), target.name()),
        Target::Region => {
            let windows = windows.and_then(|t| t.join().ok()).unwrap_or_default();
            from_overlay(&frozen, windows, started, config)?
        }
    };
    let image = Arc::new(image);
    // The file is encoded on a worker thread, started before the clipboard
    // copy, so a slow format (AVIF takes seconds at 4K) never delays it.
    let saving = (output.is_none() && config.screenshot.save)
        .then(|| crate::save::spawn(image.clone(), name, None, config.screenshot.clone()));
    // Only now the Target is chosen: never over the Overlay, never in the Capture.
    let _flash = crate::after::flash(&config.screenshot);
    if config.screenshot.copy {
        let mut png = Vec::new();
        image
            .write_png(&mut png)
            .map_err(|e| (Exit::Io, format!("cannot encode the Capture: {e}")))?;
        crate::clipboard::copy_capture(&png)?;
    }
    let path = match (output, saving) {
        (Some(output), _) => {
            deliver(&image, output, &config.screenshot)
                .map_err(|e| (Exit::Io, format!("cannot write {}: {e}", output.display())))?;
            (output != Path::new("-")).then(|| output.to_owned())
        }
        (None, Some(saving)) => {
            let path = saving.wait()?;
            println!("{}", path.display());
            Some(path)
        }
        (None, None) => None,
    };
    let delivered = Delivered {
        path,
        width: image.width,
        height: image.height,
        target: name,
    };
    crate::after::run(&delivered, &config.screenshot);
    Ok(())
}

/// Show the Overlay and cut out the Target picked there; returns the image
/// and the Target's name for file names.
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

fn from_overlay(
    frozen: &hoarder_wayland::Frozen,
    windows: Vec<Window>,
    started: Instant,
    config: &Config,
) -> Result<(Image, &'static str), (Exit, String)> {
    let mut state = State::load();
    let options = Options {
        confirm: config.screenshot.confirm,
        last_region: state.last_region().map(|r| Region {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        }),
        windows,
    };
    let outcome = overlay::run(frozen, started, &options).map_err(|e| {
        let code = match e {
            OverlayError::Unsupported(_) => Exit::Unsupported,
            OverlayError::Failed(_) => Exit::Io,
        };
        (code, e.to_string())
    })?;
    Ok(match outcome {
        Outcome::Region(r) => {
            state.set_last_region(StoredRegion {
                x: r.x,
                y: r.y,
                width: r.width,
                height: r.height,
            });
            if let Err(e) = state.save() {
                eprintln!(
                    "hoarder: cannot remember the Region in {}: {e}",
                    hoarder_config::state_path().display()
                );
            }
            (Image::region(&frozen.frames, &r), "region")
        }
        Outcome::Output(name) => {
            let frame = frozen
                .frames
                .iter()
                .find(|f| f.output == name)
                .ok_or_else(|| (Exit::Io, format!("output {name} went away")))?;
            (Image::upright(frame), "output")
        }
        Outcome::Window(window) => {
            // What you saw: the window's rectangle cut from the Frozen Frames,
            // popups and anything over it included.
            let r = window.rect;
            let region = Region {
                x: r.x as f64,
                y: r.y as f64,
                width: r.width as f64,
                height: r.height as f64,
            };
            (Image::region(&frozen.frames, &region), "window")
        }
        Outcome::AllOutputs => (Image::all_outputs(&frozen.frames), "all"),
        Outcome::Cancelled => return Err((Exit::Cancelled, "cancelled".into())),
    })
}

fn deliver(image: &Image, output: &Path, settings: &Screenshot) -> std::io::Result<()> {
    let mut buf = Vec::new();
    crate::encode::encode(image, settings.format, settings, &mut buf)
        .map_err(std::io::Error::other)?;
    if output == Path::new("-") {
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(&buf)?;
        stdout.flush()
    } else {
        std::fs::write(output, buf)
    }
}
