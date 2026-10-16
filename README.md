# budgetguard

**Verify that agent frameworks honor declared budgets.** Four adversarial
budget-breakers (long-prompt loop, retry-forever loop, tool-recursion
loop, nested-sub-agent loop); per-dimension overage reporting (tokens,
tool calls, USD, wall seconds); pre-registered gate: a framework
"enforces" the budget when every dimension stays within 10% of the
declared ceiling on all four breakers.

![status: initial scaffold](https://img.shields.io/badge/status-initial%20scaffold-lightgrey)
![language: Rust](https://img.shields.io/badge/language-Rust-blue)
![license: MIT](https://img.shields.io/badge/license-MIT-blue)

## Why

Agent frameworks ship budget flags whose enforcement is tested only on
happy-path. Production incidents from agents overspending are frequent,
named, and expensive — a $40K-in-one-weekend overspend is a documented
war story in multiple orgs. There is no public harness for the exact
question "did the budget hold under adversarial inputs."

## What is in this scaffold

- `Budget`, `Actual`, `OverageReport` types.
- `overage_pct`, `report`, `enforced` primitives.
- `Breaker` enum (the four pre-reg §2 breakers) and
  `ActualPair { framework_reported, independently_observed }` so a
  framework's self-report and the counted-from-outside observer are
  both inputs — pre-reg §4 point 1.
- `BreakerRun { breaker, framework_report, independent_report,
  enforced_framework, enforced_independent, enforced_both }`.
- `breaker_run()` scores one breaker from a budget + ActualPair.
- `certify(framework, budget, breakers, tolerance_pct) -> Certification`
  aggregates the per-reg §3 rule: all four breakers must be present
  AND enforce on BOTH observers. Missing breakers disqualify.
- CLI: `budgetguard score` (back-compat), `budgetguard certify
  <bundle.json>`. Exit codes: 0 certified, 1 not certified, 2 missing
  breakers.
- `docs/PREREGISTRATION.md` locking the four breakers and the 10%
  tolerance.

Not in this commit (openly tracked):

- `budgetguard audit --framework <name> --budget <path>` driver.
- The four adversarial breakers as scripted environments.
- Framework adapters: LangChain, LlamaIndex, OpenAI Agents, Vercel AI
  SDK, Letta.

## License

MIT. See `LICENSE`.
