//! The Reader rules (PLAN.md §5.1, ADR-0005): which of an Offer's types are
//! worth reading, and the limits a read must stay within.

use std::time::Duration;

/// How long one type may take to arrive.
pub const PER_TYPE: Duration = Duration::from_millis(500);
/// How long a whole Offer may take.
pub const PER_OFFER: Duration = Duration::from_secs(2);

/// Types that are protocol plumbing or app-private, never worth keeping.
const DENY_EXACT: [&str; 5] = [
    "SAVE_TARGETS",
    "TARGETS",
    "MULTIPLE",
    "TIMESTAMP",
    "image/x-inkscape-svg",
];
const DENY_PREFIX: [&str; 3] = ["chromium/x-", "application/x-kde-", "application/x-qt-"];

/// The types to read from an Offer of `offered`, in the source's order:
/// everything not denied (built in, or by `deny_types`), and of the images
/// only `image/png` plus the source's first image type.
pub fn plan(offered: &[String], deny_types: &[String]) -> Vec<String> {
    let allowed: Vec<&String> = offered.iter().filter(|m| !denied(m, deny_types)).collect();
    let first_image = allowed.iter().find(|m| is_image(m)).copied();
    let mut plan: Vec<String> = Vec::new();
    for m in allowed {
        let keep = !is_image(m) || m == "image/png" || Some(m) == first_image;
        if keep && !plan.contains(m) {
            plan.push(m.clone());
        }
    }
    plan
}

fn is_image(mime: &str) -> bool {
    mime.starts_with("image/")
}

fn denied(mime: &str, deny_types: &[String]) -> bool {
    let subtype = mime.split_once('/').map_or(mime, |(_, s)| s);
    DENY_EXACT.contains(&mime)
        || DENY_PREFIX.iter().any(|p| mime.starts_with(p))
        || mime.starts_with("x-moz-")
        || subtype.starts_with("x-moz-")
        || mime == crate::clipboard::PASSWORD_HINT
        || deny_types
            .iter()
            .any(|pattern| match pattern.strip_suffix('*') {
                Some(prefix) => mime.starts_with(prefix),
                None => mime == pattern,
            })
}
