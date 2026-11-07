mod after;
mod clipboard;
mod config;
mod encode;
mod hook;
mod image;
mod notify;
mod save;
mod shot;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

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
        /// No notification for this Capture (`screenshot.notify = false`).
        #[arg(long)]
        no_notify: bool,
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
    let mut cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Shot {
            target,
            output,
            format,
            no_save,
            no_copy,
            no_notify,
        }) => {
            // Shot flags are one-run overrides, applied after any `--set`.
            let format = format.or_else(|| output.as_deref().and_then(format_of_path));
            let flags = [
                format.map(|f| ("screenshot.format", f)),
                no_save.then(|| ("screenshot.save", "false".into())),
                no_copy.then(|| ("screenshot.copy", "false".into())),
                no_notify.then(|| ("screenshot.notify", "false".into())),
            ];
            cli.overrides
                .extend(flags.into_iter().flatten().map(|(k, v)| (k.into(), v)));
            config::effective(&cli.overrides)
                .and_then(|config| shot::run(target, output.as_deref(), &config))
        }
        Some(Command::Copy(args)) => clipboard::copy(args),
        Some(Command::Paste(args)) => clipboard::paste(args),
        Some(Command::ServeOffer(args)) => clipboard::serve_child(args),
        Some(Command::NotifyCapture(args)) => notify::serve_child(args),
        Some(Command::Config(command)) => config::run(command, &cli.overrides),
        None => Ok(()),
    };
    match result {
        Ok(()) => Exit::Success.into(),
        Err((code, message)) => {
            eprintln!("hoarder: {message}");
            code.into()
        }
    }
}
