//! `hoarder shot`: freeze, cut the Target, deliver (PLAN.md §4.1).

use std::io::Write as _;
use std::path::Path;
use std::time::Instant;

use clap::ValueEnum;
use hoarder_config::Config;
use hoarder_config::{State, StoredRegion};
use hoarder_ui::{Options, Outcome, OverlayError, Region, overlay};
use hoarder_wayland::CaptureError;

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
    let frozen = hoarder_wayland::freeze().map_err(|e| {
        let code = match e {
            CaptureError::NoWayland(_) | CaptureError::Unsupported(_) => Exit::Unsupported,
            CaptureError::Failed(_) | CaptureError::Io(_) => Exit::Io,
        };
        (code, e.to_string())
    })?;
    let (image, name) = match target {
        Target::All => (Image::all_outputs(&frozen.frames), target.name()),
        Target::Region => from_overlay(&frozen, started, config)?,
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
            let path = crate::save::save(&image, name, None, &config.screenshot)?;
            println!("{}", path.display());
            Ok(())
        }
        None => Ok(()),
    }
}

/// Show the Overlay and cut out the Target picked there; returns the image
/// and the Target's name for file names.
fn from_overlay(
    frozen: &hoarder_wayland::Frozen,
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
        Outcome::AllOutputs => (Image::all_outputs(&frozen.frames), "all"),
        Outcome::Cancelled => return Err((Exit::Cancelled, "cancelled".into())),
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
