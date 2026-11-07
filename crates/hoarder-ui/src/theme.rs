//! Theme tokens (PLAN.md §8): the colours, radius and fonts the Overlay and
//! Picker draw with.
//!
//! [`Theme::load`] resolves `[theme]` from the config. With `name = "zenith"`
//! it rebuilds Zenith's cascade from disk on every call, so each new Overlay
//! or Picker wears whatever Zenith wears right now:
//!
//! 1. the built-in palette named by Zenith's `[theme] name`;
//! 2. Zenith's own `[theme]` overrides (`accent`, `surface_opacity`,
//!    `module_radius`);
//! 3. the `:root` `--zb-*` variables in Zenith's `style.css`;
//!
//! and then, for every theme, Hoarder's own `[theme]` overrides on top.
//! Anything missing or unreadable is skipped, so no Zenith at all means
//! Midnight Bloom, Zenith's default.

use std::collections::HashMap;
use std::path::Path;

use hoarder_config::{Colour, ThemeName, ZenithTheme, zenith_dir};

/// Resolved theme tokens.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// The signature colour: selection border, focus.
    pub accent: Colour,
    /// Panels and pills, with their opacity in the alpha.
    pub surface: Colour,
    pub text: Colour,
    pub muted: Colour,
    pub dim: Colour,
    /// Corner radius in logical pixels.
    pub radius: f32,
    /// A font stack, like CSS `font-family`.
    pub font: String,
    pub mono_font: String,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::builtin(Builtin::MidnightBloom)
    }
}

/// The palettes Hoarder ships, matching Zenith's built-in themes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    MidnightBloom,
    TokyoNight,
    CatppuccinMocha,
    GruvboxDark,
}

impl Builtin {
    fn from_name(name: &str) -> Option<Builtin> {
        Some(match name {
            "midnight-bloom" => Builtin::MidnightBloom,
            "tokyo-night" => Builtin::TokyoNight,
            "catppuccin-mocha" => Builtin::CatppuccinMocha,
            "gruvbox-dark" => Builtin::GruvboxDark,
            _ => return None,
        })
    }
}

impl Theme {
    /// A built-in palette (the values of Zenith's theme of the same name).
    pub fn builtin(which: Builtin) -> Theme {
        let hex = |s: &str| s.parse::<Colour>().expect("built-in colour");
        let (accent, surface, opacity, text, muted, dim, radius) = match which {
            Builtin::MidnightBloom => (
                "#8de3dc", "#091113", 0.78, "#e4f2f0", "#9db4b5", "#5d7377", 10.0,
            ),
            Builtin::TokyoNight => (
                "#7aa2f7", "#1a1b26", 0.72, "#c0caf5", "#8a98bc", "#565f89", 8.0,
            ),
            Builtin::CatppuccinMocha => (
                "#cba6f7", "#1e1e2e", 0.85, "#cdd6f4", "#a6adc8", "#6c7086", 10.0,
            ),
            Builtin::GruvboxDark => (
                "#8ec07c", "#1d2021", 0.9, "#ebdbb2", "#a89984", "#7c6f64", 6.0,
            ),
        };
        Theme {
            accent: hex(accent),
            surface: hex(surface).with_alpha(opacity),
            text: hex(text),
            muted: hex(muted),
            dim: hex(dim),
            radius,
            font: "Inter, sans-serif".into(),
            mono_font: "monospace".into(),
        }
    }

    /// The theme `config` asks for, reading Zenith's files under
    /// `$XDG_CONFIG_HOME/zenith-bar` when it follows Zenith.
    pub fn load(config: &hoarder_config::Theme) -> Theme {
        Theme::load_from(config, &zenith_dir())
    }

    /// [`Theme::load`] with Zenith's config directory given.
    pub fn load_from(config: &hoarder_config::Theme, zenith_dir: &Path) -> Theme {
        let base = match config.name {
            ThemeName::Zenith => Theme::zenith(zenith_dir),
            ThemeName::MidnightBloom | ThemeName::Custom => Theme::builtin(Builtin::MidnightBloom),
            ThemeName::TokyoNight => Theme::builtin(Builtin::TokyoNight),
            ThemeName::CatppuccinMocha => Theme::builtin(Builtin::CatppuccinMocha),
            ThemeName::GruvboxDark => Theme::builtin(Builtin::GruvboxDark),
        };
        base.overridden_by(config)
    }

    fn overridden_by(mut self, c: &hoarder_config::Theme) -> Theme {
        for (slot, value) in [
            (&mut self.accent, c.accent),
            (&mut self.surface, c.surface),
            (&mut self.text, c.text),
            (&mut self.muted, c.muted),
            (&mut self.dim, c.dim),
        ] {
            if let Some(v) = value {
                *slot = v;
            }
        }
        if let Some(r) = c.radius {
            self.radius = r as f32;
        }
        for (slot, value) in [
            (&mut self.font, &c.font),
            (&mut self.mono_font, &c.mono_font),
        ] {
            if !value.trim().is_empty() {
                slot.clone_from(value);
            }
        }
        self
    }

