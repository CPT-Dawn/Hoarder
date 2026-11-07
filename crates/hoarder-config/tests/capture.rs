//! `[capture]` (PLAN.md §4.4): how a Target that spans outputs is cut.

use hoarder_config::{Depth, MixedScale, parse};

#[test]
fn mixed_scale_and_depth_live_under_capture() {
    let config = parse("[capture]\nmixed_scale = \"split\"\ndepth = 8\n").unwrap();

    assert_eq!(config.capture.mixed_scale, MixedScale::Split);
    assert_eq!(config.capture.depth, Depth::Eight);
    let defaults = parse("").unwrap().capture;
    assert_eq!(
        (defaults.mixed_scale, defaults.depth),
        (MixedScale::Upscale, Depth::Auto)
    );
}

#[test]
fn screenshot_no_longer_takes_them() {
    for old in ["mixed_scale = \"clamp\"", "depth = 8"] {
        let d = parse(&format!("[screenshot]\n{old}\n")).unwrap_err();
        assert_eq!(d.line, 2, "{d}");
    }
}
