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
    /// Kinds whose evidence matches this command and whose failure markers match its output.
    pub output_failures: Vec<&'static str>,
}

#[derive(Debug, Clone)]
pub enum Step {
    Run(Run),
    /// `doc_only`: every edited path is documentation or scratch, so verification runs stay fresh.
    Edit {
        doc_only: bool,
    },
}

#[derive(Debug, Default)]
pub struct Session {
    pub steps: Vec<Step>,
    pub last_text: String,
    /// The session received reports from other agents: subagent results, messages from other
    /// sessions, session or thread tools. Its claims may describe their work, not its own runs.
    pub agent_reports: bool,
}

const EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];
const DOC_EXT: &[&str] = &[".md", ".markdown", ".txt", ".rst", ".adoc"];
const SCRATCH_DIRS: &[&str] = &[
    "/tmp/",
    "/private/tmp/",
    "/private/var/folders/",
    "/var/folders/",
];

static BASH_EDIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\bsed\s+(-\S*\s+)*-i|\bperl\s+-\S*i|\bapply_patch\b|\bgit\s+apply\b|(^|[;&|]\s*)patch\s",
    )
    .unwrap()
});
static FILE_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\w./~-]+\.[A-Za-z]{1,8}\b").unwrap());
static WRITE_HEREDOC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(cat|tee)\b[^\n]*<<").unwrap());
static READER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^\s*(\w+=\S*\s+)*(rg|grep|egrep|fgrep|sed|cat|head|tail|less|ls|find|echo|printf|jq|awk|wc|diff|nl|pgrep|ps|lsof|",
        r"git\s+(log|show|diff|status|grep|blame))\b"
    ))
    .unwrap()
});
/// Raw-line markers of reports from other agents: messages delivered from other sessions, and
/// Codex's multi-agent tools.
const AGENT_REPORT_MARKERS: &[&str] = &[
    "<cross-session-message",
    "<side-sessions-event",
    "\"name\":\"spawn_agent\"",
    "\"name\":\"wait_agent\"",
    "\"name\":\"send_message\"",
    "\"name\":\"followup_task\"",
];
/// MCP tools that read or drive other sessions, threads or agents.
static AGENT_TOOL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)__.*(session|thread|agent)|^(get|list|read)_(session|thread)").unwrap()
});
static EXIT_CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^Exit code (-?\d+)").unwrap());
static NOTIFY_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<tool-use-id>([^<]+)</tool-use-id>").unwrap());
static NOTIFY_EXIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(exit code (-?\d+)\)").unwrap());
static NOTIFY_STATUS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<status>([a-z_]+)</status>").unwrap());

/// Per-kind evidence and failure-output regexes, compiled once by the caller.
pub struct Markers {
    pub kinds: Vec<(&'static str, Regex, Option<Regex>)>,
}

impl Markers {
    pub fn output_failures(&self, command: &str, output: &str) -> Vec<&'static str> {
        if output.is_empty() {
            return vec![];
        }
        let text = evidence_text(command);
        self.kinds
            .iter()
            .filter_map(|(name, ev, fail)| {
                (ev.is_match(&text) && fail.as_ref().is_some_and(|f| f.is_match(output)))
                    .then_some(*name)
            })
            .collect()
    }
}

/// Edits that cannot change what a check verifies: documentation, and scratch files outside any project.
pub fn non_code(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    DOC_EXT.iter().any(|e| p.ends_with(e)) || SCRATCH_DIRS.iter().any(|d| p.starts_with(d))
}

/// A Bash edit is non-code when every file it names is.
fn bash_edit_non_code(command: &str) -> bool {
    let files: Vec<&str> = FILE_TOKEN.find_iter(command).map(|m| m.as_str()).collect();
    !files.is_empty() && files.iter().all(|f| non_code(f))
}

/// Text used for evidence matching.
///
/// Segments that only read or print (`rg 'cargo test'`, `cat log`) are dropped, as is a heredoc
/// written to a file. Quotes, commas and brackets become spaces, so a harness script's
/// `subprocess.run(["cargo", "test"])` reads as `cargo test`.
pub fn evidence_text(command: &str) -> String {
    if WRITE_HEREDOC.is_match(command) {
        return String::new();
    }
    let kept: Vec<&str> = segments(command)
        .into_iter()
        .filter(|seg| !READER.is_match(seg))
        .collect();
    kept.join(" ; ").replace(['"', '\'', ',', '[', ']'], " ")
}

