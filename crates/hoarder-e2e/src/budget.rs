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

/// Print `measured` bytes as a `budget:` line for the budgets job, which
/// fails a change that grows it by more than 20% over its base. Memory has no
/// fixed ceiling (PLAN.md section 12), so this never fails on its own.
pub fn report_memory(what: &str, measured: u64) {
    eprintln!(
        "budget: {what}: measured {:.2} MB, tracked for regressions only",
        measured as f64 / (1024.0 * 1024.0),
    );
}
