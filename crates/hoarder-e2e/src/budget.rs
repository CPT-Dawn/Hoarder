//! Budgets (PLAN.md §12) for timing and memory tests.
//!
//! Budgets are written as PLAN states them. Shared CI runners are several
//! times slower than a desktop, so `HOARDER_BUDGET_SCALE` (default `1.0`)
//! multiplies every latency budget; CI sets it, local runs leave it alone.
//! Memory budgets are not scaled: a slow runner uses no more of it.

use std::time::Duration;

/// The environment variable that scales every budget.
pub const SCALE_VAR: &str = "HOARDER_BUDGET_SCALE";

/// The factor from `HOARDER_BUDGET_SCALE`, or `1.0` when it is unset.
///
/// # Panics
/// If it is set to anything but a positive number.
pub fn scale() -> f64 {
    match std::env::var(SCALE_VAR) {
        Err(_) => 1.0,
        Ok(v) => match v.trim().parse::<f64>() {
            Ok(s) if s.is_finite() && s > 0.0 => s,
            _ => panic!("{SCALE_VAR}={v:?} is not a positive number"),
        },
    }
}

/// Print `measured` against the PLAN budget `plan` (scaled by [`scale`]) and
/// fail if it is over. Timing unoptimised code proves nothing, so debug
/// builds only print.
pub fn assert_within(what: &str, measured: Duration, plan: Duration) {
    let scale = scale();
    let budget = plan.mul_f64(scale);
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    eprintln!(
        "budget: {what}: measured {:.1} ms, budget {:.1} ms (PLAN {:.1} ms x {scale})",
        ms(measured),
        ms(budget),
        ms(plan),
    );
    if !cfg!(debug_assertions) {
        assert!(
            measured < budget,
            "{what}: {:.1} ms is over the {:.1} ms budget (PLAN {:.1} ms x {SCALE_VAR}={scale})",
            ms(measured),
            ms(budget),
            ms(plan),
        );
    }
}

/// Print `measured` bytes against the PLAN budget `plan` (in bytes, never
/// scaled) and fail if it is over. Debug builds carry more code and keep
/// more around, so they only print, like the latency budgets.
pub fn assert_memory_within(what: &str, measured: u64, plan: u64) {
    let mb = |b: u64| b as f64 / (1024.0 * 1024.0);
    eprintln!(
        "budget: {what}: measured {:.2} MB, budget {:.2} MB (PLAN, not scaled)",
        mb(measured),
        mb(plan),
    );
    if !cfg!(debug_assertions) {
        assert!(
            measured < plan,
            "{what}: {:.2} MB is over the {:.2} MB budget",
            mb(measured),
            mb(plan),
        );
    }
}

/// Print `measured` bytes against the PLAN budget `plan` in the same
/// `budget:` line as [`assert_memory_within`], for the budgets job to track,
/// without failing when it is over.
pub fn report_memory(what: &str, measured: u64, plan: u64) {
    let mb = |b: u64| b as f64 / (1024.0 * 1024.0);
    let over = if measured < plan {
        ""
    } else {
        ", OVER (reported only)"
    };
    eprintln!(
        "budget: {what}: measured {:.2} MB, budget {:.2} MB (PLAN, not scaled){over}",
        mb(measured),
        mb(plan),
    );
}
