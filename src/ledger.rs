//! receipt's own run record, written by a PostToolUse hook (`receipt record`).
//!
//! One JSON line per tool call, one file per session. Subagent calls carry the parent's
//! `session_id` and their own `agent_id`, so they land in the same file. Only what a verdict needs
//! is kept: the command, its exit status, and the output lines that report a result.

use crate::rules::KINDS;
use crate::transcript::{codex_command, non_code, Markers, Outcome, Run, Session, Step};
use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;
use std::sync::LazyLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// Unix time in milliseconds.
    pub ts: u128,
    #[serde(default)]
    pub tool_use_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(flatten)]
    pub step: Recorded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Recorded {
    Run {
        command: String,
        /// "ok", "failed", "pending" (background) or "unknown" (the host did not say).
        outcome: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit: Option<i64>,
        /// Output lines that report a result (counts, failure markers), at most `SUMMARY_LINES`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        summary: Vec<String>,
    },
    Edit {
        doc_only: bool,
    },
}

const EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];
const SUMMARY_LINES: usize = 20;
const COMMAND_CHARS: usize = 4000;

static EXIT_CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^Exit code (-?\d+)").unwrap());
static EXIT_IN_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:exit(?:ed)?(?: with)? code|exit_code)\W{0,3}(-?\d+)").unwrap()
});
static PATCH_FILE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\*\*\* (?:Update|Add|Delete) File: ([^\n\\]+)").unwrap());
/// Failure markers of every kind plus common success lines.
static SUMMARY: LazyLock<Regex> = LazyLock::new(|| {
    let mut parts: Vec<String> = KINDS
        .iter()
        .filter_map(|k| k.fail_output)
        .map(|f| f.trim_start_matches("(?m)").to_string())
        .collect();
    parts.push(
        r"test result:|\b\d+ (passed|failed|skipped)\b|^(ok|PASS|FAIL)\b|Tests?:\s+\d+|Finished\b|Build succeeded"
            .to_string(),
    );
    Regex::new(&format!("(?m){}", parts.join("|"))).unwrap()
});

/// Where ledgers live: `$RECEIPT_STATE_DIR`, else `$XDG_STATE_HOME/receipt`, else `~/.local/state/receipt`.
pub fn dir() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os("RECEIPT_STATE_DIR") {
        return Ok(PathBuf::from(d));
    }
    if let Some(d) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(d).join("receipt"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/state/receipt"))
}

fn path(session_id: &str) -> Result<PathBuf> {
    let name: String = session_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    anyhow::ensure!(!name.is_empty(), "unusable session_id `{session_id}`");
    Ok(dir()?.join("sessions").join(format!("{name}.jsonl")))
}

/// Append one PostToolUse / PostToolUseFailure call to its session's ledger. Tools that neither
/// run commands nor edit files are skipped.
pub fn record(input: &Value) -> Result<()> {
    let s = |k: &str| input.get(k).and_then(Value::as_str);
    let session_id = s("session_id").context("hook input has no session_id")?;
    let tool = s("tool_name").unwrap_or("");
    let tool_input = input.get("tool_input").unwrap_or(&Value::Null);
    let failed_event = s("hook_event_name") == Some("PostToolUseFailure");

    let step = if tool == "Bash" {
        let command = match tool_input.get("command").or_else(|| tool_input.get("cmd")) {
            Some(Value::String(c)) => c.clone(),
            other => codex_command(other),
        };
        run(command, input, failed_event)
    } else if tool.starts_with("mcp__") {
        Recorded::Run {
            command: tool.to_string(),
            outcome: if failed_event { "failed" } else { "ok" }.to_string(),
            exit: None,
            summary: vec![],
        }
    } else if EDIT_TOOLS.contains(&tool) {
        let file = tool_input
            .get("file_path")
            .or_else(|| tool_input.get("notebook_path"))
            .and_then(Value::as_str)
            .unwrap_or("");
        Recorded::Edit {
            doc_only: non_code(file),
        }
    } else if tool == "apply_patch" {
        let text = tool_input.to_string();
        let files: Vec<&str> = PATCH_FILE
            .captures_iter(&text)
            .map(|c| c.get(1).unwrap().as_str().trim())
            .collect();
        Recorded::Edit {
            doc_only: !files.is_empty() && files.iter().all(|f| non_code(f)),
        }
    } else {
        return Ok(());
    };

    let entry = Entry {
        ts: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis()),
        tool_use_id: s("tool_use_id").unwrap_or("").to_string(),
        agent_id: s("agent_id").map(str::to_string),
        step,
    };
    let path = path(session_id)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    // One write per line with O_APPEND, so parallel tool calls do not interleave.
    let line = serde_json::to_string(&entry)? + "\n";
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()))
        .with_context(|| format!("append to {}", path.display()))
}

