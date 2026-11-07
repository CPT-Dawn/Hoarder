//! `hoarder shot`: freeze, cut the Target, deliver (PLAN.md §4.1).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::ValueEnum;
use hoarder_config::{Config, MixedScale, Screenshot, State, StoredRegion, WindowSource};
use hoarder_ui::{Error as UiError, Options, Outcome, Region, Theme, countdown, overlay};
use hoarder_wayland::{CaptureError, Frozen, Window};

use crate::Exit;
use crate::after::Delivered;
use crate::cut;
use crate::image::Image;

/// What a Capture covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Target {
    /// A rectangle dragged out on the Overlay; it may span outputs.
    Region,
    /// A window: picked on the Overlay, or by title with `--list`.
    Window,
    /// Every output, as one image laid out like the desktop.
    All,
}

impl Target {
    /// The name used in file name templates.
    fn name(self) -> &'static str {
        match self {
            Target::Region => "region",
            Target::Window => "window",
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
    /// `--delay`: count down this long before the freeze.
    pub delay: Option<Duration>,
}

/// Parse `--delay`: non-negative seconds, decimals allowed.
pub fn parse_delay(s: &str) -> Result<Duration, String> {
    let secs: f64 = s
        .parse()
        .map_err(|_| format!("`{s}` is not a number of seconds"))?;
    Duration::try_from_secs_f64(secs)
        .map_err(|_| format!("`{s}` is not a non-negative number of seconds"))
}

/// Capture `target`. `list` is `--list` and its TITLE, if given. `output`
/// (`-o`) replaces saving to `screenshot.dir`.
pub fn run(
    target: Target,
    list: Option<Option<&str>>,
    when: When,
    output: Option<&Path>,
    config: &Config,
) -> Result<(), (Exit, String)> {
    if list.is_some() && target != Target::Window {
        return Err((
            Exit::Usage,
            "`--list` picks a window: use `hoarder shot window --list`".into(),
        ));
    }
    if list == Some(None) {
        crate::window::by_title(None, &config.screenshot)?;
        return Ok(());
    }
    let mut state = State::load();
    let options = Options {
        confirm: config.screenshot.confirm,
        last_region: state.last_region().map(|r| Region {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        }),
        theme: Theme::load(&config.theme),
        // Both decided in `from_overlay`, for the moment the Overlay shows.
        clamp_to_output: false,
        windows: Vec::new(),
        hint_bar: state.hint_bar_due(),
        cursor: config.screenshot.cursor.into(),
        pick: false,
    };
    if let Some(delay) = when.delay.filter(|d| !d.is_zero()) {
        countdown::run(delay, &options).map_err(overlay_error)?;
    }
    let started = Instant::now();
    let (cut, name) = match (target, list) {
        (_, Some(title)) => (
            crate::window::by_title(title, &config.screenshot)?.expect("a title picks a window"),
            target.name(),
        ),
        (Target::All, None) => {
            let mut frozen = freeze()?;
            if config.screenshot.cursor {
                composite_cursor(&mut frozen);
            }
            (
                cut::cut(&frozen.frames, None, &config.screenshot),
                target.name(),
            )
        }
        (Target::Region | Target::Window, None) => {
            from_overlay(when.live, started, &options, &mut state, config)?
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
            .map(|(part, image)| {
                crate::save::spawn(
                    image.clone(),
                    name,
                    None,
                    part.clone(),
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
        target: name,
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
        "may be stale" => "the window is on a hidden workspace, where it may not have been redrawn",
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

pub(crate) fn freeze() -> Result<Frozen, (Exit, String)> {
    hoarder_wayland::freeze().map_err(|e| {
        let code = match e {
            CaptureError::NoWayland(_) | CaptureError::Unsupported(_) => Exit::Unsupported,
            CaptureError::Failed(_) | CaptureError::Io(_) => Exit::Io,
        };
        (code, e.to_string())
    })
}

pub(crate) fn overlay_error(e: UiError) -> (Exit, String) {
    let code = match e {
        UiError::Unsupported(_) => Exit::Unsupported,
        UiError::Failed(_) => Exit::Io,
    };
    (code, e.to_string())
}

/// Show the Overlay and cut out the Target picked there; returns the Cut
/// and the Target's name for file names. With `live`, the Overlay goes over
/// the live desktop and the freeze comes after it, Overlay already hidden.
fn from_overlay(
    live: bool,
    started: Instant,
    options: &Options,
    state: &mut State,
    config: &Config,
) -> Result<(cut::Cut, &'static str), (Exit, String)> {
    let settings = &config.screenshot;
    let (mut frozen, outcome, cursor) = if live {
        // No Frozen Frames yet to judge the scales by: the Overlay clamps only
        // if the live outputs it covers turn out to differ in scale.
        // The windows as they are now, on the desktop the Overlay goes over.
        let options = Options {
            clamp_to_output: settings.mixed_scale == MixedScale::Clamp,
            windows: windows(),
            ..options.clone()
        };
        let outcome = overlay::run_live(started, &options);
        count_hint_bar(&options, state);
        let outcome = outcome.map_err(overlay_error)?;
        if outcome == Outcome::Cancelled {
            return Err((Exit::Cancelled, "cancelled".into()));
        }
        (freeze()?, outcome, options.cursor.get())
    } else {
        // Ask where the windows are while the outputs freeze, so both
        // describe the same moment.
        let windows = std::thread::spawn(windows);
        let frozen = freeze()?;
        let options = Options {
            clamp_to_output: cut::clamps_regions(&frozen.frames, settings),
            windows: windows.join().unwrap_or_default(),
            ..options.clone()
        };
        let outcome = overlay::run(&frozen, started, &options);
        count_hint_bar(&options, state);
        (
            frozen,
            outcome.map_err(overlay_error)?,
            options.cursor.get(),
        )
    };
    // The user's last word on the cursor layer (PLAN.md §4.1 step 6).
    if cursor {
        composite_cursor(&mut frozen);
    }
    Ok(match outcome {
        Outcome::Region(r) => {
            state.set_last_region(StoredRegion {
                x: r.x,
                y: r.y,
                width: r.width,
                height: r.height,
            });
            save_state(state);
            (cut::cut(&frozen.frames, Some(&r), settings), "region")
        }
        Outcome::Output(name) => {
            let frame = frozen
                .frames
                .iter()
                .find(|f| f.output == name)
                .ok_or_else(|| (Exit::Io, format!("output {name} went away")))?;
            let whole = Region {
                x: frame.logical.x as f64,
                y: frame.logical.y as f64,
                width: frame.logical.width as f64,
                height: frame.logical.height as f64,
            };
            (cut::cut(&frozen.frames, Some(&whole), settings), "output")
        }
        Outcome::WindowBuffer(window) => (window_buffer(&frozen, &window, settings), "window"),
        Outcome::Window(window) if settings.window_source == WindowSource::Toplevel => {
            (window_buffer(&frozen, &window, settings), "window")
        }
        Outcome::Window(window) => {
            // What you saw: the window's rectangle cut from the Frozen Frames,
            // popups and anything over it included, under the same mixed-scale
            // and depth policies as any Region.
            let region = window_region(&window);
            (cut::cut(&frozen.frames, Some(&region), settings), "window")
        }
        Outcome::AllOutputs => (cut::cut(&frozen.frames, None, settings), "all"),
        Outcome::Cancelled => return Err((Exit::Cancelled, "cancelled".into())),
        Outcome::Pixel { .. } => unreachable!("only `hoarder pick` picks pixels"),
    })
}

/// `window`'s own buffer; if the compositor cannot give it, say why and
/// fall back to what the Frozen Frames show of it.
fn window_buffer(frozen: &Frozen, window: &Window, settings: &Screenshot) -> cut::Cut {
    crate::window::own_buffer(window, settings).unwrap_or_else(|e| {
        eprintln!("hoarder: taking the window as shown instead of its own buffer: {e}");
        cut::cut(&frozen.frames, Some(&window_region(window)), settings)
    })
}

fn window_region(window: &Window) -> Region {
    let r = window.rect;
    Region {
        x: r.x as f64,
        y: r.y as f64,
        width: r.width as f64,
        height: r.height as f64,
    }
}

/// One more showing of the hint bar, if the Overlay showed it.
fn count_hint_bar(options: &Options, state: &mut State) {
    if options.hint_bar {
        state.record_hint_bar_use();
        save_state(state);
    }
}

/// Write `state.toml`; losing it costs a convenience, never the Capture.
fn save_state(state: &State) {
    if let Err(e) = state.save() {
        eprintln!(
            "hoarder: cannot write {}: {e}",
            hoarder_config::state_path().display()
        );
    }
}

/// Put the cursor layer into the Frozen Frames.
fn composite_cursor(frozen: &mut Frozen) {
    for frame in &mut frozen.frames {
        frame.composite_cursor();
    }
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
