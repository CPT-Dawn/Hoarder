//! `hoarder shot`: freeze, cut the Target, deliver (PLAN.md §4.1).

use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;

use clap::ValueEnum;
use hoarder_config::{Config, Screenshot};
use hoarder_wayland::CaptureError;

use crate::Exit;
use crate::image::Image;

/// What a Capture covers. Region, Window and Output need the Overlay (#61).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Target {
    /// Every output, as one image laid out like the desktop.
    All,
}

/// Capture `target`. `output` (`-o`) replaces saving to `screenshot.dir`.
pub fn run(target: Target, output: Option<&Path>, config: &Config) -> Result<(), (Exit, String)> {
    let frozen = hoarder_wayland::freeze().map_err(|e| {
        let code = match e {
            CaptureError::NoWayland(_) | CaptureError::Unsupported(_) => Exit::Unsupported,
            CaptureError::Failed(_) | CaptureError::Io(_) => Exit::Io,
        };
        (code, e.to_string())
    })?;
    let image = Arc::new(match target {
        Target::All => Image::all_outputs(&frozen.frames),
    });
    if let Some(output) = output {
        return deliver(&image, output, &config.screenshot)
            .map_err(|e| (Exit::Io, format!("cannot write {}: {e}", output.display())));
    }
    // The file is encoded on a worker thread, so a slow format never delays
    // the clipboard copy, which happens here, between the two.
    let saving = config
        .screenshot
        .save
        .then(|| crate::save::spawn(image.clone(), "all", None, config.screenshot.clone()));
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
