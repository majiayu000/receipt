//! Turn a Claude Code or Codex transcript (JSONL) into an ordered list of steps.
//!
//! Only what the checker needs is kept: commands with their outcome, and file edits.
//! Command output is scanned for failure markers while parsing and then dropped.

use anyhow::{Context, Result};
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Ok,
    Failed(Option<i64>),
    /// Started in the background; no completion seen yet.
    Pending,
    /// The transcript ends before a result was recorded.
    NoResult,
    /// Denied by permissions or a hook: the command never ran.
    NotRun,
}

#[derive(Debug, Clone)]
pub struct Run {
    pub command: String,
    pub outcome: Outcome,
    /// Output matched the failure markers of a kind whose evidence matches this command.
    pub output_failed: bool,
}

#[derive(Debug, Clone)]
pub enum Step {
    Run(Run),
    /// `doc_only`: every edited path is documentation, so verification runs stay fresh.
    Edit { doc_only: bool },
}

#[derive(Debug, Default)]
pub struct Session {
    pub steps: Vec<Step>,
    pub last_text: String,
}

const EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];
const DOC_EXT: &[&str] = &[".md", ".markdown", ".txt", ".rst", ".adoc"];

static BASH_EDIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bsed\s+(-\S*\s+)*-i|\bperl\s+-\S*i|\bapply_patch\b|\bgit\s+apply\b|(^|[;&|]\s*)patch\s").unwrap());
static EXIT_CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^Exit code (-?\d+)").unwrap());
static NOTIFY_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<tool-use-id>([^<]+)</tool-use-id>").unwrap());
static NOTIFY_EXIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(exit code (-?\d+)\)").unwrap());
static NOTIFY_STATUS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<status>([a-z_]+)</status>").unwrap());

/// Per-kind evidence and failure-output regexes, compiled once by the caller.
pub struct Markers {
    pub kinds: Vec<(Regex, Option<Regex>)>,
}

impl Markers {
    pub fn output_failed(&self, command: &str, output: &str) -> bool {
        self.kinds.iter().any(|(ev, fail)| ev.is_match(command) && fail.as_ref().is_some_and(|f| f.is_match(output)))
    }
}

pub fn is_doc(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    DOC_EXT.iter().any(|e| p.ends_with(e))
}

pub fn parse_file(path: &str, markers: &Markers) -> Result<Session> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read transcript {path}"))?;
    parse_str(&raw, markers).with_context(|| format!("parse transcript {path}"))
}

pub fn parse_str(raw: &str, markers: &Markers) -> Result<Session> {
    let mut p = Parser { markers, s: Session::default(), pending: HashMap::new() };
    for (n, line) in raw.lines().enumerate() {
        if line.trim().is_empty() || !interesting(line) {
            continue;
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            // A hook can fire while the host is still appending the last line.
            Err(_) if n + 1 == raw.lines().count() => continue,
            Err(e) => return Err(e).with_context(|| format!("line {} is not JSON", n + 1)),
        };
        if v.get("payload").is_some() {
            p.codex(&v);
        } else {
            p.claude(&v);
        }
    }
    Ok(p.s)
}

/// Cheap substring filter so that large irrelevant lines (snapshots, attachments) skip JSON parsing.
fn interesting(line: &str) -> bool {
    line.contains("\"assistant\"")
        || line.contains("\"tool_result\"")
        || line.contains("task-notification")
        || line.contains("item_completed")
        || line.contains("exec_command_end")
        || line.contains("patch_apply_end")
}

struct Parser<'a> {
    markers: &'a Markers,
    s: Session,
    /// tool_use id -> index of its Run step.
    pending: HashMap<String, usize>,
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

