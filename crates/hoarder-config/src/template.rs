//! `screenshot.name`: the file name template (PLAN.md §4.5).
//!
//! `{%…}` is a strftime format of the Capture time; `{target}`, `{app}`, `{w}`
//! and `{h}` are the Target kind, the Window's app id (empty for other
//! Targets) and the image size in pixels. Everything else is literal.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameTemplate {
    source: String,
    parts: Vec<Part>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Literal(String),
    Time(String),
    Target,
    App,
    Width,
    Height,
}

/// What a name is made from: one Capture.
#[derive(Debug, Clone)]
pub struct NameFields<'a> {
    pub time: &'a jiff::Zoned,
    /// `region`, `window`, `output` or `all`.
    pub target: &'a str,
    pub app: Option<&'a str>,
    pub width: u32,
    pub height: u32,
}

impl NameTemplate {
    pub fn parse(source: &str) -> Result<Self, String> {
        let mut parts = Vec::new();
        let mut rest = source;
        while let Some(open) = rest.find('{') {
            if open > 0 {
                parts.push(Part::Literal(rest[..open].into()));
            }
            let close = rest[open..]
                .find('}')
                .ok_or_else(|| format!("unclosed `{{` in name template {source:?}"))?;
            let token = &rest[open + 1..open + close];
            parts.push(match token {
                "target" => Part::Target,
                "app" => Part::App,
                "w" => Part::Width,
                "h" => Part::Height,
                t if t.starts_with('%') => {
                    // Reject bad strftime directives now rather than at Capture time.
                    let sample = jiff::Timestamp::UNIX_EPOCH.to_zoned(jiff::tz::TimeZone::UTC);
                    jiff::fmt::strtime::format(t, &sample)
                        .map_err(|e| format!("bad time format {{{t}}}: {e}"))?;
                    Part::Time(t.into())
                }
                t => {
                    return Err(format!(
                        "unknown token {{{t}}} in name template; \
                         use {{%…}}, {{target}}, {{app}}, {{w}} or {{h}}"
                    ));
                }
            });
            rest = &rest[open + close + 1..];
        }
        if !rest.is_empty() {
            parts.push(Part::Literal(rest.into()));
        }
        if parts.is_empty() {
            return Err("the name template is empty".into());
        }
        Ok(NameTemplate {
            source: source.into(),
            parts,
        })
    }

    /// The file name stem for `fields`. A `/` from any token becomes `_`, so
    /// the name never leaves `screenshot.dir`.
    pub fn expand(&self, fields: &NameFields) -> String {
        let mut out = String::new();
        for part in &self.parts {
            let piece = match part {
                Part::Literal(s) => s.clone(),
                Part::Time(f) => jiff::fmt::strtime::format(f, fields.time).unwrap_or_default(),
                Part::Target => fields.target.into(),
                Part::App => fields.app.unwrap_or_default().into(),
                Part::Width => fields.width.to_string(),
                Part::Height => fields.height.to_string(),
            };
            out.extend(piece.chars().map(|c| match c {
                '/' | '\0' => '_',
                c => c,
            }));
        }
        out
    }

    pub fn as_str(&self) -> &str {
        &self.source
    }
}

impl Default for NameTemplate {
    fn default() -> Self {
        NameTemplate::parse("Hoarder_{%Y-%m-%d_%H-%M-%S}").expect("valid default")
    }
}

impl fmt::Display for NameTemplate {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.source)
    }
}

impl Serialize for NameTemplate {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.source)
    }
}

impl<'de> Deserialize<'de> for NameTemplate {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        NameTemplate::parse(&s).map_err(serde::de::Error::custom)
    }
}
