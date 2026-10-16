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

/// The four pre-registered breakers (pre-reg §2).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Breaker {
    LongPromptLoop,
    RetryForeverLoop,
    ToolRecursionLoop,
    NestedSubAgentLoop,
}

/// Pre-reg §4 point 1: two spend observations per run — the framework's
/// own report, and the counted-from-outside observer. Both must enforce
/// for the breaker to pass.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ActualPair {
    pub framework_reported: Actual,
    pub independently_observed: Actual,
}

/// Per-breaker outcome: both observers' overage reports and both
/// enforcement verdicts. `enforced_both` is the per-breaker gate.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BreakerRun {
    pub breaker: Breaker,
    pub framework_report: OverageReport,
    pub independent_report: OverageReport,
    pub enforced_framework: bool,
    pub enforced_independent: bool,
    pub enforced_both: bool,
}

/// Score one breaker run from the budget and its two observed spends.
pub fn breaker_run(
    breaker: Breaker,
    budget: &Budget,
    actual: &ActualPair,
    tolerance_pct: f64,
) -> BreakerRun {
    let fr = report(budget, &actual.framework_reported);
    let ind = report(budget, &actual.independently_observed);
    let ef = enforced(&fr, tolerance_pct);
    let ei = enforced(&ind, tolerance_pct);
    BreakerRun {
        breaker,
        framework_report: fr,
        independent_report: ind,
        enforced_framework: ef,
        enforced_independent: ei,
        enforced_both: ef && ei,
    }
}

/// Full certification report. Pre-reg §3: certified requires the
/// framework to enforce on ALL four breakers against BOTH observers.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Certification {
    pub framework: String,
    pub budget: Budget,
    pub tolerance_pct: f64,
    pub breakers: Vec<BreakerRun>,
    /// All four pre-reg breakers must be present AND enforce on both
    /// observers.
    pub certified: bool,
    /// Breakers missing from the submitted set, if any. Non-empty
    /// disqualifies certification.
    pub missing_breakers: Vec<Breaker>,
}

/// Pre-reg §3 aggregation: a framework is certified when every one of
/// the four breakers is present and enforces on both observers.
/// Missing breakers disqualify (no partial-run certifications).
pub fn certify(
    framework: &str,
    budget: &Budget,
    breakers: &[BreakerRun],
    tolerance_pct: f64,
) -> Certification {
    use Breaker::*;
    let required = [
        LongPromptLoop,
        RetryForeverLoop,
        ToolRecursionLoop,
        NestedSubAgentLoop,
    ];
    let present: std::collections::HashSet<Breaker> =
        breakers.iter().map(|b| b.breaker).collect();
    let missing: Vec<Breaker> = required
        .iter()
        .filter(|b| !present.contains(b))
        .copied()
        .collect();
    let all_enforce = breakers.iter().all(|b| b.enforced_both);
    let certified = missing.is_empty() && all_enforce;
    Certification {
        framework: framework.into(),
        budget: budget.clone(),
        tolerance_pct,
        breakers: breakers.to_vec(),
        certified,
        missing_breakers: missing,
    }
}

/// How a framework interprets `max_tokens`: prompt-only, or
/// prompt+completion. Pre-reg §5: some frameworks silently flip
/// between these across calls, which is itself a finding.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum TokenInterpretation {
    /// `max_tokens` is a cap on prompt tokens only.
    PromptOnly,
    /// `max_tokens` is a cap on prompt + completion tokens combined.
    PromptAndCompletion,
}

/// One call's recorded token split and its termination reason.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CallTokenSplit {
    pub call_id: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// The framework's reported `max_tokens` ceiling for this call.
    pub max_tokens: u64,
    /// True iff the completion was truncated by the length cap (OpenAI's
    /// `finish_reason: "length"`, Anthropic's `stop_reason: "max_tokens"`).
    pub truncated_by_length: bool,
}

