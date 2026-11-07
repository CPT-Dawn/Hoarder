mod after;
mod autopaste;
mod clipboard;
mod config;
mod cut;
mod daemon;
mod dist;
mod doctor;
mod encode;
mod history;
mod hook;
mod image;
mod notify;
mod pause;
mod pick;
mod picker;
mod save;
mod setup;
mod shot;
mod window;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};

/// Pixel-exact screenshots and a clipboard that outlives the app that set it.
#[derive(Debug, Parser)]
#[command(name = "hoarder", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Override a setting for this run only, e.g. `--set screenshot.save=false`.
    #[arg(long = "set", global = true, value_name = "KEY=VALUE", value_parser = config::parse_override)]
    overrides: Vec<(String, String)>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Take a Capture.
    Shot {
        /// What to capture.
        #[arg(default_value = "region")]
        target: shot::Target,
        /// Write the image to PATH, or to stdout with `-`, instead of saving it
        /// to `screenshot.dir`.
        #[arg(short = 'o', long = "output", value_name = "PATH")]
        output: Option<PathBuf>,
        /// Image format for this run (`screenshot.format`). Defaults to the
        /// extension of `-o PATH` when it names one.
        #[arg(long, value_name = "F", value_parser = ["png", "webp", "jpeg", "avif", "qoi"])]
        format: Option<String>,
        /// Do not save the Capture to `screenshot.dir` (`screenshot.save = false`).
        #[arg(long)]
        no_save: bool,
        /// Do not copy the Capture to the clipboard (`screenshot.copy = false`).
        #[arg(long)]
        no_copy: bool,
        /// Skip the freeze: select over the live desktop and capture it as it
        /// is when the selection ends.
        #[arg(long)]
        live: bool,
        /// Count down SECONDS (decimals allowed) before freezing.
        #[arg(long, value_name = "SECONDS", value_parser = shot::parse_delay)]
        delay: Option<std::time::Duration>,
        /// No notification for this Capture (`screenshot.notify = false`).
        #[arg(long)]
        no_notify: bool,
        /// With `window`: list every window, or capture the one titled
        /// TITLE (a listed line also works), from its own buffer.
        #[arg(long, value_name = "TITLE", num_args = 0..=1)]
        list: Option<Option<String>>,
        /// Include the pointer cursor (`screenshot.cursor = true`); `C` in
        /// the Overlay toggles it.
        #[arg(long)]
        cursor: bool,
    },
    /// Pick a colour off the screen and copy it.
    Pick {
        /// How to write the colour (`pick.format`).
        #[arg(long, value_name = "F", value_parser = ["hex", "rgb", "hsl"])]
        format: Option<String>,
    },
    /// Copy stdin or text to the clipboard (accepts every wl-copy flag).
    Copy(clipboard::CopyArgs),
    /// Paste the clipboard to stdout (accepts every wl-paste flag).
    Paste(clipboard::PasteArgs),
    /// Serve an Offer read from stdin; started by Hoarder itself.
    #[command(name = clipboard::SERVE_SUBCOMMAND, hide = true)]
    ServeOffer(clipboard::ServeArgs),
    /// Announce a Capture and answer its actions; started by Hoarder itself.
    #[command(name = notify::SUBCOMMAND, hide = true)]
    NotifyCapture(notify::NotifyArgs),
    /// Read, change and check `config.toml`.
    #[command(subcommand)]
    Config(config::ConfigCommand),
    /// Empty the clipboard, and keep the Daemon from restoring it.
    Clear,
    /// Search History in a panel; Enter makes the selected Clip the
    /// clipboard again, Esc closes (needs the Daemon).
    Picker,
    /// List, read, restore, delete, pin and clear Clips in History (needs
    /// the Daemon).
    #[command(subcommand)]
    History(history::HistoryCommand),
    /// Stop recording into History, for DURATION (90, 30s, 5m, 1h30m) or
    /// until `hoarder resume`. Copies still outlive their apps.
    Pause {
        #[arg(value_name = "DURATION", value_parser = pause::parse_duration)]
        duration: Option<u64>,
        #[command(flatten)]
        json: history::Json,
    },
    /// Record into History again after `hoarder pause`.
    Resume {
        #[command(flatten)]
        json: history::Json,
    },
    /// Show the Daemon's state: recording or Paused, and the clipboard.
    Status {
        #[command(flatten)]
        json: history::Json,
    },
    /// Run the Daemon in the foreground (what the service runs).
    Daemon,
    /// Install, remove or inspect the Daemon's systemd user service.
    #[command(subcommand)]
    Service(daemon::service::ServiceCommand),
    /// Check the compositor, protocols, paths, service and conflicting tools;
    /// every problem comes with a fix (exit 1 if there are any).
    Doctor,
    /// Print recommended keybindings for a compositor; `--apply` writes
    /// them to a file the compositor config includes.
    Setup {
        wm: setup::Wm,
        /// Write the bindings to Hoarder's own included file (never to the
        /// main compositor config).
        #[arg(long)]
        apply: bool,
        /// With `--apply`, replace an edited included file without asking.
        #[arg(long, requires = "apply")]
        yes: bool,
    },
    /// Print a shell completion script, e.g.
    /// `hoarder completions zsh > ~/.zfunc/_hoarder`.
    Completions { shell: dist::Shell },
    /// Write man pages, completions, the systemd user unit and the default
    /// config under DIR, for packagers.
    #[command(hide = true)]
    Dist {
        dir: PathBuf,
        /// Where the packaged binary lives (the unit's ExecStart).
        #[arg(long, value_name = "DIR", default_value = "/usr/bin")]
        bindir: PathBuf,
    },
}

