//! What Hoarder reads of Zenith's own config, to follow its theme (PLAN.md §8).

use std::path::{Path, PathBuf};

/// `$XDG_CONFIG_HOME/zenith-bar`, where Zenith keeps `config.toml` and
/// `style.css`.
pub fn zenith_dir() -> PathBuf {
    crate::config_home().join("zenith-bar")
}

/// The `[theme]` settings of Zenith's `config.toml` that shape its look.
/// Anything missing, mistyped or unparsable is `None`: a broken Zenith
/// config is never Hoarder's error.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ZenithTheme {
    /// Zenith's built-in palette, e.g. `tokyo-night`.
    pub name: Option<String>,
    /// A CSS colour.
    pub accent: Option<String>,
    pub surface_opacity: Option<f64>,
    /// In logical pixels.
    pub module_radius: Option<i64>,
}

impl ZenithTheme {
    /// Read `config.toml` in Zenith's config directory `dir`.
    pub fn read(dir: &Path) -> ZenithTheme {
        std::fs::read_to_string(dir.join("config.toml"))
            .map(|text| ZenithTheme::parse(&text))
            .unwrap_or_default()
    }

    /// The `[theme]` settings in `text`, the contents of a Zenith config.
    pub fn parse(text: &str) -> ZenithTheme {
        let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else {
            return ZenithTheme::default();
        };
        let setting = |key: &str| doc.get("theme")?.get(key);
        ZenithTheme {
            name: setting("name").and_then(|v| v.as_str()).map(str::to_owned),
            accent: setting("accent")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            surface_opacity: setting("surface_opacity")
                .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))),
            module_radius: setting("module_radius").and_then(|v| v.as_integer()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_theme_table_and_shrugs_off_the_rest() {
        let text = "[bar]\nheight = 30\n[theme]\nname = \"tokyo-night\"\naccent = \"#ff0000\"\nsurface_opacity = 1\nmodule_radius = 4\n";
        assert_eq!(
            ZenithTheme::parse(text),
            ZenithTheme {
                name: Some("tokyo-night".into()),
                accent: Some("#ff0000".into()),
                surface_opacity: Some(1.0),
                module_radius: Some(4),
            }
        );
        assert_eq!(ZenithTheme::parse("[theme\nbroken"), ZenithTheme::default());
        assert_eq!(
            ZenithTheme::parse("[theme]\nmodule_radius = \"big\""),
            ZenithTheme::default()
        );
    }
}
