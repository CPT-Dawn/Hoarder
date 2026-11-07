//! `hoarder config …` and the effective config every other command runs with
//! (PLAN.md §9).

use std::path::Path;

use clap::Subcommand;
use hoarder_config::{Config, EditError};

use crate::Exit;

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the path of the config file.
    Path,
    /// Print the effective value of a setting, a section, or everything.
    Get {
        /// A dotted key such as `screenshot.format`, or a section such as `screenshot`.
        key: Option<String>,
        /// Keep running and print the value again whenever it changes.
        #[arg(long)]
        watch: bool,
    },
    /// Change a setting in the file, keeping every comment.
    Set {
        /// A dotted key such as `screenshot.format`.
        key: String,
        /// A TOML value (`true`, `90`, `["a"]`); bare words are taken as strings.
        value: String,
    },
    /// Put a setting back to its default; with no key, rewrite the whole file.
    Reset { key: Option<String> },
    /// Validate the file; problems are printed as `PATH:LINE:COLUMN: message`
    /// and exit with code 2.
    Check,
    /// Open the file in `$VISUAL` or `$EDITOR`, then check it.
    Edit,
}

/// A `--set KEY=VALUE` argument.
pub fn parse_override(arg: &str) -> Result<(String, String), String> {
    arg.split_once('=')
        .map(|(k, v)| (k.trim().to_owned(), v.to_owned()))
        .ok_or_else(|| format!("expected KEY=VALUE, got {arg:?}"))
}

/// The config file with `overrides` applied. A broken file only costs a
/// warning and the defaults (a typo must not cost the user a Capture); a bad
/// override is a usage error.
pub fn effective(overrides: &[(String, String)]) -> Result<Config, (Exit, String)> {
    let path = hoarder_config::path();
    let text = match read(&path) {
        Ok(text) => match hoarder_config::parse(&text) {
            Ok(_) => text,
            Err(d) => {
                eprintln!("hoarder: {}:{d}; using the defaults", path.display());
                String::new()
            }
        },
        Err(e) => {
            eprintln!("hoarder: {e}; using the defaults");
            String::new()
        }
    };
    hoarder_config::with_overrides(&text, overrides)
        .map_err(|e| (Exit::Usage, format!("--set {e}")))
}

/// The file's text; a missing file reads as empty.
fn read(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// The file's text, or the commented default file if there is none yet.
fn read_or_template(path: &Path) -> Result<String, (Exit, String)> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(hoarder_config::DEFAULT_FILE.to_owned())
        }
        Err(e) => Err((Exit::Io, format!("{}: {e}", path.display()))),
    }
}

fn write(path: &Path, text: &str) -> Result<(), (Exit, String)> {
    let io = |e: std::io::Error| (Exit::Io, format!("cannot write {}: {e}", path.display()));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    // Write a sibling and rename it over the file, so a running process never
    // reads half a file.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}

fn edit_error(path: &Path, e: EditError) -> (Exit, String) {
    let message = match e {
        EditError::Syntax(d) => format!("{}:{d}", path.display()),
        e => e.to_string(),
    };
    (Exit::Usage, message)
}

pub fn run(command: ConfigCommand, overrides: &[(String, String)]) -> Result<(), (Exit, String)> {
    let path = hoarder_config::path();
    match command {
        ConfigCommand::Path => println!("{}", path.display()),
        ConfigCommand::Get { key, watch } => {
            let mut last = show(&effective(overrides)?, key.as_deref())?;
            print!("{last}");
            if watch {
                watch_value(&path, key.as_deref(), overrides, &mut last)?;
            }
        }
        ConfigCommand::Set { key, value } => {
            let text = read_or_template(&path)?;
            let text =
                hoarder_config::set(&text, &key, &value).map_err(|e| edit_error(&path, e))?;
            write(&path, &text)?;
        }
        ConfigCommand::Reset { key: Some(key) } => {
            let text = read_or_template(&path)?;
            let text = hoarder_config::reset(&text, &key).map_err(|e| edit_error(&path, e))?;
            write(&path, &text)?;
        }
        ConfigCommand::Reset { key: None } => write(&path, hoarder_config::DEFAULT_FILE)?,
        ConfigCommand::Check => check(&path)?,
        ConfigCommand::Edit => {
            if !path.exists() {
                write(&path, hoarder_config::DEFAULT_FILE)?;
            }
            let editor = ["VISUAL", "EDITOR"]
                .iter()
                .find_map(|v| std::env::var(v).ok().filter(|e| !e.is_empty()))
                .unwrap_or_else(|| "vi".into());
            // Through the shell, so `EDITOR="code --wait"` works.
            let status = std::process::Command::new("sh")
                .args(["-c", &format!("{editor} \"$1\""), "sh"])
                .arg(&path)
                .status()
                .map_err(|e| (Exit::Io, format!("cannot run {editor}: {e}")))?;
            if !status.success() {
                return Err((Exit::Io, format!("{editor} exited with {status}")));
            }
            check(&path)?;
        }
    }
    Ok(())
}

fn check(path: &Path) -> Result<(), (Exit, String)> {
    let text = read(path).map_err(|e| (Exit::Io, e))?;
    hoarder_config::parse(&text)
        .map(drop)
        .map_err(|d| (Exit::Usage, format!("{}:{d}", path.display())))
}

fn show(config: &Config, key: Option<&str>) -> Result<String, (Exit, String)> {
    hoarder_config::get(config, key).ok_or_else(|| {
        (
            Exit::Usage,
            format!("unknown setting `{}`", key.unwrap_or_default()),
        )
    })
}

/// `get --watch`: print the value again each time an edit changes it. A file
/// that becomes invalid is reported and the last good value stands.
fn watch_value(
    path: &Path,
    key: Option<&str>,
    overrides: &[(String, String)],
    last: &mut String,
) -> Result<(), (Exit, String)> {
    use std::io::Write as _;
    let io = |e: std::io::Error| (Exit::Io, format!("cannot watch {}: {e}", path.display()));
    let mut watcher = hoarder_config::Watcher::new(path).map_err(io)?;
    std::io::stdout().flush().map_err(io)?;
    loop {
        watcher.wait().map_err(io)?;
        let text = read(path).map_err(|e| (Exit::Io, e))?;
        let config = match hoarder_config::parse(&text) {
            Ok(_) => match hoarder_config::with_overrides(&text, overrides) {
                Ok(config) => config,
                Err(e) => {
                    eprintln!("hoarder: {e}");
                    continue;
                }
            },
            Err(d) => {
                eprintln!("hoarder: {}:{d}", path.display());
                continue;
            }
        };
        let now = show(&config, key)?;
        if now != *last {
            print!("{now}");
            std::io::stdout().flush().map_err(io)?;
            *last = now;
        }
    }
}
