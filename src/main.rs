//! receipt CLI.
//!
//!   receipt hook                        Stop / SubagentStop hook for Claude Code and Codex (hook JSON on stdin)
//!   receipt check <transcript> [--message TEXT]
//!                                       report claims and verdicts; exit 1 if any claim would block
//!   receipt audit [--show N] <transcript>...
//!                                       replay every stop of past sessions: what would have been blocked
//!   receipt install [--codex] [--project DIR] [--dry-run]
//!                                       add the hook to Claude Code (default) or Codex settings

mod install;

use anyhow::{bail, Context, Result};
use receipt::{feedback, transcript, Checker, Config, Finding, Verdict};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage: receipt hook
       receipt check <transcript.jsonl> [--message TEXT]
       receipt audit [--show N] <transcript.jsonl>...
       receipt install [--codex] [--project DIR] [--dry-run]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("hook" | "--hook") => hook(),
        Some("check") => check(&args[1..]),
        Some("audit") => audit(&args[1..]),
        Some("install") => install::run(&args[1..]),
        Some("-V" | "--version") => {
            println!("receipt {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        Some("-h" | "--help") => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        _ => Err(anyhow::anyhow!(USAGE)),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            // Exit 1 is a visible, non-blocking hook error on both hosts: never block on our own failure.
            eprintln!("receipt: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn read(path: &str) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("read transcript {path}"))
}

fn hook() -> Result<ExitCode> {
    if std::env::var("RECEIPT_OFF").is_ok_and(|v| v == "1") {
        return Ok(ExitCode::SUCCESS);
    }
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .context("read hook input")?;
    let input: Value = serde_json::from_str(&buf).context("hook input is not JSON")?;
    let str_field = |k: &str| {
        input
            .get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };

    let cwd = str_field("cwd").map(Path::new);
    let checker = Checker::new(&Config::load(cwd)?)?;
    // SubagentStop: check the subagent against its own transcript.
    let path = str_field("agent_transcript_path")
        .or_else(|| str_field("transcript_path"))
        .context("hook input has no transcript_path")?;

    // The host's own copy of the final message is fresher than the transcript file.
    let message = str_field("last_assistant_message").map(str::to_string);
    if let Some(m) = &message {
        if checker.claims(m).is_empty() {
            return Ok(ExitCode::SUCCESS);
        }
    }
    let raw = read(path)?;
    let text = match message {
        Some(m) => m,
        None => transcript::parse_str(&raw, &checker.markers)?.last_text,
    };
    let findings = checker.check_transcript(&checker.claims(&text), &raw)?;
    let missing: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.verdict != Verdict::Supported)
        .collect();
    if missing.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }

    let already_continued = input.get("stop_hook_active").and_then(Value::as_bool) == Some(true);
    if findings.iter().any(|f| f.blocks) && !already_continued {
        eprintln!("{}", feedback(&findings));
        return Ok(ExitCode::from(2));
    }
    // Blocked once already this cycle, or only warnings: tell the user, let the agent stop.
    let kinds: Vec<String> = missing
        .iter()
        .map(|f| format!("{} ({})", f.claim.kind, f.verdict.label()))
        .collect();
    let lead = if already_continued {
        "still unverified"
    } else {
        "unverified"
    };
    println!(
        "{}",
        json!({ "systemMessage": format!("receipt: {lead}: {}", kinds.join(", ")) })
    );
    Ok(ExitCode::SUCCESS)
}

fn check(args: &[String]) -> Result<ExitCode> {
    let mut path = None;
    let mut message = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--message" => message = Some(it.next().context("--message needs a value")?.clone()),
            _ if path.is_none() => path = Some(a.clone()),
            _ => bail!(USAGE),
        }
    }
    let path = path.context(USAGE)?;
    let checker = Checker::new(&Config::load(std::env::current_dir().ok().as_deref())?)?;
    let raw = read(&path)?;
    let text = match message {
        Some(m) => m,
        None => transcript::parse_str(&raw, &checker.markers)?.last_text,
    };
    let findings = checker.check_transcript(&checker.claims(&text), &raw)?;
    if findings.is_empty() {
        println!("no completion claims in the final message");
        return Ok(ExitCode::SUCCESS);
    }
    for f in &findings {
        println!(
            "{:<13} {:<9} {}\n              claim: \"{}\"",
            f.verdict.label(),
            f.claim.kind,
            f.detail,
            f.claim.text
        );
    }
    Ok(if findings.iter().any(|f| f.blocks) {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

/// Replay every turn-final message of past sessions through the hook logic.
fn audit(args: &[String]) -> Result<ExitCode> {
    let mut show = 20;
    let mut files = vec![];
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--show" => show = it.next().context("--show needs a number")?.parse()?,
            _ => files.push(a.clone()),
        }
    }
    if files.is_empty() {
        bail!(USAGE);
    }
    let checker = Checker::new(&Config::load(std::env::current_dir().ok().as_deref())?)?;
    let (mut stops, mut with_claims, mut blocked, mut warned, mut relayed) = (0, 0, 0, 0, 0);
    let mut verdicts: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    let mut listed = vec![];
    for file in &files {
        let raw = read(file)?;
        let lines: Vec<&str> = raw.lines().collect();
        for (upto, text) in transcript::turn_finals(&lines).1 {
            stops += 1;
            let claims = checker.claims(&text);
            if claims.is_empty() {
                continue;
            }
            with_claims += 1;
            let findings = checker.check_transcript(&claims, &lines[..upto].join("\n"))?;
            for f in &findings {
                *verdicts
                    .entry((f.claim.kind, f.verdict.label()))
                    .or_default() += 1;
            }
            if findings.iter().any(|f| f.blocks) {
                blocked += 1;
                if listed.len() < show {
                    listed.push((format!("{file}:{upto}"), findings));
                }
            } else if findings.iter().any(|f| f.verdict != Verdict::Supported) {
                warned += 1;
                relayed += usize::from(findings.iter().any(|f| f.relayed));
            }
        }
    }
    for (place, findings) in &listed {
        println!("{place}");
        for f in findings.iter().filter(|f| f.blocks) {
            let claim: String = f.claim.text.chars().take(100).collect();
            println!(
                "  [{}] {} \"{claim}\" — {}",
                f.claim.kind,
                f.verdict.label(),
                f.detail
            );
        }
    }
    println!(
        "stops: {stops}, with claims: {with_claims}, would block: {blocked}, would warn: {warned} ({relayed} relayed)"
    );
    for ((kind, verdict), n) in verdicts {
        println!("  {kind:<9} {verdict:<13} {n}");
    }
    Ok(ExitCode::SUCCESS)
}
