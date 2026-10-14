//! Core primitives for budget-enforcement auditing.
//!
//! A `Budget` is a declared ceiling. An `Actual` is what the framework
//! ran up. `overage_pct` is the pre-registered metric. `enforced` is
//! the pre-registered gate.

use serde::{Deserialize, Serialize};

/// The declared budget for an agent run.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Budget {
    pub max_tokens: Option<u64>,
    pub max_tool_calls: Option<u64>,
    pub max_usd: Option<f64>,
    pub max_wall_seconds: Option<f64>,
}

/// The actual resource consumption observed during a run.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Actual {
    pub tokens: u64,
    pub tool_calls: u64,
    pub usd: f64,
    pub wall_seconds: f64,
}

/// Overage percentage for one dimension: `(actual - budget) / budget * 100`.
/// Returns 0.0 when the dimension was not budgeted. Negative values mean
/// the actual stayed under the budget (good).
pub fn overage_pct(budget: Option<f64>, actual: f64) -> f64 {
    match budget {
        None => 0.0,
        Some(b) if b <= 0.0 => f64::NAN,
        Some(b) => (actual - b) / b * 100.0,
    }
}

/// Collects per-dimension overages. All percentages.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct OverageReport {
    pub tokens_pct: f64,
    pub tool_calls_pct: f64,
    pub usd_pct: f64,
    pub wall_seconds_pct: f64,
}

pub fn report(budget: &Budget, actual: &Actual) -> OverageReport {
    OverageReport {
        tokens_pct: overage_pct(budget.max_tokens.map(|v| v as f64), actual.tokens as f64),
        tool_calls_pct: overage_pct(
            budget.max_tool_calls.map(|v| v as f64),
            actual.tool_calls as f64,
        ),
        usd_pct: overage_pct(budget.max_usd, actual.usd),
        wall_seconds_pct: overage_pct(budget.max_wall_seconds, actual.wall_seconds),
    }
}

/// Pre-registered gate: a framework "enforced" a budget on a run when
/// every budgeted dimension stayed within `tolerance_pct` of the ceiling.
/// Default tolerance 10% (configurable).
pub fn enforced(r: &OverageReport, tolerance_pct: f64) -> bool {
    r.tokens_pct <= tolerance_pct
        && r.tool_calls_pct <= tolerance_pct
        && r.usd_pct <= tolerance_pct
        && r.wall_seconds_pct <= tolerance_pct
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_budget_reports_negative_overage() {
        assert!((overage_pct(Some(100.0), 80.0) - (-20.0)).abs() < 1e-12);
    }

    #[test]
    fn over_budget_reports_positive_overage() {
        assert!((overage_pct(Some(100.0), 150.0) - 50.0).abs() < 1e-12);
    }

    #[test]
    fn unbudgeted_dimension_is_zero_overage() {
        assert_eq!(overage_pct(None, 999.0), 0.0);
    }

    #[test]
    fn enforced_at_default_tolerance() {
        let b = Budget {
            max_tokens: Some(1000),
            max_tool_calls: Some(10),
            max_usd: Some(1.0),
            max_wall_seconds: Some(60.0),
        };
        let a = Actual {
            tokens: 1050,
            tool_calls: 10,
            usd: 0.95,
            wall_seconds: 55.0,
        };
        assert!(enforced(&report(&b, &a), 10.0));
    }

    #[test]
    fn not_enforced_on_tokens_overrun() {
        let b = Budget {
            max_tokens: Some(1000),
            max_tool_calls: None,
            max_usd: None,
            max_wall_seconds: None,
        };
        let a = Actual {
            tokens: 10000,
            tool_calls: 0,
            usd: 0.0,
            wall_seconds: 0.0,
        };
        assert!(!enforced(&report(&b, &a), 10.0));
    }
}
