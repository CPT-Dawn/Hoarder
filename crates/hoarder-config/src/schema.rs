//! The typed schema (PLAN.md §9). Every table and key has a default, so a
//! partial file, or no file at all, is a complete configuration.

use std::fmt;

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};

use crate::template::NameTemplate;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub theme: Theme,
    pub screenshot: Screenshot,
    pub pick: Pick,
    pub clipboard: Clipboard,
    pub history: History,
    pub privacy: Privacy,
    pub picker: Picker,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    #[default]
    Zenith,
    MidnightBloom,
    TokyoNight,
    CatppuccinMocha,
    GruvboxDark,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Screenshot {
    /// Where Captures are saved; `~`, `$VAR` and `${VAR}` are expanded, and
    /// `$XDG_PICTURES_DIR` falls back to `user-dirs.dirs`, then `~/Pictures`.
    pub dir: String,
    pub name: NameTemplate,
    pub format: Format,
    pub save: bool,
    pub copy: bool,
    pub notify: bool,
    pub confirm: bool,
    pub freeze: bool,
    pub cursor: bool,
    pub window_source: WindowSource,
    pub mixed_scale: MixedScale,
    pub depth: Depth,
    pub sound: bool,
    pub flash: bool,
    pub on_capture: String,
    pub png: Png,
    pub webp: Webp,
    pub jpeg: Jpeg,
    pub avif: Avif,
}

impl Default for Screenshot {
    fn default() -> Self {
        Screenshot {
            dir: "$XDG_PICTURES_DIR/Screenshots".into(),
            name: NameTemplate::default(),
            format: Format::Png,
            save: true,
            copy: true,
            notify: true,
            confirm: false,
            freeze: true,
            cursor: false,
            window_source: WindowSource::Frame,
            mixed_scale: MixedScale::Upscale,
            depth: Depth::Auto,
            sound: false,
            flash: false,
            on_capture: String::new(),
            png: Png::default(),
            webp: Webp::default(),
            jpeg: Jpeg::default(),
            avif: Avif::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Png,
    Webp,
    Jpeg,
    Avif,
    Qoi,
}

impl Format {
    /// The file extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Webp => "webp",
            Format::Jpeg => "jpg",
            Format::Avif => "avif",
            Format::Qoi => "qoi",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowSource {
    #[default]
    Frame,
    Toplevel,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MixedScale {
    #[default]
    Upscale,
    Clamp,
    Split,
}

/// `capture.depth`: `"auto"` or `8`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Depth {
    #[default]
    Auto,
    Eight,
}

impl Serialize for Depth {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Depth::Auto => s.serialize_str("auto"),
            Depth::Eight => s.serialize_i64(8),
        }
    }
}

impl<'de> Deserialize<'de> for Depth {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = Depth;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(r#""auto" or 8"#)
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Depth, E> {
                match v {
                    "auto" => Ok(Depth::Auto),
                    _ => Err(E::invalid_value(de::Unexpected::Str(v), &self)),
                }
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Depth, E> {
                match v {
                    8 => Ok(Depth::Eight),
                    _ => Err(E::invalid_value(de::Unexpected::Signed(v), &self)),
                }
            }
        }
        d.deserialize_any(V)
    }
}

/// PNG compression: `"fast"`, `"best"` or a level from 0 to 9.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PngLevel {
    Fast,
    #[default]
    Best,
    Level(u8),
}

impl Serialize for PngLevel {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            PngLevel::Fast => s.serialize_str("fast"),
            PngLevel::Best => s.serialize_str("best"),
            PngLevel::Level(l) => s.serialize_i64(i64::from(*l)),
        }
    }
}

impl<'de> Deserialize<'de> for PngLevel {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = PngLevel;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(r#""fast", "best" or a level from 0 to 9"#)
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<PngLevel, E> {
                match v {
                    "fast" => Ok(PngLevel::Fast),
                    "best" => Ok(PngLevel::Best),
                    _ => Err(E::invalid_value(de::Unexpected::Str(v), &self)),
                }
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<PngLevel, E> {
                match u8::try_from(v) {
                    Ok(l @ 0..=9) => Ok(PngLevel::Level(l)),
                    _ => Err(E::invalid_value(de::Unexpected::Signed(v), &self)),
                }
            }
        }
        d.deserialize_any(V)
    }
}