    /// Zenith's cascade, as described in the module docs.
    fn zenith(dir: &Path) -> Theme {
        let config = ZenithTheme::read(dir);
        let base = (config.name.as_deref())
            .and_then(Builtin::from_name)
            .unwrap_or(Builtin::MidnightBloom);
        let mut vars = Variables::of(&Theme::builtin(base));
        if let Some(accent) = &config.accent {
            vars.set("--zb-accent", accent);
        }
        if let Some(o) = config.surface_opacity {
            vars.set("--zb-surface-opacity", &o.to_string());
        }
        if let Some(r) = config.module_radius {
            vars.set("--zb-module-radius", &format!("{r}px"));
        }
        if let Ok(css) = std::fs::read_to_string(dir.join("style.css")) {
            for (name, value) in root_variables(&css) {
                vars.set(&name, &value);
            }
        }
        vars.theme(Theme::builtin(base))
    }
}

/// CSS custom properties, with `var()` references resolved as they are set.
struct Variables(HashMap<String, String>);

impl Variables {
    fn of(theme: &Theme) -> Variables {
        let opaque = Colour {
            a: 255,
            ..theme.surface
        };
        let opacity = f32::from(theme.surface.a) / 255.0;
        let mut vars = Variables(HashMap::new());
        for (name, value) in [
            ("--zb-accent", theme.accent.to_string()),
            ("--zb-surface-color", opaque.to_string()),
            ("--zb-surface-opacity", opacity.to_string()),
            ("--zb-text", theme.text.to_string()),
            ("--zb-text-muted", theme.muted.to_string()),
            ("--zb-text-dim", theme.dim.to_string()),
            ("--zb-module-radius", format!("{}px", theme.radius)),
            ("--zb-font-family", theme.font.clone()),
            ("--zb-mono", theme.mono_font.clone()),
        ] {
            vars.0.insert(name.into(), value);
        }
        vars
    }

    fn set(&mut self, name: &str, value: &str) {
        if let Some(v) = self.resolve(value) {
            self.0.insert(name.to_owned(), v);
        }
    }

    /// `value` with every `var(--name[, fallback])` replaced; `None` if one
    /// names an unknown variable and has no fallback.
    fn resolve(&self, value: &str) -> Option<String> {
        let mut out = String::new();
        let mut rest = value;
        while let Some(i) = rest.find("var(") {
            out.push_str(&rest[..i]);
            let after = &rest[i + 4..];
            let end = matching_paren(after)?;
            let inner = &after[..end];
            let (name, fallback) = match inner.split_once(',') {
                Some((n, f)) => (n.trim(), Some(f.trim())),
                None => (inner.trim(), None),
            };
            match (self.0.get(name), fallback) {
                (Some(v), _) => out.push_str(v),
                (None, Some(f)) => out.push_str(&self.resolve(f)?),
                (None, None) => return None,
            }
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        Some(out.trim().to_owned())
    }

    /// The tokens these variables describe; any that do not parse keep
    /// `fallback`'s value.
    fn theme(&self, fallback: Theme) -> Theme {
        let colour =
            |name: &str, or: Colour| self.0.get(name).and_then(|v| v.parse().ok()).unwrap_or(or);
        let opacity = self
            .0
            .get("--zb-surface-opacity")
            .and_then(|v| v.trim().parse::<f32>().ok())
            .unwrap_or(f32::from(fallback.surface.a) / 255.0);
        let radius = self
            .0
            .get("--zb-module-radius")
            .and_then(|v| v.trim().trim_end_matches("px").trim().parse::<f32>().ok())
            .filter(|r| r.is_finite() && *r >= 0.0)
            .unwrap_or(fallback.radius);
        let font = |name: &str, or: &str| {
            self.0
                .get(name)
                .map(|v| v.trim())
                .filter(|v| !v.is_empty())
                .unwrap_or(or)
                .to_owned()
        };
        Theme {
            accent: colour("--zb-accent", fallback.accent),
            surface: colour("--zb-surface-color", fallback.surface).with_alpha(opacity),
            text: colour("--zb-text", fallback.text),
            muted: colour("--zb-text-muted", fallback.muted),
            dim: colour("--zb-text-dim", fallback.dim),
            radius,
            font: font("--zb-font-family", &fallback.font),
            mono_font: font("--zb-mono", &fallback.mono_font),
        }
    }
}

/// Index of the `)` closing an already-opened `(` in `s`.
fn matching_paren(s: &str) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => return Some(i),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// The `--zb-*` declarations of every `:root` rule in `css`, in order.
fn root_variables(css: &str) -> Vec<(String, String)> {
    let css = strip_comments(css);
    let mut out = Vec::new();
    let mut rest = css.as_str();
    while let Some(open) = rest.find('{') {
        let selector = rest[..open].rsplit(['}', ';']).next().unwrap_or("").trim();
        let Some(close) = rest[open..].find('}') else {
            break;
        };
        let body = &rest[open + 1..open + close];
        if selector.split(',').any(|s| s.trim() == ":root") {
            for decl in body.split(';') {
                if let Some((name, value)) = decl.split_once(':') {
                    let name = name.trim();
                    if name.starts_with("--zb-") {
                        out.push((name.to_owned(), value.trim().to_owned()));
                    }
                }
            }
        }
        rest = &rest[open + close + 1..];
    }
    out
}

fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        rest = rest[i + 2..]
            .find("*/")
            .map_or("", |j| &rest[i + 2 + j + 2..]);
    }
    out.push_str(rest);
    out
}
