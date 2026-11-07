mod clipboard;
mod image;
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
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Take a Capture.
    Shot {
        /// What to capture.
        target: shot::Target,
        /// Write the image to PATH, or to stdout with `-`.
        #[arg(short = 'o', long = "output", value_name = "PATH")]
        output: PathBuf,
    },
    /// Copy stdin or text to the clipboard (accepts every wl-copy flag).
    Copy(clipboard::CopyArgs),
    /// Paste the clipboard to stdout (accepts every wl-paste flag).
    Paste(clipboard::PasteArgs),
    /// Serve an Offer read from stdin; started by Hoarder itself.
    #[command(name = clipboard::SERVE_SUBCOMMAND, hide = true)]
    ServeOffer(clipboard::ServeArgs),
}

/// Process exit codes (PLAN.md §10). Usage errors (2) come from clap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Success = 0,
    /// Cancelled by the user, or (for `paste`) nothing to paste.
    Cancelled = 1,
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
        Some(Command::Shot { target, output }) => shot::run(target, &output),
        Some(Command::Copy(args)) => clipboard::copy(args),
        Some(Command::Paste(args)) => clipboard::paste(args),
        Some(Command::ServeOffer(args)) => clipboard::serve_child(args),
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