impl Parser<'_> {
    fn push_run(&mut self, command: String, outcome: Outcome, output: &str) -> usize {
        let output_failed = !output.is_empty() && self.markers.output_failed(&command, output);
        self.s.steps.push(Step::Run(Run { command, outcome, output_failed }));
        self.s.steps.len() - 1
    }

    fn run_mut(&mut self, idx: usize) -> &mut Run {
        match &mut self.s.steps[idx] {
            Step::Run(r) => r,
            Step::Edit { .. } => unreachable!("pending index always points at a Run"),
        }
    }

    fn claude(&mut self, v: &Value) {
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return;
        }
        let role = v.get("type").and_then(Value::as_str).unwrap_or("");
        let content = v.pointer("/message/content");
        if role == "user" {
            if let Some(Value::String(s)) = content {
                self.notification(s);
            }
        }
        let Some(blocks) = content.and_then(Value::as_array) else { return };
        let mut text = String::new();
        for b in blocks {
            match (role, b.get("type").and_then(Value::as_str)) {
                ("assistant", Some("text")) => text.push_str(b.get("text").and_then(Value::as_str).unwrap_or("")),
                ("assistant", Some("tool_use")) => self.claude_tool_use(b),
                ("user", Some("tool_result")) => self.claude_tool_result(b, v),
                ("user", Some("text")) => self.notification(b.get("text").and_then(Value::as_str).unwrap_or("")),
                _ => {}
            }
        }
        if !text.trim().is_empty() {
            self.s.last_text = text;
        }
    }

    fn claude_tool_use(&mut self, b: &Value) {
        let name = b.get("name").and_then(Value::as_str).unwrap_or("");
        let id = b.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let input = b.get("input");
        if name == "Bash" {
            let command = input.and_then(|i| i.get("command")).and_then(Value::as_str).unwrap_or("").to_string();
            if BASH_EDIT.is_match(&command) {
                self.s.steps.push(Step::Edit { doc_only: false });
            }
            let idx = self.push_run(command, Outcome::NoResult, "");
            self.pending.insert(id, idx);
        } else if EDIT_TOOLS.contains(&name) {
            let path = input
                .and_then(|i| i.get("file_path").or_else(|| i.get("notebook_path")))
                .and_then(Value::as_str)
                .unwrap_or("");
            self.s.steps.push(Step::Edit { doc_only: is_doc(path) });
        } else if name.contains("__") {
            // MCP tools: the tool name is the "command" (e.g. github create_pull_request).
            let idx = self.push_run(name.to_string(), Outcome::NoResult, "");
            self.pending.insert(id, idx);
        }
    }

    fn claude_tool_result(&mut self, b: &Value, line: &Value) {
        let id = b.get("tool_use_id").and_then(Value::as_str).unwrap_or("");
        let Some(&idx) = self.pending.get(id) else { return };
        let out = text_of(b.get("content").unwrap_or(&Value::Null));
        let is_error = b.get("is_error").and_then(Value::as_bool).unwrap_or(false);
        let interrupted = line.pointer("/toolUseResult/interrupted").and_then(Value::as_bool) == Some(true);
        let outcome = if is_error {
            match EXIT_CODE.captures(&out) {
                Some(c) => Outcome::Failed(c[1].parse().ok()),
                // Permission denied, a PreToolUse hook block, or a tool-level error: nothing ran.
                None if self.run_mut(idx).command.contains("__") => Outcome::Failed(None),
                None => Outcome::NotRun,
            }
        } else if interrupted {
            Outcome::Failed(None)
        } else if out.contains("running in background with ID") {
            Outcome::Pending
        } else {
            Outcome::Ok
        };
        let command = self.run_mut(idx).command.clone();
        let output_failed = self.markers.output_failed(&command, &out);
        let r = self.run_mut(idx);
        r.outcome = outcome;
        r.output_failed = output_failed;
        if r.outcome != Outcome::Pending {
            self.pending.remove(id);
        }
    }

    /// `<task-notification>` reports how a background command ended.
    fn notification(&mut self, s: &str) {
        if !s.contains("<task-notification>") {
            return;
        }
        for chunk in s.split("<task-notification>").skip(1) {
            let Some(id) = NOTIFY_ID.captures(chunk).map(|c| c[1].to_string()) else { continue };
            let Some(&idx) = self.pending.get(&id) else { continue };
            let code = NOTIFY_EXIT.captures(chunk).and_then(|c| c[1].parse::<i64>().ok());
            let status = NOTIFY_STATUS.captures(chunk).map(|c| c[1].to_string());
            let outcome = match (code, status.as_deref()) {
                (Some(0), _) => Outcome::Ok,
                (Some(c), _) => Outcome::Failed(Some(c)),
                (None, Some("completed")) => Outcome::Ok,
                (None, Some(_)) => Outcome::Failed(None),
                (None, None) => continue,
            };
            self.run_mut(idx).outcome = outcome;
            self.pending.remove(&id);
        }
    }

    fn codex(&mut self, v: &Value) {
        let Some(p) = v.get("payload") else { return };
        let ptype = p.get("type").and_then(Value::as_str).unwrap_or("");
        match ptype {
            "item_completed" => {
                if let Some(item) = p.get("item") {
                    self.codex_item(item);
                }
            }
            "exec_command_end" => {
                let command = codex_command(p.get("command"));
                let out = p
                    .get("aggregated_output")
                    .or_else(|| p.get("formatted_output"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let outcome = codex_exit(p.get("exit_code"));
                self.codex_run(command, outcome, out);
            }
            "patch_apply_end" if p.get("success").and_then(Value::as_bool) != Some(false) => {
                self.codex_edit(p.get("changes"));
            }
            _ => {}
        }
    }

    fn codex_item(&mut self, item: &Value) {
        match item.get("type").and_then(Value::as_str).unwrap_or("") {
            "CommandExecution" => {
                let command = codex_command(item.get("command"));
                let out = item.get("aggregated_output").and_then(Value::as_str).unwrap_or("");
                let outcome = match item.get("status").and_then(Value::as_str) {
                    Some("declined") => Outcome::NotRun,
                    _ => codex_exit(item.get("exit_code")),
                };
                self.codex_run(command, outcome, out);
            }
            "FileChange" => {
                if item.get("status").and_then(Value::as_str) != Some("failed") {
                    self.codex_edit(item.get("changes"));
                }
            }
            "McpToolCall" => {
                let server = item.get("server").and_then(Value::as_str).unwrap_or("");
                let tool = item.get("tool").and_then(Value::as_str).unwrap_or("");
                let outcome = match item.get("status").and_then(Value::as_str) {
                    Some("completed") => Outcome::Ok,
                    _ => Outcome::Failed(None),
                };
                self.push_run(format!("mcp__{server}__{tool}"), outcome, "");
            }
            "AgentMessage" => {
                let text = text_of(item.get("content").unwrap_or(&Value::Null));
                if !text.trim().is_empty() {
                    self.s.last_text = text;
                }
            }
            _ => {}
        }
    }

    fn codex_run(&mut self, command: String, outcome: Outcome, out: &str) {
        if BASH_EDIT.is_match(&command) {
            self.s.steps.push(Step::Edit { doc_only: false });
        }
        self.push_run(command, outcome, out);
    }

    fn codex_edit(&mut self, changes: Option<&Value>) {
        let doc_only = match changes.and_then(Value::as_object) {
            Some(m) if !m.is_empty() => m.keys().all(|k| is_doc(k)),
            _ => false,
        };
        self.s.steps.push(Step::Edit { doc_only });
    }
}

fn codex_exit(v: Option<&Value>) -> Outcome {
    let code = match v {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.parse().ok(),
        _ => None,
    };
    match code {
        Some(0) => Outcome::Ok,
        Some(c) => Outcome::Failed(Some(c)),
        None => Outcome::NoResult,
    }
}

/// Codex records argv; `["/bin/zsh", "-lc", "cargo test"]` is the script `cargo test`.
fn codex_command(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => {
            let parts: Vec<&str> = a.iter().filter_map(Value::as_str).collect();
            let shell = parts.first().is_some_and(|p| p.ends_with("sh"));
            if shell && parts.len() >= 3 && parts[1].starts_with('-') {
                parts[2..].join(" ")
            } else {
                parts.join(" ")
            }
        }
        _ => String::new(),
    }
}
