//! `hoarder history list|get|restore|delete [--json]`: History, read and
//! changed through the Daemon's socket (PLAN.md §10, §11).

use std::io::Write as _;

use clap::{Args, Subcommand};
use hoarder_ipc::{Connection, Failure, Op};

use crate::Exit;

type Fail = (Exit, String);

#[derive(Debug, Subcommand)]
pub enum HistoryCommand {
    /// One Clip per line, newest first: its id, a tab, and a preview.
    List {
        /// Show at most this many Clips.
        #[arg(long, short = 'n')]
        limit: Option<u32>,
        #[command(flatten)]
        json: Json,
    },
    /// Write a Clip to stdout (its first type unless `--type` says).
    Get {
        #[command(flatten)]
        clip: ClipArg,
        /// The MIME type to write.
        #[arg(long, short = 't', value_name = "MIME")]
        r#type: Option<String>,
        /// Print the Clip's details as JSON instead of its data.
        #[command(flatten)]
        json: Json,
    },
    /// Make a Clip the clipboard again.
    Restore {
        #[command(flatten)]
        clip: ClipArg,
        #[command(flatten)]
        json: Json,
    },
    /// Remove a Clip from History.
    Delete {
        #[command(flatten)]
        clip: ClipArg,
        #[command(flatten)]
        json: Json,
    },
}

#[derive(Debug, Args)]
pub struct Json {
    /// Print machine-readable JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
pub struct ClipArg {
    /// The Clip's id; a whole `history list` line works too.
    #[arg(value_name = "ID", value_parser = clip_id)]
    id: i64,
}

/// The leading number of `arg`, so `12<TAB>preview` picks Clip 12.
fn clip_id(arg: &str) -> Result<i64, String> {
    let digits = arg.trim_start().split(|c: char| !c.is_ascii_digit()).next();
    digits
        .and_then(|d| d.parse().ok())
        .ok_or_else(|| format!("{arg:?} does not start with a Clip id"))
}

pub fn run(command: HistoryCommand) -> Result<(), Fail> {
    let mut daemon = Connection::connect().map_err(|_| {
        (
            Exit::Io,
            "the Daemon is not running, and History lives in it; start it with \
             `hoarder service enable`"
                .to_owned(),
        )
    })?;
    match command {
        HistoryCommand::List { limit, json } => {
            let clips = answer(daemon.list(limit))?;
            if json.json {
                return print_json(&clips);
            }
            let mut out = String::new();
            for clip in clips {
                out.push_str(&format!("{}\t{}\n", clip.id, clip.preview));
            }
            write_stdout(out.as_bytes())
        }
        HistoryCommand::Get { clip, r#type, json } => {
            let (got, mut data) = answer(daemon.get(clip.id, r#type))?;
            if json.json {
                return print_json(&got);
            }
            let mut stdout = std::io::stdout().lock();
            std::io::copy(&mut data, &mut stdout)
                .and_then(|_| stdout.flush())
                .map_err(|e| (Exit::Io, format!("cannot write the Clip: {e}")))
        }
        HistoryCommand::Restore { clip, json } => {
            let clip = answer(daemon.restore(clip.id))?;
            if json.json { print_json(&clip) } else { Ok(()) }
        }
        HistoryCommand::Delete { clip, json } => {
            answer(daemon.call(Op::Delete { clip: clip.id }))?;
            if json.json {
                print_json(&serde_json::json!({"id": clip.id, "deleted": true}))
            } else {
                Ok(())
            }
        }
    }
}

/// The Daemon's answer, or why there is none.
fn answer<T>(response: std::io::Result<Result<T, Failure>>) -> Result<T, Fail> {
    match response {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(failure)) => Err((Exit::Io, failure.message)),
        Err(e) => Err((Exit::Io, format!("lost the Daemon: {e}"))),
    }
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<(), Fail> {
    let mut line = serde_json::to_string(value).expect("JSON values serialise");
    line.push('\n');
    write_stdout(line.as_bytes())
}

fn write_stdout(bytes: &[u8]) -> Result<(), Fail> {
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(bytes)
        .and_then(|()| stdout.flush())
        .map_err(|e| (Exit::Io, format!("cannot write to stdout: {e}")))
}
