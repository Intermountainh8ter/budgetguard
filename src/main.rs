//! budgetguard CLI — scaffold.

use budgetguard::{certify, enforced, report, Actual, Breaker, BreakerRun, Budget};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "budgetguard",
    version,
    about = "Agent budget enforcement audit"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Compare a declared budget to the observed actual spend and print
    /// per-dimension overage plus the pre-registered enforced verdict.
    Score {
        budget: std::path::PathBuf,
        actual: std::path::PathBuf,
        #[arg(long, default_value_t = 10.0)]
        tolerance_pct: f64,
    },
    /// Aggregate four BreakerRuns into a full pre-reg §3 certification.
    /// Reads a certification bundle:
    /// `{"framework": "...", "budget": {...}, "breakers": [BreakerRun, ...]}`
    /// Exits 0 certified, 1 not certified, 2 breakers missing.
    Certify {
        bundle: std::path::PathBuf,
        #[arg(long, default_value_t = 10.0)]
        tolerance_pct: f64,
    },
    Version,
}

fn load_json<T: serde::de::DeserializeOwned>(p: &std::path::Path) -> anyhow::Result<T> {
    Ok(serde_json::from_str(&std::fs::read_to_string(p)?)?)
}

#[derive(serde::Deserialize)]
struct CertifyBundle {
    framework: String,
    budget: Budget,
    breakers: Vec<BreakerRun>,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Score {
            budget,
            actual,
            tolerance_pct,
        } => {
            let b: Budget = load_json(&budget)?;
            let a: Actual = load_json(&actual)?;
            let r = report(&b, &a);
            let verdict = if enforced(&r, tolerance_pct) {
                "ENFORCED"
            } else {
                "NOT ENFORCED"
            };
            println!("{}", serde_json::to_string_pretty(&r)?);
            println!("tolerance: {tolerance_pct:.2}%");
            println!("verdict:   {verdict}");
        }
        Cmd::Certify {
            bundle,
            tolerance_pct,
        } => {
            let b: CertifyBundle = load_json(&bundle)?;
            let c = certify(&b.framework, &b.budget, &b.breakers, tolerance_pct);
            let missing = !c.missing_breakers.is_empty();
            let certified = c.certified;
            println!("{}", serde_json::to_string_pretty(&c)?);
            if missing {
                std::process::exit(2);
            } else if !certified {
                std::process::exit(1);
            }
            // Ensure Breaker is used so the import is live even if the
            // bundle variants are all serialized by name.
            let _ = Breaker::LongPromptLoop;
        }
        Cmd::Version => println!("budgetguard {}", env!("CARGO_PKG_VERSION")),
    }
    Ok(())
}
