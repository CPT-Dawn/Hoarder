//! Colours as CSS writes them: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`,
//! `rgb()` and `rgba()`, in comma or space syntax, with an optional alpha.

use std::fmt;
use std::str::FromStr;

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};

/// An sRGB colour with straight (not premultiplied) alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Colour {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Colour {
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Colour { r, g, b, a }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Colour::rgba(r, g, b, 255)
    }

    /// The same colour at `alpha` (0 to 1) opacity.
    pub fn with_alpha(self, alpha: f32) -> Self {
        Colour {
            a: unit(alpha),
            ..self
        }
    }
}

fn unit(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Why a string is not a [`Colour`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadColour(String);

impl fmt::Display for BadColour {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{:?} is not a colour (try \"#rrggbb\", \"#rrggbbaa\" or \"rgb(r g b / a)\")",
            self.0
        )
    }
}

impl std::error::Error for BadColour {}

impl FromStr for Colour {
    type Err = BadColour;

    fn from_str(s: &str) -> Result<Self, BadColour> {
        let bad = || BadColour(s.to_owned());
        let t = s.trim();
        if let Some(hex) = t.strip_prefix('#') {
            return hex_colour(hex).ok_or_else(bad);
        }
        let lower = t.to_ascii_lowercase();
        let args = lower
            .strip_prefix("rgba(")
            .or_else(|| lower.strip_prefix("rgb("))
            .and_then(|r| r.strip_suffix(')'))
            .ok_or_else(bad)?;
        functional(args).ok_or_else(bad)
    }
}

fn hex_colour(hex: &str) -> Option<Colour> {
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize| u8::from_str_radix(&hex[i..=i], 16).ok().map(|v| v * 17);
    let pair = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some(match hex.len() {
        3 => Colour::rgb(digit(0)?, digit(1)?, digit(2)?),
        4 => Colour::rgba(digit(0)?, digit(1)?, digit(2)?, digit(3)?),
        6 => Colour::rgb(pair(0)?, pair(2)?, pair(4)?),
        8 => Colour::rgba(pair(0)?, pair(2)?, pair(4)?, pair(6)?),
        _ => return None,
    })
}

/// `r g b`, `r g b / a`, `r, g, b` or `r, g, b, a`.
fn functional(args: &str) -> Option<Colour> {
    let (rgb, alpha) = match args.split_once('/') {
        Some((rgb, a)) => (rgb, Some(a.trim())),
        None => (args, None),
    };
    let parts: Vec<&str> = if rgb.contains(',') {
        rgb.split(',').map(str::trim).collect()
    } else {
        rgb.split_whitespace().collect()
    };
    let (channels, alpha) = match (parts.as_slice(), alpha) {
        ([r, g, b], a) => ([*r, *g, *b], a),
        ([r, g, b, a], None) => ([*r, *g, *b], Some(*a)),
        _ => return None,
    };
    let channel = |v: &str| -> Option<u8> {
        match v.strip_suffix('%') {
            Some(p) => Some(unit(p.parse::<f32>().ok()? / 100.0)),
            None => Some(v.parse::<f32>().ok()?.clamp(0.0, 255.0).round() as u8),
        }
    };
    let a = match alpha {
        None => 255,
        Some(a) => match a.strip_suffix('%') {
            Some(p) => unit(p.parse::<f32>().ok()? / 100.0),
            None => unit(a.parse().ok()?),
        },
    };
    let [r, g, b] = channels;
    Some(Colour::rgba(channel(r)?, channel(g)?, channel(b)?, a))
}

impl fmt::Display for Colour {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)?;
        if self.a != 255 {
            write!(f, "{:02x}", self.a)?;
        }
        Ok(())
    }
}

impl Serialize for Colour {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Colour {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(de::Error::custom)
    }
}

/// `serde(with)` for an override that is either `""` (keep the theme's value)
/// or a colour.
pub(crate) mod unset_or_colour {
    use super::*;

    pub fn serialize<S: Serializer>(v: &Option<Colour>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(c) => c.serialize(s),
            None => s.serialize_str(""),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Colour>, D::Error> {
        let s = String::deserialize(d)?;
        if s.is_empty() {
            Ok(None)
        } else {
            s.parse().map(Some).map_err(de::Error::custom)
        }
    }
}

/// `serde(with)` for a radius override: `""` (the theme's) or 0 to 64 pixels.
pub(crate) mod unset_or_radius {
    use super::*;

    pub const MAX: u32 = 64;

    pub fn serialize<S: Serializer>(v: &Option<u32>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(r) => s.serialize_i64(i64::from(*r)),
            None => s.serialize_str(""),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = Option<u32>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "pixels from 0 to {MAX}, or \"\" for the theme's")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                match v {
                    "" => Ok(None),
                    _ => Err(E::invalid_value(de::Unexpected::Str(v), &self)),
                }
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                match u32::try_from(v) {
                    Ok(r) if r <= MAX => Ok(Some(r)),
                    _ => Err(E::invalid_value(de::Unexpected::Signed(v), &self)),
                }
            }
        }
        d.deserialize_any(V)
    }
}
