//! `hoarder shot`: freeze, cut the Target, deliver (PLAN.md §4.1).

use std::io::Write as _;
use std::path::Path;

use clap::ValueEnum;
use hoarder_config::Config;
use hoarder_wayland::CaptureError;

use crate::Exit;
use crate::after::Delivered;
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
    let _flash = crate::after::flash(&config.screenshot);
    let image = match target {
        Target::All => Image::all_outputs(&frozen.frames),
    };
    if config.screenshot.copy {
        let mut png = Vec::new();
        image
            .write_png(&mut png)
            .map_err(|e| (Exit::Io, format!("cannot encode the Capture: {e}")))?;
        crate::clipboard::copy_capture(&png)?;
    }
    let path = match output {
        Some(output) => {
            deliver(&image, output)
                .map_err(|e| (Exit::Io, format!("cannot write {}: {e}", output.display())))?;
            (output != Path::new("-")).then(|| output.to_owned())
        }
        None if config.screenshot.save => {
            let path = crate::save::save(&image, "all", None, &config.screenshot)?;
            println!("{}", path.display());
            Some(path)
        }
        None => None,
    };
    let delivered = Delivered {
        path,
        width: image.width,
        height: image.height,
        target: "all",
    };
    crate::after::run(&delivered, &config.screenshot);
    Ok(())
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
