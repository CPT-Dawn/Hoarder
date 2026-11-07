//! Editing a config file as text: every comment, blank line and bit of
//! formatting the user wrote survives, except the value being replaced.

use std::fmt;

use toml_edit::{DocumentMut, Item, Table, Value};

use crate::{Config, Diagnostic, parse};

/// Why an edit was refused. The file is left as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// The file is not valid TOML, so there is nothing safe to edit.
    Syntax(Diagnostic),
    /// The key is not a setting (or names a table where a value is needed).
    UnknownKey(String),
    /// The edit would leave the config invalid; the position is in the edited text.
    Invalid { key: String, problem: Diagnostic },
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            EditError::Syntax(d) => write!(f, "the file is not valid TOML: {d}"),
            EditError::UnknownKey(k) => write!(f, "unknown setting `{k}`"),
            EditError::Invalid { key, problem } => {
                write!(f, "{key}: {}", problem.message)
            }
        }
    }
}

impl std::error::Error for EditError {}

fn split_key(key: &str) -> Result<Vec<&str>, EditError> {
    let path: Vec<&str> = key.split('.').collect();
    if path.iter().any(|p| p.is_empty()) {
        return Err(EditError::UnknownKey(key.into()));
    }
    Ok(path)
}

fn document(text: &str) -> Result<DocumentMut, EditError> {
    text.parse::<DocumentMut>().map_err(|e| {
        let offset = e.span().map_or(0, |s| s.start);
        EditError::Syntax(Diagnostic::at(text, offset, e.message().trim_end().into()))
    })
}

/// Whether `key` names a setting (a leaf value) of the schema.
fn is_setting(key: &[&str]) -> bool {
    get_item(&Config::default(), key).is_some_and(|item| item.is_value())
}

/// Set `key` (dotted, like `screenshot.format`) to `value` in the config file
/// `text` and return the new file text.
///
/// `value` is read as a TOML value (`true`, `90`, `["a", "b"]`, `"quoted"`);
/// if that is not valid for the key, it is taken as a plain string, so
/// `set screenshot.format webp` works without quotes.
pub fn set(text: &str, key: &str, value: &str) -> Result<String, EditError> {
    let path = split_key(key)?;
    if !is_setting(&path) {
        return Err(EditError::UnknownKey(key.into()));
    }
    let doc = document(text)?;
    let as_toml = value.parse::<Value>().ok();
    let candidates = as_toml.into_iter().chain([Value::from(value)]);
    let mut first_problem = None;
    for v in candidates {
        let mut edited = doc.clone();
        set_value(&mut edited, &path, v);
        let out = edited.to_string();
        match parse(&out) {
            Ok(_) => return Ok(out),
            Err(problem) => {
                first_problem.get_or_insert(problem);
            }
        }
    }
    Err(EditError::Invalid {
        key: key.into(),
        problem: first_problem.expect("at least one candidate"),
    })
}

/// Set `key` back to its default value, keeping comments and formatting.
pub fn reset(text: &str, key: &str) -> Result<String, EditError> {
    let path = split_key(key)?;
    let value = default_value(&path).ok_or_else(|| EditError::UnknownKey(key.into()))?;
    let mut doc = document(text)?;
    set_value(&mut doc, &path, value);
    Ok(doc.to_string())
}

fn default_value(path: &[&str]) -> Option<Value> {
    get_item(&Config::default(), path)?.as_value().cloned()
}

/// The value of `key` in `config` as `hoarder config get` prints it: a string
/// bare, any other value as TOML, a section (or, with no key, everything) as
/// TOML tables. Ends with a newline; `None` for an unknown key.
pub fn get(config: &Config, key: Option<&str>) -> Option<String> {
    let path = match key {
        Some(key) => split_key(key).ok()?,
        None => Vec::new(),
    };
    Some(match get_item(config, &path)? {
        Item::Value(Value::String(s)) => format!("{}\n", s.value()),
        Item::Value(v) => format!("{}\n", v.decorated("", "")),
        Item::Table(t) => DocumentMut::from(t).to_string(),
        other => format!("{other}\n"),
    })
}

/// `config` as a document of `[standard]` tables, then the item at `path`.
fn get_item(config: &Config, path: &[&str]) -> Option<Item> {
    let mut doc = toml_edit::ser::to_document(config).expect("serialisable");
    expand_inline_tables(doc.as_table_mut());
    let mut item = doc.as_item();
    for part in path {
        item = item.get(part)?;
    }
    Some(item.clone())
}

/// The serialiser writes nested structs as `{ inline = tables }`; turn them
/// into `[section]`s, as a person would write them.
fn expand_inline_tables(table: &mut Table) {
    for (_, item) in table.iter_mut() {
        if let Some(inline) = item.as_inline_table() {
            let mut t = inline.clone().into_table();
            t.set_implicit(true);
            *item = Item::Table(t);
        }
        if let Some(t) = item.as_table_mut() {
            expand_inline_tables(t);
        }
    }
}

/// Put `v` at `path`, creating `[standard.tables]` (never inline ones) on the
/// way and keeping the old value's decor so its trailing comment survives.
/// `insert` on an existing key would drop the comments above it, so existing
/// slots are overwritten in place.
fn set_value(doc: &mut DocumentMut, path: &[&str], mut v: Value) {
    let (last, parents) = path.split_last().expect("non-empty key");
    let mut item: &mut Item = doc.as_item_mut();
    for p in parents {
        if item.as_table_like().is_none() {
            // A scalar where a table belongs: the edit is invalid anyway, and
            // validation reports it; just give the key somewhere to live.
            *item = Item::Table(Table::new());
        }
        let table = item.as_table_like_mut().expect("a table");
        if !table.contains_key(p) {
            let mut new = Table::new();
            new.set_implicit(true);
            table.insert(p, Item::Table(new));
        }
        item = table.get_mut(p).expect("just ensured");
    }
    if item.as_table_like().is_none() {
        *item = Item::Table(Table::new());
    }
    let table = item.as_table_like_mut().expect("a table");
    match table.get_mut(last) {
        Some(slot) => {
            if let Some(old) = slot.as_value() {
                *v.decor_mut() = old.decor().clone();
            }
            *slot = Item::Value(v);
        }
        None => {
            table.insert(last, Item::Value(v));
        }
    }
}