fn run(mut command: String, input: &Value, failed_event: bool) -> Recorded {
    if command.chars().count() > COMMAND_CHARS {
        command = command.chars().take(COMMAND_CHARS).collect();
    }
    let response = input.get("tool_response").unwrap_or(&Value::Null);
    let output = if failed_event {
        input
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    } else {
        match response {
            Value::String(t) => t.clone(),
            Value::Object(o) => [
                "stdout",
                "stderr",
                "output",
                "aggregated_output",
                "formatted_output",
            ]
            .iter()
            .filter_map(|k| o.get(*k).and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
            _ => String::new(),
        }
    };
    let number = |k: &str| response.get(k).and_then(Value::as_i64);
    let (outcome, exit) = if failed_event {
        let code = EXIT_CODE.captures(&output).and_then(|c| c[1].parse().ok());
        ("failed", code)
    } else if response.get("interrupted").and_then(Value::as_bool) == Some(true) {
        ("failed", None)
    } else if response.get("backgroundTaskId").is_some() {
        ("pending", None)
    } else if let Some(code) = number("exit_code").or_else(|| number("exitCode")) {
        (if code == 0 { "ok" } else { "failed" }, Some(code))
    } else if response.get("stdout").is_some() {
        // Claude Code reports a non-zero exit as PostToolUseFailure, so this one exited 0.
        ("ok", Some(0))
    } else if let Some(code) = EXIT_IN_TEXT
        .captures(&output)
        .and_then(|c| c[1].parse::<i64>().ok())
    {
        (if code == 0 { "ok" } else { "failed" }, Some(code))
    } else {
        ("unknown", None)
    };
    let mut summary: Vec<String> = output
        .lines()
        .filter(|l| SUMMARY.is_match(l))
        .map(|l| l.trim().chars().take(200).collect())
        .collect();
    if summary.len() > SUMMARY_LINES {
        summary.drain(..summary.len() - SUMMARY_LINES);
    }
    Recorded::Run {
        command,
        outcome: outcome.to_string(),
        exit,
        summary,
    }
}

/// The session's ledger, or `None` when nothing was recorded for it.
pub fn load(session_id: &str) -> Result<Option<Vec<Entry>>> {
    let path = path(session_id)?;
    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut entries = vec![];
    for (n, line) in lines.iter().enumerate() {
        match serde_json::from_str(line) {
            Ok(e) => entries.push(e),
            // A hook may still be appending the last line.
            Err(_) if n + 1 == lines.len() => {}
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("{}:{} is not a ledger entry", path.display(), n + 1))
            }
        }
    }
    Ok(Some(entries))
}

/// Whether the ledger saw this transcript's first command, i.e. it was recording from the start.
/// A ledger that starts mid-session would hide the earlier runs, so it must not be trusted.
pub fn covers(entries: &[Entry], transcript: &Session) -> bool {
    let first = transcript.steps.iter().find_map(|s| match s {
        Step::Run(r) if !r.command.contains("__") => Some(r),
        _ => None,
    });
    let Some(first) = first else { return true };
    entries.iter().any(|e| match &e.step {
        Recorded::Run { command, .. } => {
            (!first.id.is_empty() && e.tool_use_id == first.id) || *command == first.command
        }
        Recorded::Edit { .. } => false,
    })
}

/// The steps the verdict uses: the ledger's runs and edits (only `agent`'s for a subagent stop).
/// Background runs and runs whose exit status the host did not report take their outcome from the
/// transcript; the final message and agent-report flag also come from the transcript.
pub fn session(
    entries: &[Entry],
    agent: Option<&str>,
    transcript: &Session,
    markers: &Markers,
) -> Session {
    let from_transcript = |id: &str, command: &str| {
        transcript.steps.iter().rev().find_map(|s| match s {
            Step::Run(r)
                if (!id.is_empty() && r.id == id) || (id.is_empty() && r.command == command) =>
            {
                Some((r.outcome.clone(), r.output_failures.clone()))
            }
            _ => None,
        })
    };
    let steps = entries
        .iter()
        .filter(|e| agent.is_none_or(|a| e.agent_id.as_deref() == Some(a)))
        .map(|e| match &e.step {
            Recorded::Edit { doc_only } => Step::Edit {
                doc_only: *doc_only,
            },
            Recorded::Run {
                command,
                outcome,
                exit,
                summary,
            } => {
                let recorded = markers.output_failures(command, &summary.join("\n"));
                let (outcome, output_failures) = match outcome.as_str() {
                    "ok" => (Outcome::Ok, recorded),
                    "failed" => (Outcome::Failed(*exit), recorded),
                    other => match from_transcript(&e.tool_use_id, command)
                        .or_else(|| from_transcript("", command))
                    {
                        Some((o, mut seen))
                            if !matches!(o, Outcome::NoResult | Outcome::Pending) =>
                        {
                            // Union of what the transcript output and the recorded summary show.
                            for k in recorded {
                                if !seen.contains(&k) {
                                    seen.push(k);
                                }
                            }
                            (o, seen)
                        }
                        _ if other == "pending" => (Outcome::Pending, recorded),
                        _ => (Outcome::NoResult, recorded),
                    },
                };
                Step::Run(Run {
                    id: e.tool_use_id.clone(),
                    command: command.clone(),
                    outcome,
                    output_failures,
                })
            }
        })
        .collect();
    Session {
        steps,
        last_text: transcript.last_text.clone(),
        agent_reports: transcript.agent_reports,
    }
}
