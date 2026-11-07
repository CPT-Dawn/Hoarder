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
    // A running Daemon never reads half a file.
    hoarder_config::write_atomic(path, text)
        .map_err(|e| (Exit::Io, format!("cannot write {}: {e}", path.display())))
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
        ConfigCommand::Get { key, watch: false } => {
            print!("{}", show(&effective(overrides)?, key.as_deref())?);
        }
        ConfigCommand::Get { key, watch: true } => {
            watch_value(&path, key.as_deref(), overrides)?;
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
/// that becomes invalid is reported and the last good value stands; so does
/// an empty one, which is what an editor that truncates and then writes in
/// place leaves for a moment. Deleting the file reverts to the defaults.
fn watch_value(
    path: &Path,
    key: Option<&str>,
    overrides: &[(String, String)],
) -> Result<(), (Exit, String)> {
    use std::io::Write as _;
    let io = |e: std::io::Error| (Exit::Io, format!("cannot watch {}: {e}", path.display()));
    // Watch first, so no change between the first read and the watch is missed.
    let mut watcher = hoarder_config::Watcher::new(path).map_err(io)?;
    let mut last = show(&effective(overrides)?, key)?;
    print!("{last}");
    std::io::stdout().flush().map_err(io)?;
    loop {
        watcher.wait().map_err(io)?;
        // A file that changed while it was read may be half written: let it
        // settle and read it again.
        let reread = loop {
            let reread = reread(path, overrides);
            if !watcher.changed().map_err(io)? {
                break reread;
            }
            watcher.settle().map_err(io)?;
        };
        let config = match reread {
            Reread::Apply(config) => config,
            Reread::Empty => continue,
            Reread::Broken(e) => {
                eprintln!("hoarder: {e}");
                continue;
            }
        };
        let now = show(&config, key)?;
        if now != last {
            print!("{now}");
            std::io::stdout().flush().map_err(io)?;
            last = now;
        }
    }
}

/// What a reload of `config.toml` found. Only [`Reread::Apply`] may replace
/// the config in force; anything else keeps the last good one.
#[derive(Debug)]
pub enum Reread {
    /// A valid file (or none at all, which means the defaults), with the
    /// `--set` overrides applied.
    Apply(Box<Config>),
    /// An empty file: far more likely an in-place save caught between its
    /// truncate and its write than a wish for the defaults.
    Empty,
    /// A file that cannot be read or does not parse, or an override that no
    /// longer applies; the message says why, naming the file and line.
    Broken(String),
}

/// Re-read the config file for live reload. Every reload path (the
/// Daemon's `reload` op and its inotify watch, `config get --watch`) goes
/// through here, so none of them trades a half-saved file for the defaults.
pub fn reread(path: &Path, overrides: &[(String, String)]) -> Reread {
    let text = match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => return Reread::Empty,
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Reread::Broken(format!("cannot read {}: {e}", path.display())),
    };
    if let Err(d) = hoarder_config::parse(&text) {
        return Reread::Broken(format!("{}:{d}", path.display()));
    }
    match hoarder_config::with_overrides(&text, overrides) {
        Ok(config) => Reread::Apply(Box::new(config)),
        Err(e) => Reread::Broken(format!("--set {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reread_text(text: Option<&str>) -> Reread {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        if let Some(text) = text {
            std::fs::write(&path, text).unwrap();
        }
        reread(&path, &[])
    }

    #[test]
    fn an_empty_file_is_a_save_in_progress_not_the_defaults() {
        assert!(matches!(reread_text(Some("")), Reread::Empty));
        assert!(matches!(reread_text(Some(" \n\n")), Reread::Empty));
    }

    #[test]
    fn a_broken_file_says_where() {
        let Reread::Broken(why) = reread_text(Some("[clipboard]\npersist = \"maybe\"\n")) else {
            panic!("a broken file must not apply");
        };
        assert!(why.contains("config.toml:2"), "{why}");
    }

    #[test]
    fn a_good_file_applies_and_a_missing_one_means_the_defaults() {
        let Reread::Apply(config) = reread_text(Some("[clipboard]\npersist = false\n")) else {
            panic!("a good file applies");
        };
        assert!(!config.clipboard.persist);
        let Reread::Apply(config) = reread_text(None) else {
            panic!("no file is the defaults");
        };
        assert_eq!(*config, Config::default());
    }

    #[test]
    fn an_override_that_no_longer_applies_keeps_the_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[clipboard]\npersist = false\n").unwrap();
        let overrides = [("clipboard.nope".to_owned(), "1".to_owned())];
        assert!(matches!(reread(&path, &overrides), Reread::Broken(_)));
    }
}
