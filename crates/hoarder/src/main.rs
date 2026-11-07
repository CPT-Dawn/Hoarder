mod after;
mod clipboard;
mod config;
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
        target: shot::Target,
        /// Write the image to PATH, or to stdout with `-`, instead of saving it
        /// to `screenshot.dir`.
        #[arg(short = 'o', long = "output", value_name = "PATH")]
        output: Option<PathBuf>,
        /// Leave the clipboard alone (`screenshot.copy = false` for this run).
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

/// Process exit codes (PLAN.md §10). clap exits 2 on its own usage errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Success = 0,
    /// Cancelled by the user, or (for `paste`) nothing to paste.
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
    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Shot {
            target,
            output,
            no_copy,
            no_notify,
        }) => config::effective(&cli.overrides).and_then(|mut config| {
            config.screenshot.copy &= !no_copy;
            config.screenshot.notify &= !no_notify;
            shot::run(target, output.as_deref(), &config)
        }),
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
