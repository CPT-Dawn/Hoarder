//! `hoarder shot`: freeze, cut the Target, deliver (PLAN.md §4.1).

use std::io::Write as _;
use std::path::Path;

use clap::ValueEnum;
use hoarder_wayland::CaptureError;

use crate::Exit;
use crate::image::Image;

/// What a Capture covers. Region, Window and Output need the Overlay (#61).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Target {
    /// Every output, as one image laid out like the desktop.
    All,
}

pub fn run(target: Target, output: Option<&Path>, copy: bool) -> Result<(), (Exit, String)> {
    let frozen = hoarder_wayland::freeze().map_err(|e| {
        let code = match e {
            CaptureError::NoWayland(_) | CaptureError::Unsupported(_) => Exit::Unsupported,
            CaptureError::Failed(_) | CaptureError::Io(_) => Exit::Io,
        };
        (code, e.to_string())
    })?;
    let image = match target {
        Target::All => Image::all_outputs(&frozen.frames),
    };
    let mut png = Vec::new();
    image
        .write_png(&mut png)
        .map_err(|e| (Exit::Io, format!("cannot encode the Capture: {e}")))?;
    if copy {
        crate::clipboard::copy_capture(&png)?;
    }
    match output {
        Some(output) => deliver(png, output)
            .map_err(|e| (Exit::Io, format!("cannot write {}: {e}", output.display()))),
        None => Ok(()),
    }
}

fn deliver(buf: Vec<u8>, output: &Path) -> std::io::Result<()> {
    if output == Path::new("-") {
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(&buf)?;
        stdout.flush()
    } else {
        std::fs::write(output, buf)
    }
}
