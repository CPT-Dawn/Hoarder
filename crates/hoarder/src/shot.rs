//! `hoarder shot`: freeze, cut the Target, deliver (PLAN.md §4.1).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use clap::ValueEnum;
use hoarder_config::{Config, Screenshot};
use hoarder_ui::{Options, Outcome, OverlayError, overlay};
use hoarder_wayland::CaptureError;

use crate::Exit;
use crate::after::Delivered;
use crate::cut;
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
    let cut = match target {
        Target::All => cut::cut(&frozen.frames, None, &config.screenshot),
        Target::Region => {
            let options = Options {
                clamp_to_output: cut::clamps_regions(&frozen.frames, &config.screenshot),
            };
            match overlay::run(&frozen, started, options).map_err(|e| {
                let code = match e {
                    OverlayError::Unsupported(_) => Exit::Unsupported,
                    OverlayError::Failed(_) => Exit::Io,
                };
                (code, e.to_string())
            })? {
                Outcome::Region(region) => {
                    cut::cut(&frozen.frames, Some(&region), &config.screenshot)
                }
                Outcome::Cancelled => return Err((Exit::Cancelled, "cancelled".into())),
            }
        }
    };
    // Also on stderr, for runs that end up in a pipe rather than a notification.
    for badge in cut.badges.labels() {
        eprintln!("hoarder: {badge}: {}", explain(badge));
    }
    let main = Arc::new(cut.main_part().image.clone());
    let parts: Vec<(Option<String>, Arc<Image>)> = cut
        .parts
        .into_iter()
        .map(|p| (p.output, Arc::new(p.image)))
        .collect();
    // The files are encoded on worker threads, started before the clipboard
    // copy, so a slow format (AVIF takes seconds at 4K) never delays it.
    let savings: Vec<_> = if output.is_none() && config.screenshot.save {
        parts
            .iter()
            .map(|(name, image)| {
                crate::save::spawn(
                    image.clone(),
                    target.name(),
                    None,
                    name.clone(),
                    config.screenshot.clone(),
                )
            })
            .collect()
    } else {
        Vec::new()
    };
    // Only now the Target is chosen: never over the Overlay, never in the Capture.
    let _flash = crate::after::flash(&config.screenshot);
    if config.screenshot.copy {
        let mut png = Vec::new();
        main.write_png(&mut png)
            .map_err(|e| (Exit::Io, format!("cannot encode the Capture: {e}")))?;
        crate::clipboard::copy_capture(&png)?;
    }
    let delivered = |image: &Image, path: Option<PathBuf>| Delivered {
        path,
        width: image.width,
        height: image.height,
        target: target.name(),
        badges: cut.badges,
    };
    let delivered: Vec<Delivered> = match output {
        Some(output) => {
            let write = |image: &Image, path: &Path| {
                deliver(image, path, &config.screenshot)
                    .map_err(|e| (Exit::Io, format!("cannot write {}: {e}", path.display())))
            };
            match parts.as_slice() {
                _ if output == Path::new("-") => {
                    if parts.len() > 1 {
                        eprintln!(
                            "hoarder: stdout takes one image; writing the largest of the {} split parts",
                            parts.len()
                        );
                    }
                    write(&main, output)?;
                    vec![delivered(&main, None)]
                }
                [(_, image)] => {
                    write(image, output)?;
                    vec![delivered(image, Some(output.to_owned()))]
                }
                _ => parts
                    .iter()
                    .map(|(name, image)| {
                        let path = beside(output, name.as_deref().unwrap_or_default());
                        write(image, &path)?;
                        Ok(delivered(image, Some(path)))
                    })
                    .collect::<Result<_, _>>()?,
            }
        }
        None if savings.is_empty() => vec![delivered(&main, None)],
        None => savings
            .into_iter()
            .zip(&parts)
            .map(|(saving, (_, image))| {
                let path = saving.wait()?;
                println!("{}", path.display());
                Ok(delivered(image, Some(path)))
            })
            .collect::<Result<_, (Exit, String)>>()?,
    };
    crate::after::run(&delivered, &config.screenshot);
    Ok(())
}

/// What a badge means, for the stderr note.
fn explain(badge: &str) -> &'static str {
    match badge {
        "mixed scale" => "lower-scale outputs were enlarged by nearest neighbour",
        "HDR" => "taken from an HDR output without tone mapping",
        _ => "",
    }
}

/// `path` with `_SUFFIX` added to its file stem: `a/shot.png` → `a/shot_DP-1.png`.
fn beside(path: &Path, suffix: &str) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let name = match path.extension() {
        Some(ext) => format!("{stem}_{suffix}.{}", ext.to_string_lossy()),
        None => format!("{stem}_{suffix}"),
    };
    path.with_file_name(name)
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