/// An integer that must lie in `MIN..=MAX`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Int<const MIN: u32, const MAX: u32>(u32);

impl<const MIN: u32, const MAX: u32> Int<MIN, MAX> {
    /// Panics if `v` is out of range; meant for defaults.
    pub const fn new(v: u32) -> Self {
        assert!(MIN <= v && v <= MAX, "out of range");
        Int(v)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

impl<const MIN: u32, const MAX: u32> Serialize for Int<MIN, MAX> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_i64(i64::from(self.0))
    }
}

impl<'de, const MIN: u32, const MAX: u32> Deserialize<'de> for Int<MIN, MAX> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<const MIN: u32, const MAX: u32>;
        impl<const MIN: u32, const MAX: u32> Visitor<'_> for V<MIN, MAX> {
            type Value = Int<MIN, MAX>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "an integer from {MIN} to {MAX}")
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                match u32::try_from(v) {
                    Ok(v) if (MIN..=MAX).contains(&v) => Ok(Int(v)),
                    _ => Err(E::invalid_value(de::Unexpected::Signed(v), &self)),
                }
            }
        }
        d.deserialize_i64(V::<MIN, MAX>)
    }
}

pub type Percent = Int<0, 100>;
/// A count or size limit: any non-negative integer that fits in 32 bits.
pub type Count = Int<0, { u32::MAX }>;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Png {
    pub level: PngLevel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Webp {
    pub lossless: bool,
    pub quality: Percent,
}

impl Default for Webp {
    fn default() -> Self {
        Webp {
            lossless: true,
            quality: Int::new(90),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Jpeg {
    pub quality: Int<1, 100>,
}

impl Default for Jpeg {
    fn default() -> Self {
        Jpeg {
            quality: Int::new(92),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Avif {
    pub quality: Percent,
    pub speed: Int<1, 10>,
}

impl Default for Avif {
    fn default() -> Self {
        Avif {
            quality: Int::new(80),
            speed: Int::new(6),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Pick {
    pub format: PickFormat,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PickFormat {
    #[default]
    Hex,
    Rgb,
    Hsl,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Clipboard {
    pub persist: bool,
    pub deny_types: Vec<String>,
    pub max_offer_mb: Count,
    /// App ids whose copies the Daemon never takes over (the escape hatch
    /// for apps that misbehave when Hoarder serves their Selection).
    pub no_takeover_apps: Vec<String>,
    pub on_clip: String,
    pub primary: Primary,
}

impl Default for Clipboard {
    fn default() -> Self {
        Clipboard {
            persist: true,
            deny_types: Vec::new(),
            max_offer_mb: Int::new(64),
            no_takeover_apps: Vec::new(),
            on_clip: String::new(),
            primary: Primary::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Primary {
    pub persist: bool,
    pub history: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct History {
    pub enabled: bool,
    pub max_clips: Count,
    pub max_age_days: Count,
    pub max_clip_mb: Count,
    pub max_total_mb: Count,
    pub encrypt: bool,
}

impl Default for History {
    fn default() -> Self {
        History {
            enabled: true,
            max_clips: Int::new(1000),
            max_age_days: Int::new(30),
            max_clip_mb: Int::new(64),
            max_total_mb: Int::new(1024),
            encrypt: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Privacy {
    pub exclude_apps: Vec<String>,
    pub patterns: bool,
    pub extra_patterns: Vec<String>,
    pub concealed_ttl_secs: Count,
}

impl Default for Privacy {
    fn default() -> Self {
        Privacy {
            exclude_apps: [
                "org.keepassxc.KeePassXC",
                "Bitwarden",
                "1Password",
                "proton-pass",
                "org.gnome.World.Secrets",
            ]
            .map(String::from)
            .into(),
            patterns: true,
            extra_patterns: Vec::new(),
            concealed_ttl_secs: Int::new(120),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Picker {
    pub auto_paste: bool,
    pub terminal_apps: Vec<String>,
}

impl Default for Picker {
    fn default() -> Self {
        Picker {
            auto_paste: true,
            terminal_apps: [
                "kitty",
                "foot",
                "Alacritty",
                "org.wezfurlong.wezterm",
                "com.mitchellh.ghostty",
                "org.kde.konsole",
            ]
            .map(String::from)
            .into(),
        }
    }
}
