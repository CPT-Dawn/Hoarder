//! `hoarder shot`: freeze, cut the Target, deliver (PLAN.md §4.1).

use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use clap::ValueEnum;
use hoarder_config::{Config, Screenshot};
use hoarder_ui::{Outcome, OverlayError, overlay};
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
    let image = Arc::new(match target {
        Target::All => Image::all_outputs(&frozen.frames),
        Target::Region => match overlay::run(&frozen, started).map_err(|e| {
            let code = match e {
                OverlayError::Unsupported(_) => Exit::Unsupported,
                OverlayError::Failed(_) => Exit::Io,
            };
            (code, e.to_string())
        })? {
            Outcome::Region(region) => Image::region(&frozen.frames, &region),
            Outcome::Cancelled => return Err((Exit::Cancelled, "cancelled".into())),
        },
    });
    // The file is encoded on a worker thread, started before the clipboard
    // copy, so a slow format (AVIF takes seconds at 4K) never delays it.
    let saving = (output.is_none() && config.screenshot.save).then(|| {
        crate::save::spawn(
            image.clone(),
            target.name(),
            None,
            config.screenshot.clone(),
        )
    });
    if config.screenshot.copy {
        let mut png = Vec::new();
        image
            .write_png(&mut png)
            .map_err(|e| (Exit::Io, format!("cannot encode the Capture: {e}")))?;
        crate::clipboard::copy_capture(&png)?;
    }
    if let Some(output) = output {
        return deliver(&image, output, &config.screenshot)
            .map_err(|e| (Exit::Io, format!("cannot write {}: {e}", output.display())));
    }
    if let Some(saving) = saving {
        println!("{}", saving.wait()?.display());
    }
    Ok(())
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
