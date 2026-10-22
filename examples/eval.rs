//! Score claim detection and the stop gate on the labelled sets.
//!
//!   cargo run --release --example eval -- [--verbose] eval/real.jsonl eval/synthetic.jsonl
//!
//! Detection is scored per (case, kind). The gate is scored per case, positive = blocked.
//! The expected gate decision applies the evidence rules to the hand-labelled kinds, so gate
//! errors come only from detection errors. Real candidates were sampled per stratum (detector
//! hit / broad keyword net only); the weighted rows scale each case by stratum size / sample size.

use receipt::claims::Claim;
use receipt::transcript::{Outcome, Run, Session, Step};
use receipt::{Checker, Config};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize)]
struct Case {
    id: String,
    set: String,
    split: String,
    text: String,
    steps: Vec<StepSpec>,
    claims: Vec<String>,
    #[serde(default)]
    stratum: Option<String>,
    #[serde(default)]
    stratum_size: Option<f64>,
}

#[derive(Deserialize)]
struct StepSpec {
    run: Option<String>,
    outcome: Option<String>,
    #[serde(default)]
    fail_output: bool,
    edit: Option<String>,
}

#[derive(Default)]
struct Score {
    tp: f64,
    fp: f64,
    fn_: f64,
}

impl Score {
    fn pr(&self) -> String {
        let p = if self.tp + self.fp > 0.0 {
            100.0 * self.tp / (self.tp + self.fp)
        } else {
            f64::NAN
        };
        let r = if self.tp + self.fn_ > 0.0 {
            100.0 * self.tp / (self.tp + self.fn_)
        } else {
            f64::NAN
        };
        format!("{p:.1}% / {r:.1}%")
    }
}

fn session(steps: &[StepSpec]) -> Session {
    let steps = steps
        .iter()
        .map(|s| match (&s.run, &s.edit) {
            (Some(cmd), _) => Step::Run(Run {
                command: cmd.clone(),
                outcome: match s.outcome.as_deref() {
                    Some("ok") => Outcome::Ok,
                    Some("failed") => Outcome::Failed(None),
                    Some("pending") => Outcome::Pending,
                    Some("not_run") => Outcome::NotRun,
                    _ => Outcome::NoResult,
                },
                output_failed: s.fail_output,
            }),
            (None, Some(kind)) => Step::Edit {
                doc_only: kind == "doc",
            },
            (None, None) => panic!("step has neither run nor edit"),
        })
        .collect();
    Session {
        steps,
        last_text: String::new(),
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let verbose = args.iter().any(|a| a == "--verbose");
    let checker = Checker::new(&Config::default())?;
    let mut cases = vec![];
    for f in args.iter().filter(|a| !a.starts_with("--")) {
        for line in std::fs::read_to_string(f)?
            .lines()
            .filter(|l| !l.trim().is_empty())
        {
            cases.push(serde_json::from_str::<Case>(line)?);
        }
    }
    // Sample sizes per stratum, for the weighted rows.
    let mut sampled: BTreeMap<String, f64> = BTreeMap::new();
    for c in &cases {
        if let Some(s) = &c.stratum {
            *sampled.entry(s.clone()).or_default() += 1.0;
        }
    }

    // (set, split, weighted) -> (detection, gate)
    let mut scores: BTreeMap<(String, String, bool), (Score, Score)> = BTreeMap::new();
    for c in &cases {
        let s = session(&c.steps);
        let predicted: BTreeSet<&str> = checker.claims(&c.text).iter().map(|c| c.kind).collect();
        let expected: BTreeSet<&str> = c.claims.iter().map(String::as_str).collect();
        let labelled: Vec<Claim> = c
            .claims
            .iter()
            .map(|k| Claim {
                kind: receipt::rules::kind(k).expect("known kind").name,
                text: c.text.clone(),
            })
            .collect();
        let gate_expected = checker.check(&labelled, &s).iter().any(|f| f.blocks);
        let gate_predicted = checker
            .check(&checker.claims(&c.text), &s)
            .iter()
            .any(|f| f.blocks);

        if verbose && (predicted != expected || gate_expected != gate_predicted) {
            println!(
                "{} [{}] {}\n    expected {:?} got {:?}; gate expected {} got {}",
                c.id, c.split, c.text, expected, predicted, gate_expected, gate_predicted
            );
        }
        let weight = match (&c.stratum, c.stratum_size) {
            (Some(st), Some(size)) => size / sampled[st],
            _ => 1.0,
        };
        let weights: &[(bool, f64)] = if c.stratum.is_some() {
            &[(false, 1.0), (true, weight)]
        } else {
            &[(false, 1.0)]
        };
        for &(weighted, w) in weights {
            for split in [c.split.as_str(), "all"] {
                let (det, gate) = scores
                    .entry((c.set.clone(), split.to_string(), weighted))
                    .or_default();
                det.tp += w * predicted.intersection(&expected).count() as f64;
                det.fp += w * predicted.difference(&expected).count() as f64;
                det.fn_ += w * expected.difference(&predicted).count() as f64;
                match (gate_expected, gate_predicted) {
                    (true, true) => gate.tp += w,
                    (false, true) => gate.fp += w,
                    (true, false) => gate.fn_ += w,
                    (false, false) => {}
                }
            }
        }
    }
    println!("| set | split | weighting | cases | claim detection P / R | gate (block) P / R |");
    println!("|---|---|---|---|---|---|");
    for ((set, split, weighted), (det, gate)) in &scores {
        let n = cases
            .iter()
            .filter(|c| &c.set == set && (split == "all" || &c.split == split))
            .count();
        let wlabel = if *weighted {
            "by stratum"
        } else {
            "as sampled"
        };
        println!(
            "| {set} | {split} | {wlabel} | {n} | {} | {} |",
            det.pr(),
            gate.pr()
        );
    }
    Ok(())
}
