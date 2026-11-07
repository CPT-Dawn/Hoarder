//! Typed configuration schema, defaults, validation and live reload for Hoarder
//! (PLAN.md §9).
//!
//! - [`Config`] is the typed schema; [`Config::default`] is what an empty or
//!   missing file means.
//! - [`parse`] validates a file and reports problems with a line and column.
//! - [`load`] reads `config.toml` from [`path`].
//! - [`set`] and [`reset`] edit the file's text without losing comments.
//! - [`Watcher`] tells when the file changed, for live reload.

mod edit;
mod schema;
mod template;
mod watch;

use std::fmt;
use std::path::{Path, PathBuf};

pub use edit::{EditError, get, reset, set};
pub use schema::*;
pub use template::{NameFields, NameTemplate};
pub use watch::Watcher;

/// The file `config reset` writes: every setting at its default, documented.
pub const DEFAULT_FILE: &str = include_str!("default.toml");

/// `$XDG_CONFIG_HOME/hoarder/config.toml`, or `~/.config/hoarder/config.toml`.
pub fn path() -> PathBuf {
    config_home().join("hoarder").join("config.toml")
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn config_home() -> PathBuf {
    match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home().join(".config"),
    }
}

/// A problem in a config file, at a 1-based line and column (in characters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl Diagnostic {
    fn at(text: &str, offset: usize, message: String) -> Self {
        let before = &text[..offset.min(text.len())];
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        Diagnostic {
            line: before.matches('\n').count() + 1,
            column: before[line_start..].chars().count() + 1,
            message,
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.column, self.message)
    }
}

impl std::error::Error for Diagnostic {}

/// Validate a config file and turn it into a [`Config`]. Missing keys take
/// their defaults; unknown keys and bad values are errors.
pub fn parse(text: &str) -> Result<Config, Diagnostic> {
    toml_edit::de::from_str(text).map_err(|e| {
        let offset = e.span().map_or(0, |s| s.start);
        Diagnostic::at(text, offset, e.message().trim_end().to_owned())
    })
}

/// Why a config file could not be loaded.
#[derive(Debug)]
pub enum LoadError {
    Io(PathBuf, std::io::Error),
    Invalid(PathBuf, Diagnostic),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            LoadError::Io(p, e) => write!(f, "{}: {e}", p.display()),
            LoadError::Invalid(p, d) => write!(f, "{}:{d}", p.display()),
        }
    }
}

impl std::error::Error for LoadError {}

/// Read and validate the file at `path`. A missing file is the default config.
pub fn load(path: &Path) -> Result<Config, LoadError> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text).map_err(|d| LoadError::Invalid(path.into(), d)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(LoadError::Io(path.into(), e)),
    }
}

/// Expand `~` and `$VAR`/`${VAR}` in a directory setting. `$XDG_PICTURES_DIR`
/// falls back to `user-dirs.dirs`, then to `~/Pictures`, like `xdg-user-dir`.
pub fn expand_dir(dir: &str) -> Result<PathBuf, String> {
    let mut out = String::new();
    let mut rest = dir;
    if let Some(r) = rest.strip_prefix('~')
        && (r.is_empty() || r.starts_with('/'))
    {
        out.push_str(&home().to_string_lossy());
        rest = r;
    }
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let (name, tail) = if let Some(braced) = after.strip_prefix('{') {
            let end = braced
                .find('}')
                .ok_or_else(|| format!("unclosed `${{` in {dir:?}"))?;
            (&braced[..end], &braced[end + 1..])
        } else {
            let end = after
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            (&after[..end], &after[end..])
        };
        out.push_str(&env_lookup(name).ok_or_else(|| format!("${name} is not set (in {dir:?})"))?);
        rest = tail;
    }
    out.push_str(rest);
    Ok(PathBuf::from(out))
}

fn env_lookup(name: &str) -> Option<String> {
    if let Some(v) = std::env::var_os(name).filter(|v| !v.is_empty()) {
        return Some(v.to_string_lossy().into_owned());
    }
    match name {
        "HOME" => Some(home().to_string_lossy().into_owned()),
        "XDG_PICTURES_DIR" => Some(
            user_dir("XDG_PICTURES_DIR")
                .unwrap_or_else(|| home().join("Pictures"))
                .to_string_lossy()
                .into_owned(),
        ),
        _ => None,
    }
}

/// A directory from `$XDG_CONFIG_HOME/user-dirs.dirs` (`NAME="$HOME/…"`).
fn user_dir(name: &str) -> Option<PathBuf> {
    let text = std::fs::read_to_string(config_home().join("user-dirs.dirs")).ok()?;
    text.lines().find_map(|line| {
        let value = line.trim().strip_prefix(name)?.strip_prefix('=')?;
        let value = value.trim().trim_matches('"');
        let path = match value.strip_prefix("$HOME") {
            Some(rest) => home().join(rest.trim_start_matches('/')),
            None if value.starts_with('/') => PathBuf::from(value),
            None => return None,
        };
        Some(path)
    })
}

/// The file text with one-run overrides (`KEY=VALUE`, as for [`set`]) applied,
/// validated as a whole. The file itself is not touched.
pub fn with_overrides(text: &str, overrides: &[(String, String)]) -> Result<Config, EditError> {
    let mut text = text.to_owned();
    for (key, value) in overrides {
        text = set(&text, key, value)?;
    }
    parse(&text).map_err(EditError::Syntax)
}
