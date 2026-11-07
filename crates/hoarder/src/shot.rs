//! `hoarder shot`: freeze, cut the Target, deliver (PLAN.md §4.1).

use std::io::Write as _;
use std::path::Path;
use std::time::Instant;

use clap::ValueEnum;
use hoarder_config::Config;
use hoarder_ui::{Outcome, OverlayError, Region, overlay};
use hoarder_wayland::{CaptureError, Frozen};

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

/// When the pixels are taken.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct When {
    /// `--live`: no freeze; the Region is cut from the desktop as it is when
    /// the selection ends.
    pub live: bool,
}

/// Capture `target`. `output` (`-o`) replaces saving to `screenshot.dir`.
pub fn run(
    target: Target,
    when: When,
    output: Option<&Path>,
    config: &Config,
) -> Result<(), (Exit, String)> {
    let started = Instant::now();
    let image = match target {
        Target::All => Image::all_outputs(&freeze()?.frames),
        Target::Region if when.live => {
            let region = picked(overlay::run_live(started))?;
            Image::region(&freeze()?.frames, &region)
        }
        Target::Region => {
            let frozen = freeze()?;
            let region = picked(overlay::run(&frozen, started))?;
            Image::region(&frozen.frames, &region)
        }
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
            let path = crate::save::save(&image, target.name(), None, &config.screenshot)?;
            println!("{}", path.display());
            Ok(())
        }
        None => Ok(()),
    }
}

fn freeze() -> Result<Frozen, (Exit, String)> {
    hoarder_wayland::freeze().map_err(|e| {
        let code = match e {
            CaptureError::NoWayland(_) | CaptureError::Unsupported(_) => Exit::Unsupported,
            CaptureError::Failed(_) | CaptureError::Io(_) => Exit::Io,
        };
        (code, e.to_string())
    })
}

/// The Region the user picked on the Overlay, or why there is none.
fn picked(outcome: Result<Outcome, OverlayError>) -> Result<Region, (Exit, String)> {
    match outcome {
        Ok(Outcome::Region(region)) => Ok(region),
        Ok(Outcome::Cancelled) => Err((Exit::Cancelled, "cancelled".into())),
        Err(e) => {
            let code = match e {
                OverlayError::Unsupported(_) => Exit::Unsupported,
                OverlayError::Failed(_) => Exit::Io,
            };
            Err((code, e.to_string()))
        }
    }
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