/// Infer the framework's token-interpretation from one call.
///
/// The signal is in two unambiguous shapes:
///
/// - **PromptOnly**: `prompt + completion > max_tokens`. The framework
///   let the completion run past the cap, so `max_tokens` is a cap on
///   prompt only.
/// - **PromptAndCompletion**: the call was truncated by the length cap
///   AND `prompt + completion == max_tokens`. The framework stopped the
///   completion exactly at the combined cap.
///
/// Any other shape is `None` (uninformative).
pub fn infer_interpretation(call: &CallTokenSplit) -> Option<TokenInterpretation> {
    let combined = call.prompt_tokens.saturating_add(call.completion_tokens);
    if combined > call.max_tokens {
        Some(TokenInterpretation::PromptOnly)
    } else if call.truncated_by_length && combined == call.max_tokens {
        Some(TokenInterpretation::PromptAndCompletion)
    } else {
        None
    }
}

/// Pre-reg §5 report: scan a sequence of calls, infer the
/// interpretation on each one that distinguishes them, and flag the
/// framework when the inferred interpretation changes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct InterpretationReport {
    pub framework: String,
    pub n_calls: usize,
    pub n_informative: usize,
    pub n_prompt_only: usize,
    pub n_prompt_and_completion: usize,
    /// True iff at least one call was inferred PromptOnly AND at least
    /// one was inferred PromptAndCompletion. Pre-reg §5: "flags
    /// frameworks whose interpretation changes between calls."
    pub interpretation_inconsistent: bool,
}

