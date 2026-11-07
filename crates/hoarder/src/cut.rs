//! Cutting a Target out of the Frozen Frames under `screenshot.mixed_scale`
//! and `screenshot.depth` (PLAN.md §4.1 step 5, §4.4).
//!
//! The policies only matter where a Target spans outputs at different scales:
//! a Target on outputs that share one scale is always cut pixel for pixel.

use hoarder_config::{Depth, MixedScale, Screenshot};
use hoarder_ui::Region;
use hoarder_wayland::{FrozenFrame, LogicalRect};

use crate::image::{Image, scale_of};

/// Marks the notification puts on a Capture (PLAN.md §4.4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Badges {
    /// Lower-scale pixels were enlarged to match a higher-scale output.
    pub mixed_scale: bool,
    /// Pixels came from an HDR output and were not tone mapped.
    pub hdr: bool,
}

impl Badges {
    /// The badge texts, in display order.
    pub fn labels(self) -> Vec<&'static str> {
        [(self.mixed_scale, "mixed scale"), (self.hdr, "HDR")]
            .into_iter()
            .filter_map(|(on, label)| on.then_some(label))
            .collect()
    }
}

/// One image of a Capture. `output` names its output when the Capture was
/// split into one image per output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub output: Option<String>,
    pub image: Image,
}

/// A Target cut from the Frozen Frames: one Part, or one per output under
/// `mixed_scale = "split"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cut {
    pub parts: Vec<Part>,
    pub badges: Badges,
}

impl Cut {
    /// The Part with the most pixels: the one that goes to the clipboard and
    /// to stdout, which hold a single image.
    pub fn main_part(&self) -> &Part {
        self.parts
            .iter()
            .max_by_key(|p| p.image.width as u64 * p.image.height as u64)
            .expect("a Cut has at least one Part")
    }
}

/// Whether the Overlay must keep a Region on the output where its drag
/// started: `mixed_scale = "clamp"` on a desktop whose outputs differ in scale.
pub fn clamps_regions(frames: &[FrozenFrame], settings: &Screenshot) -> bool {
    settings.mixed_scale == MixedScale::Clamp && mixed(frames.iter())
}

/// Cut `region`, or All Outputs when it is `None`, from `frames`.
pub fn cut(frames: &[FrozenFrame], region: Option<&Region>, settings: &Screenshot) -> Cut {
    let mut touched: Vec<&FrozenFrame> = match region {
        Some(r) => frames.iter().filter(|f| overlaps(&f.logical, r)).collect(),
        None => frames.iter().collect(),
    };
    // Split Parts come in reading order, whatever order the compositor
    // advertised the outputs in.
    touched.sort_by_key(|f| (f.logical.y, f.logical.x));
    let whole = |f: &FrozenFrame| {
        let LogicalRect {
            x,
            y,
            width,
            height,
        } = f.logical;
        Region {
            x: x as f64,
            y: y as f64,
            width: width as f64,
            height: height as f64,
        }
    };
    let is_mixed = mixed(touched.iter().copied());
    let split_parts: Vec<Part> = if is_mixed && settings.mixed_scale == MixedScale::Split {
        touched
            .iter()
            .map(|&f| Part {
                output: Some(f.output.clone()),
                image: Image::region(&[f], &intersect(region.copied().unwrap_or(whole(f)), f)),
            })
            // A sliver thinner than a physical pixel makes no image.
            .filter(|p: &Part| p.image.width > 0 && p.image.height > 0)
            .collect()
    } else {
        Vec::new()
    };
    let split = !split_parts.is_empty();
    let parts = if split {
        split_parts
    } else {
        let image = match region {
            Some(r) => Image::region(&touched, r),
            None => Image::all_outputs(&touched),
        };
        vec![Part {
            output: None,
            image,
        }]
    };
    let parts = parts
        .into_iter()
        .map(|p| Part {
            image: match settings.depth {
                Depth::Auto => p.image,
                Depth::Eight => p.image.into_eight_bit(),
            },
            ..p
        })
        .collect();
    Cut {
        parts,
        badges: Badges {
            mixed_scale: is_mixed && !split,
            hdr: touched.iter().any(|f| f.hdr),
        },
    }
}

/// Whether `frames` are not all at one scale.
fn mixed<'a>(mut frames: impl Iterator<Item = &'a FrozenFrame>) -> bool {
    let Some(first) = frames.next().map(scale_of) else {
        return false;
    };
    frames.any(|f| (scale_of(f) - first).abs() > 1e-9)
}

fn overlaps(o: &LogicalRect, r: &Region) -> bool {
    let (ox0, oy0) = (o.x as f64, o.y as f64);
    let (ox1, oy1) = (ox0 + o.width as f64, oy0 + o.height as f64);
    r.x < ox1 && ox0 < r.x + r.width && r.y < oy1 && oy0 < r.y + r.height
}

/// The part of `r` on `frame`'s output.
fn intersect(r: Region, frame: &FrozenFrame) -> Region {
    let o = frame.logical;
    let x0 = r.x.max(o.x as f64);
    let y0 = r.y.max(o.y as f64);
    let x1 = (r.x + r.width).min((o.x + o.width as i32) as f64);
    let y1 = (r.y + r.height).min((o.y + o.height as i32) as f64);
    Region {
        x: x0,
        y: y0,
        width: (x1 - x0).max(0.0),
        height: (y1 - y0).max(0.0),
    }
}

#[cfg(test)]
mod tests {
    use hoarder_wayland::{ShmFormat, Transform};

    use super::*;

    fn frame(name: &str, x: i32, hdr: bool) -> FrozenFrame {
        FrozenFrame {
            output: name.into(),
            logical: LogicalRect {
                x,
                y: 0,
                width: 4,
                height: 2,
            },
            format: ShmFormat::Xrgb8888,
            width: 4,
            height: 2,
            stride: 16,
            transform: Transform::Normal,
            y_invert: false,
            hdr,
            data: vec![0; 32],
        }
    }

    /// Headless Sway has no HDR outputs, so the badge is checked here.
    #[test]
    fn only_a_target_touching_an_hdr_output_gets_the_hdr_badge() {
        let frames = [frame("SDR", 0, false), frame("HDR", 4, true)];
        let on = |x: f64| Region {
            x,
            y: 0.0,
            width: 2.0,
            height: 2.0,
        };
        let settings = Screenshot::default();
        assert!(!cut(&frames, Some(&on(1.0)), &settings).badges.hdr);
        assert!(cut(&frames, Some(&on(5.0)), &settings).badges.hdr);
        assert_eq!(
            cut(&frames, None, &settings).badges.labels(),
            ["HDR"],
            "All Outputs at one scale: HDR, but no mixed scale badge"
        );
    }
}
