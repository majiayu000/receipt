//! Build eval candidates from local transcripts without copying them.
//!
//!   cargo run --release --example extract -- <out.jsonl> <per-stratum> <transcript.jsonl>...
//!
//! For every turn-final assistant message, sentences that hit a broad keyword net become
//! candidates. Only the redacted sentence and normalised command metadata are written:
//! program + subcommand for runs that match some evidence rule, outcome, failure flag, and
//! whether each edit was code or docs. Labels are added by hand afterwards.

use receipt::claims::sentences;
use receipt::transcript::{parse_str, turn_finals, Outcome, Step};
use receipt::{Checker, Config};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashSet;

const BROAD: &str = concat!(
    r"(?i)pass|green|succe|compil|build|clean|lint|clippy|warning|type.?check|format|fmt|commit|push|\bPR\b|pull request|",
    r"deploy|publish|releas|测试|单测|用例|通过|编译|构建|检查|警告|格式|提交|推送|部署|发布|上线"
);

struct Candidate {
    file: String,
    /// Number of transcript lines up to and including the final message.
    upto: usize,
    sentence: String,
    detected: bool,
    host: &'static str,
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    anyhow::ensure!(
        args.len() >= 3,
        "usage: extract <out.jsonl> <per-stratum> <transcript>..."
    );
    let (out_path, per) = (&args[0], args[1].parse::<usize>()?);
    let checker = Checker::new(&Config::default())?;
    let broad = Regex::new(BROAD)?;

    let mut cands = vec![];
    let mut seen = HashSet::new();
    for file in &args[2..] {
        let Ok(raw) = std::fs::read_to_string(file) else {
            continue;
        };
        let lines: Vec<&str> = raw.lines().collect();
        let (host, finals) = turn_finals(&lines);
        for (upto, text) in finals {
            for s in sentences(&text) {
                if s.chars().count() > 300 || !broad.is_match(&s) {
                    continue;
                }
                let red = redact(&s);
                if !seen.insert(red.clone()) {
                    continue;
                }
                let detected = !checker.claims(&s).is_empty();
                cands.push(Candidate {
                    file: file.clone(),
                    upto,
                    sentence: red,
                    detected,
                    host,
                });
            }
        }
    }
    let (pos, neg): (Vec<&Candidate>, Vec<&Candidate>) = cands.iter().partition(|c| c.detected);
    eprintln!(
        "candidates: {} detected, {} broad-only",
        pos.len(),
        neg.len()
    );

    let mut out = String::new();
    let mut n = 0;
    for (stratum, group) in [("detected", &pos), ("broad", &neg)] {
        let mut group: Vec<&&Candidate> = group.iter().collect();
        group.sort_by_key(|c| fnv(&c.sentence));
        for c in group.into_iter().take(per) {
            let raw = std::fs::read_to_string(&c.file)?;
            let prefix: String = raw.lines().take(c.upto).collect::<Vec<_>>().join("\n");
            let session = parse_str(&prefix, &checker.markers)?;
            n += 1;
            let rec = json!({
                "id": format!("r{n:03}"),
                "set": "real",
                "host": c.host,
                "stratum": stratum,
                "stratum_size": if stratum == "detected" { pos.len() } else { neg.len() },
                "text": c.sentence,
                "steps": steps(&session.steps),
            });
            out.push_str(&rec.to_string());
            out.push('\n');
        }
    }
    std::fs::write(out_path, out)?;
    eprintln!("wrote {n} candidates to {out_path}");
    Ok(())
}

/// Keep what decides a verdict: the last 5 evidence runs per kind and the edits after them.
fn steps(steps: &[Step]) -> Vec<Value> {
    let evidence: Vec<Regex> = receipt::rules::KINDS
        .iter()
        .map(|k| Regex::new(k.evidence).unwrap())
        .collect();
    let mut keep = vec![false; steps.len()];
    for ev in &evidence {
        let idx: Vec<usize> = steps
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(s, Step::Run(r) if ev.is_match(&r.command)))
            .map(|(i, _)| i)
            .collect();
        for &i in idx.iter().rev().take(5) {
            keep[i] = true;
        }
    }
    let first = keep.iter().position(|k| *k).unwrap_or(steps.len());
    let mut out: Vec<Value> = vec![];
    for (i, s) in steps.iter().enumerate().skip(first) {
        match s {
            Step::Run(r) if keep[i] => {
                let outcome = match r.outcome {
                    Outcome::Ok => "ok",
                    Outcome::Failed(_) => "failed",
                    Outcome::Pending => "pending",
                    Outcome::NoResult => "no_result",
                    Outcome::NotRun => "not_run",
                };
                // `script`: a multi-line or looped command, where the match may be text rather than a real run.
                let script = r.command.contains('\n')
                    || r.command.contains("<<")
                    || r.command.contains("for ");
                out.push(json!({ "run": normalise(&r.command), "outcome": outcome, "fail_output": !r.output_failures.is_empty(), "fail_output_kinds": r.output_failures, "script": script }));
            }
            Step::Edit { doc_only } => {
                let kind = if *doc_only { "doc" } else { "code" };
                // Consecutive edits of the same kind collapse into one.
                if out
                    .last()
                    .and_then(|v| v.get("edit"))
                    .and_then(Value::as_str)
                    != Some(kind)
                {
                    out.push(json!({ "edit": kind }));
                }
            }
            _ => {}
        }
    }
    out
}

/// The evidence-rule match texts (e.g. `cargo test`): same classification, no paths or arguments.
fn normalise(cmd: &str) -> String {
    let mut parts: Vec<String> = vec![];
    for k in receipt::rules::KINDS {
        if let Some(m) = Regex::new(k.evidence).unwrap().find(cmd) {
            // Arguments with paths, emails or assignments become `<arg>`; the rule still matches.
            let t = m
                .as_str()
                .split_whitespace()
                .map(|t| {
                    if t.contains(['/', '@', '=', '\\', '"', '\'']) {
                        "<arg>"
                    } else {
                        t
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            if !parts.contains(&t) {
                parts.push(t);
            }
        }
    }
    parts.join(" && ")
}

fn redact(s: &str) -> String {
    let rules = [
        (r"https?://\S+", "<url>"),
        (r"[\w.+-]+@[\w-]+\.[\w.]+", "<email>"),
        (
            r"(sk-|ghp_|gho_|github_pat_|xox[bp]-|AKIA)[A-Za-z0-9_\-]{8,}",
            "<secret>",
        ),
        (r"\b[A-Za-z0-9+/_\-]{40,}\b", "<secret>"),
        (
            r"(^|[\s(（:：=`])(~|\.{1,2})?/[^\s`'\x22)\]，。]*",
            "${1}<path>",
        ),
    ];
    let mut out = s.to_string();
    for (pat, rep) in rules {
        out = Regex::new(pat).unwrap().replace_all(&out, rep).into_owned();
    }
    // The local account name, wherever it appears outside a path (e.g. in an email-less byline).
    for name in local_names() {
        out = Regex::new(&format!(r"(?i)\b{}\b", regex::escape(&name)))
            .unwrap()
            .replace_all(&out, "<user>")
            .into_owned();
    }
    out
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}

/// `$USER` and the last component of `$HOME`, so no account name is written into this file.
fn local_names() -> Vec<String> {
    let mut names: Vec<String> = [
        std::env::var("USER").ok(),
        std::env::var("HOME")
            .ok()
            .and_then(|h| h.rsplit('/').next().map(str::to_string)),
    ]
    .into_iter()
    .flatten()
    .filter(|n| n.len() >= 3)
    .collect();
    names.dedup();
    names
}