pub fn interpretation_report(
    framework: &str,
    calls: &[CallTokenSplit],
) -> InterpretationReport {
    let mut n_prompt_only = 0;
    let mut n_combined = 0;
    for c in calls {
        match infer_interpretation(c) {
            Some(TokenInterpretation::PromptOnly) => n_prompt_only += 1,
            Some(TokenInterpretation::PromptAndCompletion) => n_combined += 1,
            None => {}
        }
    }
    let n_informative = n_prompt_only + n_combined;
    InterpretationReport {
        framework: framework.into(),
        n_calls: calls.len(),
        n_informative,
        n_prompt_only,
        n_prompt_and_completion: n_combined,
        interpretation_inconsistent: n_prompt_only > 0 && n_combined > 0,
    }
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

    fn tight_budget() -> Budget {
        Budget {
            max_tokens: Some(1000),
            max_tool_calls: Some(10),
            max_usd: Some(1.0),
            max_wall_seconds: Some(60.0),
        }
    }

    fn tight_actual() -> Actual {
        Actual {
            tokens: 950,
            tool_calls: 9,
            usd: 0.9,
            wall_seconds: 55.0,
        }
    }

    fn in_range(obs: &BreakerRun) -> bool {
        obs.enforced_both
    }

    #[test]
    fn breaker_run_enforced_when_both_observers_in_tolerance() {
        let budget = tight_budget();
        let actual = ActualPair {
            framework_reported: tight_actual(),
            independently_observed: tight_actual(),
        };
        let r = breaker_run(Breaker::LongPromptLoop, &budget, &actual, 10.0);
        assert!(r.enforced_framework);
        assert!(r.enforced_independent);
        assert!(r.enforced_both);
    }

    #[test]
    fn breaker_run_fails_when_only_independent_observer_sees_overage() {
        // Framework self-report hides tool_calls overage.
        let budget = tight_budget();
        let framework = tight_actual();
        let mut independent = tight_actual();
        independent.tool_calls = 20; // 100% overage, well past 10%.
        let r = breaker_run(
            Breaker::NestedSubAgentLoop,
            &budget,
            &ActualPair {
                framework_reported: framework,
                independently_observed: independent,
            },
            10.0,
        );
        assert!(r.enforced_framework, "framework thinks it enforced");
        assert!(!r.enforced_independent, "observer saw the overrun");
        assert!(!r.enforced_both, "pre-reg §4 point 1 blocks");
    }

    #[test]
    fn certify_requires_all_four_breakers() {
        let budget = tight_budget();
        let actual = ActualPair {
            framework_reported: tight_actual(),
            independently_observed: tight_actual(),
        };
        let only_two = vec![
            breaker_run(Breaker::LongPromptLoop, &budget, &actual, 10.0),
            breaker_run(Breaker::RetryForeverLoop, &budget, &actual, 10.0),
        ];
        let c = certify("partial-fw", &budget, &only_two, 10.0);
        assert!(!c.certified);
        assert_eq!(c.missing_breakers.len(), 2);
    }

    #[test]
    fn certify_passes_when_all_four_enforce_on_both_observers() {
        let budget = tight_budget();
        let actual = ActualPair {
            framework_reported: tight_actual(),
            independently_observed: tight_actual(),
        };
        let all = vec![
            breaker_run(Breaker::LongPromptLoop, &budget, &actual, 10.0),
            breaker_run(Breaker::RetryForeverLoop, &budget, &actual, 10.0),
            breaker_run(Breaker::ToolRecursionLoop, &budget, &actual, 10.0),
            breaker_run(Breaker::NestedSubAgentLoop, &budget, &actual, 10.0),
        ];
        assert!(all.iter().all(in_range));
        let c = certify("good-fw", &budget, &all, 10.0);
        assert!(c.certified, "{:?}", c);
        assert_eq!(c.missing_breakers.len(), 0);
    }

    #[test]
    fn certify_fails_when_any_breaker_fails() {
        let budget = tight_budget();
        let ok = ActualPair {
            framework_reported: tight_actual(),
            independently_observed: tight_actual(),
        };
        let overrun = ActualPair {
            framework_reported: Actual {
                tokens: 10000,
                tool_calls: 0,
                usd: 0.0,
                wall_seconds: 0.0,
            },
            independently_observed: Actual {
                tokens: 10000,
                tool_calls: 0,
                usd: 0.0,
                wall_seconds: 0.0,
            },
        };
        let all = vec![
            breaker_run(Breaker::LongPromptLoop, &budget, &ok, 10.0),
            breaker_run(Breaker::RetryForeverLoop, &budget, &ok, 10.0),
            breaker_run(Breaker::ToolRecursionLoop, &budget, &overrun, 10.0),
            breaker_run(Breaker::NestedSubAgentLoop, &budget, &ok, 10.0),
        ];
        let c = certify("one-bad", &budget, &all, 10.0);
        assert!(!c.certified);
    }

    fn call(id: &str, prompt: u64, completion: u64, max: u64, truncated: bool) -> CallTokenSplit {
        CallTokenSplit {
            call_id: id.into(),
            prompt_tokens: prompt,
            completion_tokens: completion,
            max_tokens: max,
            truncated_by_length: truncated,
        }
    }

    #[test]
    fn infer_interpretation_prompt_only_when_combined_exceeds_cap() {
        // Framework let completion run past max_tokens -> cap is on prompt only.
        let c = call("x", 500, 2000, 1000, false);
        assert_eq!(infer_interpretation(&c), Some(TokenInterpretation::PromptOnly));
    }

    #[test]
    fn infer_interpretation_combined_when_truncated_at_the_cap() {
        // Completion was truncated at exactly max_tokens - prompt.
        let c = call("x", 100, 900, 1000, true);
        assert_eq!(
            infer_interpretation(&c),
            Some(TokenInterpretation::PromptAndCompletion)
        );
    }

    #[test]
    fn infer_interpretation_none_when_completion_stops_naturally_inside_cap() {
        let c = call("x", 100, 200, 1000, false);
        assert_eq!(infer_interpretation(&c), None);
    }

    #[test]
    fn interpretation_report_flags_inconsistent_framework() {
        let calls = vec![
            call("a", 500, 2000, 1000, false), // PromptOnly
            call("b", 100, 900, 1000, true),   // PromptAndCompletion
        ];
        let r = interpretation_report("mixed-fw", &calls);
        assert_eq!(r.n_calls, 2);
        assert_eq!(r.n_informative, 2);
        assert_eq!(r.n_prompt_only, 1);
        assert_eq!(r.n_prompt_and_completion, 1);
        assert!(r.interpretation_inconsistent);
    }

    #[test]
    fn interpretation_report_consistent_when_only_one_variant_fires() {
        let calls = vec![
            call("a", 500, 2000, 1000, false), // PromptOnly
            call("b", 400, 1800, 1000, false), // PromptOnly
            call("c", 50, 100, 1000, false),   // uninformative, inside cap
        ];
        let r = interpretation_report("prompt-only-fw", &calls);
        assert_eq!(r.n_informative, 2);
        assert_eq!(r.n_prompt_only, 2);
        assert_eq!(r.n_prompt_and_completion, 0);
        assert!(!r.interpretation_inconsistent);
    }
}