/// Split on `&&`, `||`, `;`, `|` and newlines outside quotes (`rg 'a|b'` stays one segment).
/// A newline always splits, so an unbalanced quote in a heredoc line cannot swallow the rest.
fn segments(command: &str) -> Vec<&str> {
    let mut out = vec![];
    let (mut start, mut quote) = (0, None::<char>);
    let mut chars = command.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match (quote, c) {
            (_, '\n') => {
                quote = None;
                out.push(&command[start..i]);
                start = i + 1;
            }
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, ';' | '|' | '&') => {
                if c == '&' && chars.peek().map(|(_, n)| *n) != Some('&') {
                    continue;
                }
                out.push(&command[start..i]);
                if matches!(chars.peek(), Some((_, '|' | '&'))) {
                    chars.next();
                }
                start = chars.peek().map_or(command.len(), |(j, _)| *j);
            }
            _ => {}
        }
    }
    out.push(&command[start..]);
    out
}

pub fn parse_file(path: &str, markers: &Markers) -> Result<Session> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read transcript {path}"))?;
    parse_str(&raw, markers).with_context(|| format!("parse transcript {path}"))
}

pub fn parse_str(raw: &str, markers: &Markers) -> Result<Session> {
    let mut p = Parser {
        markers,
        s: Session::default(),
        pending: HashMap::new(),
    };
    for (n, line) in raw.lines().enumerate() {
        if line.trim().is_empty() || !interesting(line) {
            continue;
        }
        // Claude Code delivers these in queue-operation / attachment records; the tags survive JSON escaping.
        if line.contains("<task-notification>") {
            p.notification(line);
        }
        if AGENT_REPORT_MARKERS.iter().any(|m| line.contains(m)) {
            p.s.agent_reports = true;
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
        Value::Array(a) => a
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

impl Parser<'_> {
    fn push_run(&mut self, command: String, outcome: Outcome, output: &str) -> usize {
        let output_failures = self.markers.output_failures(&command, output);
        self.s.steps.push(Step::Run(Run {
            command,
            outcome,
            output_failures,
        }));
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
        let Some(blocks) = v.pointer("/message/content").and_then(Value::as_array) else {
            return;
        };
        let mut text = String::new();
        for b in blocks {
            match (role, b.get("type").and_then(Value::as_str)) {
                ("assistant", Some("text")) => {
                    text.push_str(b.get("text").and_then(Value::as_str).unwrap_or(""))
                }
                ("assistant", Some("tool_use")) => self.claude_tool_use(b),
                ("user", Some("tool_result")) => self.claude_tool_result(b, v),
                _ => {}
            }
        }
        if !text.trim().is_empty() {
            self.s.last_text = text;
        }
    }

    fn claude_tool_use(&mut self, b: &Value) {
        let name = b.get("name").and_then(Value::as_str).unwrap_or("");
        let id = b
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let input = b.get("input");
        if name == "Bash" {
            let command = input
                .and_then(|i| i.get("command"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if BASH_EDIT.is_match(&command) {
                self.s.steps.push(Step::Edit {
                    doc_only: bash_edit_non_code(&command),
                });
            }
            let idx = self.push_run(command, Outcome::NoResult, "");
            self.pending.insert(id, idx);
        } else if EDIT_TOOLS.contains(&name) {
            let path = input
                .and_then(|i| i.get("file_path").or_else(|| i.get("notebook_path")))
                .and_then(Value::as_str)
                .unwrap_or("");
            self.s.steps.push(Step::Edit {
                doc_only: non_code(path),
            });
        } else {
            if matches!(name, "Agent" | "Task" | "SendMessage") || AGENT_TOOL.is_match(name) {
                self.s.agent_reports = true;
            }
            if name.contains("__") {
                // MCP tools: the tool name is the "command" (e.g. github create_pull_request).
                let idx = self.push_run(name.to_string(), Outcome::NoResult, "");
                self.pending.insert(id, idx);
            }
        }
    }

    fn claude_tool_result(&mut self, b: &Value, line: &Value) {
        let id = b.get("tool_use_id").and_then(Value::as_str).unwrap_or("");
        let Some(&idx) = self.pending.get(id) else {
            return;
        };
        let out = text_of(b.get("content").unwrap_or(&Value::Null));
        let is_error = b.get("is_error").and_then(Value::as_bool).unwrap_or(false);
        let interrupted = line
            .pointer("/toolUseResult/interrupted")
            .and_then(Value::as_bool)
            == Some(true);
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
        let output_failures = self.markers.output_failures(&command, &out);
        let r = self.run_mut(idx);
        r.outcome = outcome;
        r.output_failures = output_failures;
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
            let Some(id) = NOTIFY_ID.captures(chunk).map(|c| c[1].to_string()) else {
                continue;
            };
            let Some(&idx) = self.pending.get(&id) else {
                continue;
            };
            let code = NOTIFY_EXIT
                .captures(chunk)
                .and_then(|c| c[1].parse::<i64>().ok());
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
                let out = item
                    .get("aggregated_output")
                    .and_then(Value::as_str)
                    .unwrap_or("");
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
                if AGENT_TOOL.is_match(tool) {
                    self.s.agent_reports = true;
                }
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
            self.s.steps.push(Step::Edit {
                doc_only: bash_edit_non_code(&command),
            });
        }
        self.push_run(command, outcome, out);
    }

    fn codex_edit(&mut self, changes: Option<&Value>) {
        let doc_only = match changes.and_then(Value::as_object) {
            Some(m) if !m.is_empty() => m.keys().all(|k| non_code(k)),
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

/// Turn-final assistant messages, each with the number of transcript lines up to it.
/// Used by the eval and replay tools to look at every stop of a past session.
pub fn turn_finals(lines: &[&str]) -> (&'static str, Vec<(usize, String)>) {
    let codex = lines.iter().take(5).any(|l| l.contains("\"payload\""));
    let mut out = vec![];
    let mut last: Option<(usize, String)> = None;
    for (i, line) in lines.iter().enumerate() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if codex {
            if v.pointer("/payload/type").and_then(Value::as_str) == Some("task_complete") {
                if let Some(t) = v
                    .pointer("/payload/last_agent_message")
                    .and_then(Value::as_str)
                {
                    out.push((i + 1, t.to_string()));
                }
            }
            continue;
        }
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        match v.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                let text: String = v
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|b| b.get("text").and_then(Value::as_str))
                            .collect()
                    })
                    .unwrap_or_default();
                if !text.trim().is_empty() {
                    last = Some((i + 1, text));
                }
            }
            // A real prompt (string content, not a tool result or a notification) ends the turn.
            Some("user") => {
                let prompt = matches!(v.pointer("/message/content"), Some(Value::String(s)) if !s.contains("<task-notification>"));
                if prompt && v.get("isMeta").and_then(Value::as_bool) != Some(true) {
                    out.extend(last.take());
                }
            }
            _ => {}
        }
    }
    out.extend(last);
    (if codex { "codex" } else { "claude" }, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_failures_retain_matching_kind_names() {
        let markers = Markers {
            kinds: vec![
                (
                    "build",
                    Regex::new("cargo clippy").unwrap(),
                    Some(Regex::new("error").unwrap()),
                ),
                (
                    "lint",
                    Regex::new("cargo clippy").unwrap(),
                    Some(Regex::new("warning|error").unwrap()),
                ),
                ("commit", Regex::new("git commit").unwrap(), None),
            ],
        };
        assert_eq!(markers.output_failures("cargo clippy", "warning"), ["lint"]);
        assert_eq!(
            markers.output_failures("cargo clippy", "error"),
            ["build", "lint"]
        );
        assert!(markers.output_failures("git commit", "error").is_empty());
        assert!(markers.output_failures("cargo clippy", "").is_empty());
    }

    #[test]
    fn segments_respect_quotes() {
        assert_eq!(
            segments("rg -n 'a|cargo test' x && cargo build"),
            ["rg -n 'a|cargo test' x ", " cargo build"]
        );
        assert_eq!(segments("a; b | c || d\ne"), ["a", " b ", " c ", " d", "e"]);
    }

    #[test]
    fn readers_and_written_heredocs_are_not_evidence() {
        assert!(!evidence_text("rg -n 'publish|cargo test|ci' src").contains("cargo test"));
        assert!(evidence_text("cat > run.sh <<'EOF'\ncargo test\nEOF").is_empty());
        assert!(evidence_text("cd x && cargo test 2>&1 | tail -3").contains("cargo test"));
        assert!(
            evidence_text("python3 - <<'PY'\nsubprocess.run(['cargo', 'test'])\nPY")
                .contains("cargo    test")
        );
    }
}
