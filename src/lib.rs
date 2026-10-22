//! receipt: a completion claim in an agent's final message needs a passing command in its transcript.

pub mod claims;
pub mod rules;
pub mod transcript;

use anyhow::{Context, Result};
use claims::{Claim, Detector};
use regex::Regex;
use rules::{Kind, KINDS};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use transcript::{Markers, Outcome, Session, Step};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Kinds to switch off entirely.
    #[serde(default)]
    pub disable: Vec<String>,
    /// "block" (default) or "warn".
    #[serde(default)]
    pub stale: Option<String>,
    /// Extra evidence regexes per kind name.
    #[serde(default)]
    pub evidence: HashMap<String, Vec<String>>,
}

impl Config {
    /// `<cwd>/.receipt.toml`, else `$XDG_CONFIG_HOME/receipt/config.toml` (or `~/.config/...`).
    pub fn load(cwd: Option<&Path>) -> Result<Config> {
        let mut candidates: Vec<PathBuf> = vec![];
        if let Some(c) = cwd {
            candidates.push(c.join(".receipt.toml"));
        }
        if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
            candidates.push(PathBuf::from(x).join("receipt/config.toml"));
        } else if let Some(h) = std::env::var_os("HOME") {
            candidates.push(PathBuf::from(h).join(".config/receipt/config.toml"));
        }
        for p in candidates {
            if p.is_file() {
                let raw =
                    std::fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?;
                let cfg: Config =
                    toml::from_str(&raw).with_context(|| format!("parse {}", p.display()))?;
                cfg.validate()
                    .with_context(|| format!("invalid {}", p.display()))?;
                return Ok(cfg);
            }
        }
        Ok(Config::default())
    }

    fn validate(&self) -> Result<()> {
        for name in self.disable.iter().chain(self.evidence.keys()) {
            anyhow::ensure!(rules::kind(name).is_some(), "unknown claim kind `{name}`");
        }
        if let Some(s) = &self.stale {
            anyhow::ensure!(
                s == "block" || s == "warn",
                "stale must be \"block\" or \"warn\", got `{s}`"
            );
        }
        Ok(())
    }

    fn stale_blocks(&self) -> bool {
        self.stale.as_deref() != Some("warn")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Supported,
    Stale,
    Contradicted,
    Unsupported,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Supported => "SUPPORTED",
            Verdict::Stale => "STALE",
            Verdict::Contradicted => "CONTRADICTED",
            Verdict::Unsupported => "UNSUPPORTED",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub claim: Claim,
    pub verdict: Verdict,
    pub detail: String,
    pub blocks: bool,
    /// Downgraded to a warning because the session relays other agents' reports.
    pub relayed: bool,
}

/// Compiled rules for one config.
pub struct Checker {
    kinds: Vec<&'static Kind>,
    evidence: Vec<Regex>,
    detector: Detector,
    pub markers: Markers,
    stale_blocks: bool,
}

impl Checker {
    pub fn new(cfg: &Config) -> Result<Checker> {
        let kinds: Vec<&'static Kind> = KINDS
            .iter()
            .filter(|k| !cfg.disable.iter().any(|d| d == k.name))
            .collect();
        let mut evidence = vec![];
        for k in &kinds {
            let mut src = k.evidence.to_string();
            for extra in cfg.evidence.get(k.name).into_iter().flatten() {
                Regex::new(extra)
                    .with_context(|| format!("evidence regex for {}: {extra}", k.name))?;
                src = format!("{src}|{extra}");
            }
            evidence
                .push(Regex::new(&src).with_context(|| format!("evidence regex for {}", k.name))?);
        }
        let markers = Markers {
            kinds: kinds
                .iter()
                .zip(&evidence)
                .map(|(k, ev)| (ev.clone(), k.fail_output.map(|f| Regex::new(f).unwrap())))
                .collect(),
        };
        Ok(Checker {
            detector: Detector::new(&kinds),
            kinds,
            evidence,
            markers,
            stale_blocks: cfg.stale_blocks(),
        })
    }

    pub fn claims(&self, text: &str) -> Vec<Claim> {
        self.detector.detect(text)
    }

    /// Parse a transcript and check the claims against it.
    pub fn check_transcript(&self, claims: &[Claim], raw: &str) -> Result<Vec<Finding>> {
        Ok(self.check(claims, &transcript::parse_str(raw, &self.markers)?))
    }

    pub fn check(&self, claims: &[Claim], s: &Session) -> Vec<Finding> {
        claims.iter().map(|c| self.check_one(c, s)).collect()
    }

