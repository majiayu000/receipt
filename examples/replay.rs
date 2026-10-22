//! Replay every stop of past sessions through the hook logic and count what would be blocked.
//!
//!   cargo run --release --example replay -- [--show N] <transcript.jsonl>...
//!
//! `--show N` prints N blocked stops (claim, verdict, last commands) for manual review. That output
//! contains raw session text: read it locally, never commit it.

use receipt::transcript::{parse_str, turn_finals, Step};
use receipt::{Checker, Config, Verdict};
use std::collections::BTreeMap;

fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let show = match args.iter().position(|a| a == "--show") {
        Some(i) => {
            let n = args[i + 1].parse::<usize>()?;
            args.drain(i..=i + 1);
            n
        }
        None => 0,
    };
    let checker = Checker::new(&Config::default())?;
    let (mut stops, mut with_claims, mut blocked, mut shown) = (0, 0, 0, 0);
    let mut verdicts: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for file in &args {
        let Ok(raw) = std::fs::read_to_string(file) else {
            continue;
        };
        let lines: Vec<&str> = raw.lines().collect();
        let (_, finals) = turn_finals(&lines);
        for (upto, text) in finals {
            stops += 1;
            let claims = checker.claims(&text);
            if claims.is_empty() {
                continue;
            }
            with_claims += 1;
            let session = parse_str(&lines[..upto].join("\n"), &checker.markers)?;
            let findings = checker.check(&claims, &session);
            for f in &findings {
                *verdicts
                    .entry((f.claim.kind, f.verdict.label()))
                    .or_default() += 1;
            }
            if !findings.iter().any(|f| f.blocks) {
                continue;
            }
            blocked += 1;
            if shown < show {
                shown += 1;
                println!("--- {file}:{upto}");
                for f in findings.iter().filter(|f| f.verdict != Verdict::Supported) {
                    println!(
                        "  [{}] {} \"{}\" — {}",
                        f.claim.kind,
                        f.verdict.label(),
                        f.claim.text,
                        f.detail
                    );
                }
                let runs: Vec<&str> = session
                    .steps
                    .iter()
                    .rev()
                    .filter_map(|s| match s {
                        Step::Run(r) => Some(r.command.lines().next().unwrap_or("")),
                        Step::Edit { .. } => None,
                    })
                    .take(4)
                    .collect();
                println!("  last commands: {runs:?}");
            }
        }
    }
    println!("stops: {stops}, with claims: {with_claims}, blocked: {blocked}");
    for ((kind, verdict), n) in verdicts {
        println!("  {kind:<9} {verdict:<13} {n}");
    }
    Ok(())
}