/// The `screenshot.format` an `-o` file name asks for, if its extension is one.
fn format_of_path(path: &std::path::Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let format = match ext.as_str() {
        "jpg" | "jpeg" => "jpeg",
        "png" | "webp" | "avif" | "qoi" => &ext,
        _ => return None,
    };
    Some(format.to_owned())
}

/// Process exit codes (PLAN.md §10). clap exits 2 on its own usage errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Success = 0,
    /// Cancelled by the user (Esc or right-click in the Overlay), or (for
    /// `paste`) nothing to paste.
    Cancelled = 1,
    /// Bad arguments or an invalid config.
    Usage = 2,
    /// Unsupported compositor or protocol.
    Unsupported = 3,
    Io = 4,
}

impl From<Exit> for ExitCode {
    fn from(e: Exit) -> Self {
        ExitCode::from(e as u8)
    }
}

fn main() -> ExitCode {
    // The Compat shims: `wl-copy` and `wl-paste` symlinks to this binary.
    if let Some((name, result)) = clipboard::run_as_shim() {
        return report(name, result);
    }
    let mut cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Shot {
            target,
            output,
            format,
            no_save,
            no_copy,
            live,
            delay,
            no_notify,
            list,
            cursor,
        }) => {
            // Shot flags are one-run overrides, applied after any `--set`.
            let format = format.or_else(|| output.as_deref().and_then(format_of_path));
            let flags = [
                format.map(|f| ("screenshot.format", f)),
                no_save.then(|| ("screenshot.save", "false".into())),
                no_copy.then(|| ("screenshot.copy", "false".into())),
                no_notify.then(|| ("screenshot.notify", "false".into())),
                cursor.then(|| ("screenshot.cursor", "true".into())),
            ];
            cli.overrides
                .extend(flags.into_iter().flatten().map(|(k, v)| (k.into(), v)));
            config::effective(&cli.overrides).and_then(|config| {
                shot::run(
                    target,
                    list.as_ref().map(Option::as_deref),
                    shot::When { live, delay },
                    output.as_deref(),
                    &config,
                )
            })
        }
        Some(Command::Pick { format }) => {
            cli.overrides
                .extend(format.map(|f| ("pick.format".to_owned(), f)));
            config::effective(&cli.overrides).and_then(|config| pick::run(&config))
        }
        Some(Command::Copy(args)) => clipboard::copy(args),
        Some(Command::Paste(args)) => clipboard::paste(args),
        Some(Command::ServeOffer(args)) => clipboard::serve_child(args),
        Some(Command::NotifyCapture(args)) => notify::serve_child(args),
        Some(Command::Config(command)) => config::run(command, &cli.overrides),
        Some(Command::Clear) => daemon::clear(hoarder_wayland::clipboard::Selection::Clipboard),
        Some(Command::Picker) => picker::run(&cli.overrides),
        Some(Command::History(command)) => history::run(command),
        Some(Command::Pause { duration, json }) => pause::pause(duration, json.json),
        Some(Command::Resume { json }) => pause::resume(json.json),
        Some(Command::Status { json }) => pause::status(json.json),
        Some(Command::Daemon) => daemon::run(&cli.overrides),
        Some(Command::Service(command)) => daemon::service::run(command),
        Some(Command::Doctor) => doctor::run(),
        Some(Command::Setup { wm, apply, yes }) => setup::run(wm, apply, yes),
        Some(Command::Completions { shell }) => dist::completions(shell, Cli::command()),
        Some(Command::Dist { dir, bindir }) => dist::run(&dir, &bindir, Cli::command()),
        None => doctor::first_run(),
    };
    report("hoarder", result)
}

/// The exit code for `result`, with any complaint on stderr as `name: …`.
fn report(name: &str, result: Result<(), (Exit, String)>) -> ExitCode {
    match result {
        Ok(()) => Exit::Success.into(),
        Err((code, message)) => {
            eprintln!("{name}: {message}");
            code.into()
        }
    }
}