    fn check_one(&self, claim: &Claim, s: &Session) -> Finding {
        let i = self
            .kinds
            .iter()
            .position(|k| k.name == claim.kind)
            .expect("claim kind comes from this checker");
        let (kind, ev) = (self.kinds[i], &self.evidence[i]);
        let last = s
            .steps
            .iter()
            .enumerate()
            .rev()
            .find_map(|(idx, st)| match st {
                Step::Run(r)
                    if r.outcome != Outcome::NotRun
                        && ev.is_match(&transcript::evidence_text(&r.command)) =>
                {
                    Some((idx, r))
                }
                _ => None,
            });
        let (verdict, detail) = match last {
            None => (
                Verdict::Unsupported,
                format!("no {} command ran in this session", kind.name),
            ),
            Some((idx, r)) => {
                let cmd = short(&r.command);
                let step = idx + 1;
                match &r.outcome {
                    Outcome::Failed(Some(c)) => (Verdict::Contradicted, format!("the last `{cmd}` (step {step}) failed (exit {c})")),
                    Outcome::Failed(None) => (Verdict::Contradicted, format!("the last `{cmd}` (step {step}) failed")),
                    Outcome::Ok if r.output_failed => {
                        (Verdict::Contradicted, format!("the last `{cmd}` (step {step}) exited 0 but its output reports failures"))
                    }
                    Outcome::Pending => {
                        (Verdict::Unsupported, format!("the last `{cmd}` (step {step}) ran in the background and never reported back"))
                    }
                    Outcome::NoResult => (Verdict::Unsupported, format!("the last `{cmd}` (step {step}) has no recorded result")),
                    Outcome::NotRun => unreachable!("filtered above"),
                    Outcome::Ok => {
                        let edits = s.steps[idx + 1..].iter().filter(|x| matches!(x, Step::Edit { doc_only: false })).count();
                        if kind.stale && edits > 0 {
                            (Verdict::Stale, format!("`{cmd}` passed at step {step}, then {edits} code edit(s) followed"))
                        } else {
                            (Verdict::Supported, format!("`{cmd}` passed at step {step}"))
                        }
                    }
                }
            }
        };
        let mut blocks = match verdict {
            Verdict::Supported => false,
            Verdict::Stale => self.stale_blocks,
            Verdict::Unsupported => !kind.unsupported_warns,
            Verdict::Contradicted => true,
        };
        let mut detail = detail;
        // A verification may have run through a project runner receipt does not know (a harness
        // script, a built binary). If one succeeded after the last code edit, warn instead of block.
        if blocks && kind.stale && matches!(verdict, Verdict::Stale | Verdict::Unsupported) {
            if let Some(step) = opaque_run_after_last_edit(s, &self.evidence) {
                blocks = false;
                detail.push_str(&format!(
                    "; an unrecognised command passed after the last edit (step {step})"
                ));
            }
        }
        // Nothing ran at all, in a session that received other agents' reports: most likely the
        // agent is passing on their result. Failed and stale runs of its own still block.
        let relayed = blocks && verdict == Verdict::Unsupported && s.agent_reports;
        if relayed {
            blocks = false;
            detail.push_str("; this session received reports from other agents, so it may be relaying their result");
        }
        Finding {
            claim: claim.clone(),
            verdict,
            detail,
            blocks,
            relayed,
        }
    }
}

/// The first successful command after the last code edit that no evidence rule knows and that is
/// not housekeeping (git, file moves, process checks) or read-only. Returns its 1-based step number.
fn opaque_run_after_last_edit(s: &Session, evidence: &[Regex]) -> Option<usize> {
    let from = s
        .steps
        .iter()
        .rposition(|x| matches!(x, Step::Edit { doc_only: false }))
        .map_or(0, |i| i + 1);
    s.steps[from..]
        .iter()
        .enumerate()
        .find_map(|(i, st)| match st {
            Step::Run(r) if r.outcome == Outcome::Ok && !r.command.contains("__") => {
                let text = transcript::evidence_text(&r.command);
                let program = text
                    .split_whitespace()
                    .find(|t| !t.contains('='))
                    .unwrap_or("");
                let known = evidence.iter().any(|e| e.is_match(&text));
                (!text.trim().is_empty() && !known && !HOUSEKEEPING.contains(&program))
                    .then_some(from + i + 1)
            }
            _ => None,
        })
}

/// Programs that cannot verify code. Read-only commands are already dropped by `evidence_text`.
const HOUSEKEEPING: &[&str] = &[
    "git", "gh", "cd", "mkdir", "cp", "mv", "rm", "touch", "chmod", "ln", "ps", "pgrep", "pkill",
    "kill", "lsof", "sleep", "wait", "open", "which", "command", "type", "set", "export", "source",
    "true", "date", "pwd", "du", "df",
];

fn short(cmd: &str) -> String {
    let first = cmd
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    let mut s: String = first.chars().take(80).collect();
    if first.chars().count() > 80 || cmd.trim().lines().count() > 1 {
        s.push('…');
    }
    s
}

const MAX_LISTED: usize = 5;

/// The message the agent receives when the stop is blocked.
pub fn feedback(findings: &[Finding]) -> String {
    let blocking: Vec<&Finding> = findings.iter().filter(|f| f.blocks).collect();
    let n = blocking.len();
    let mut out = format!(
        "receipt: {n} claim{} in your final message {} no passing run in this session.\n",
        if n == 1 { "" } else { "s" },
        if n == 1 { "has" } else { "have" }
    );
    for (i, f) in blocking.iter().take(MAX_LISTED).enumerate() {
        out.push_str(&format!(
            "{}. [{}] \"{}\" — {}: {}.\n",
            i + 1,
            f.claim.kind,
            f.claim.text,
            f.verdict.label(),
            f.detail
        ));
    }
    if n > MAX_LISTED {
        out.push_str(&format!("(+{} more)\n", n - MAX_LISTED));
    }
    out.push_str(concat!(
        "FIX: for each item, either run that check once in the foreground and report its real result, ",
        "or reword only that sentence to say what was actually verified (for example \"tests not run\").\n",
        "If a check fails, report the failure. Fix it only if fixing it is part of the task you were given.\n",
        "DO NOT: edit tests or config to make a check pass, add new checks or scripts, or re-run checks that are not listed above.",
    ));
    out
}
