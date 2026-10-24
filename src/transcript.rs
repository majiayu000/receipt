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
    /// The host's id for this call (Claude Code `tool_use_id`, Codex item or call id). May be empty.
    pub id: String,
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
/// Programs whose first argument names the tool (`cargo clippy`, `go vet`, `npm run lint`).
const LAUNCHERS: &[&str] = &[
    "cargo",
    "go",
    "npm",
    "pnpm",
    "yarn",
    "bun",
    "deno",
    "make",
    "just",
    "gradle",
    "gradlew",
    "./gradlew",
    "mvn",
    "swift",
    "dotnet",
    "zig",
    "uv",
];
static SCRIPT_RUNNER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\S*/)?(python[0-9.]*|node|bun|deno|ruby|bash|sh|zsh)$").unwrap()
});
static SCRIPT_FILE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\.(py|js|mjs|ts|rb|sh)$").unwrap());
static HEREDOC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<<-?\s*['"]?(\w+)['"]?"#).unwrap());
/// Prefixes that run the next word as the command.
static WRAPPER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^(\w+=\S*\s+|(time|nice|nohup|command|exec|xargs|sudo)\s+|env(\s+-u\s+\S+|\s+-i)*\s+|timeout\s+\S+\s+|",
        r"(fnm|mise|volta|nvm|asdf)\s+(exec|run)\b.*?\s--\s+|",
        r"uv\s+run\s+(--(with|from|python|project|directory)[= ]\S+\s+|--(no-project|frozen|locked|isolated)\s+)*|",
        r"(poetry|pipenv|pdm|hatch|rye)\s+run\s+|uvx\s+|npx\s+(--yes\s+|-y\s+)?|bunx\s+|bun\s+x\s+|pnpm\s+(exec|dlx)\s+|",
        r"(\S*/)?python[0-9.]*\s+-m\s+)"
    ))
    .unwrap()
});
static SHELL_C: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^(\S*/)?(?:ba|z)?sh\s+-\w*c\s+(?:'([^']*)'|"([^"]*)")"#).unwrap()
});
// The groups name what is redefined: `name() {`, `function name`, `alias name=`.
static REDEFINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|[;&|\n]\s*)(function\s+)?(?:(\w[\w.-]*)\s*\(\)\s*\{|alias\s+(\w[\w.-]*)=)")
        .unwrap()
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
    /// Kinds whose evidence names a program this command executes and whose failure markers
    /// match its output.
    pub fn output_failures(&self, command: &str, output: &str) -> Vec<&'static str> {
        if output.is_empty() {
            return vec![];
        }
        self.kinds
            .iter()
            .filter_map(|(name, ev, fail)| {
                (fail.as_ref().is_some_and(|f| f.is_match(output)) && runs(ev, command))
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

/// The commands a shell line actually executes, one per segment, each starting at its program.
///
/// Evidence must name the program of a segment: `echo cargo test` executes `echo`, a heredoc
/// written to a file executes nothing, and a Python string `["cargo", "test"]` is not a command.
/// Wrappers are peeled off (`timeout 60 cargo test`, `uv run pytest`, `python -m pytest`,
/// `bash -c "cargo test"`). Segments that only read or print are dropped, and so is a segment
/// whose program the same line redefines (`cargo() { echo ok; }`, `alias pytest=…`).
pub fn executed(command: &str) -> Vec<String> {
    if WRITE_HEREDOC.is_match(command) {
        return vec![];
    }
    let redefined: Vec<String> = REDEFINE
        .captures_iter(command)
        .filter_map(|c| {
            c.get(3)
                .or_else(|| c.get(4))
                .map(|m| m.as_str().to_string())
        })
        .collect();
    let command = &shell_lines(command);
    let mut out = vec![];
    for seg in segments(command) {
        let mut seg = seg.trim();
        loop {
            let before = seg;
            if let Some(m) = WRAPPER.find(seg) {
                seg = seg[m.end()..].trim_start();
            }
            if seg == before {
                break;
            }
        }
        if let Some(c) = SHELL_C.captures(seg) {
            let inner = c.get(2).or_else(|| c.get(3)).map_or("", |m| m.as_str());
            out.extend(executed(inner));
            continue;
        }
        // A wrapper script handed a command (`python3 gpulock.py cargo test`) runs that command.
        let tokens: Vec<&str> = seg.split_whitespace().collect();
        if tokens.len() > 2
            && SCRIPT_RUNNER.is_match(tokens[0])
            && SCRIPT_FILE.is_match(tokens[1])
            && LAUNCHERS.contains(&tokens[2])
        {
            let rest = seg.find(tokens[2]).map_or("", |i| &seg[i..]);
            out.extend(executed(rest));
            continue;
        }
        if seg.is_empty() || READER.is_match(seg) {
            continue;
        }
        let program = seg.split_whitespace().next().unwrap_or("");
        let program = program.rsplit('/').next().unwrap_or(program);
        if redefined.iter().any(|r| r == program) {
            continue;
        }
        // A path to a tool counts as the tool: `/usr/bin/python3 -m pytest`, `.venv/bin/ruff check`.
        let first = seg.split_whitespace().next().unwrap_or("");
        let seg = if seg.starts_with('/') || seg.starts_with("~/") || first.contains("/bin/") {
            let rest = &seg[seg.find(char::is_whitespace).unwrap_or(seg.len())..];
            format!("{program}{rest}")
        } else {
            seg.to_string()
        };
        out.push(seg);
    }
    out
}

/// The lines a shell executes: a heredoc body is kept only when it is fed to a shell
/// (`bash <<EOF`); a `python3 - <<'PY'` body is Python, not commands.
fn shell_lines(command: &str) -> String {
    let mut out = vec![];
    let mut body: Option<(String, bool)> = None; // (terminator, body is shell)
    for line in command.lines() {
        if let Some((end, shell)) = &body {
            if line.trim() == end {
                body = None;
            } else if *shell {
                out.push(line);
            }
            continue;
        }
        out.push(line);
        if let Some(c) = HEREDOC.captures(line) {
            let program = line.split_whitespace().next().unwrap_or("");
            let program = program.rsplit('/').next().unwrap_or(program);
            body = Some((c[1].to_string(), matches!(program, "sh" | "bash" | "zsh")));
        }
    }
    out.join("\n")
}

/// Whether an evidence rule names the program of one of the executed segments, or the subcommand
/// of a launcher (`cargo clippy`, `go vet`). MCP tool names are matched whole.
pub fn runs(evidence: &Regex, command: &str) -> bool {
    if command.starts_with("mcp__") {
        return evidence.is_match(command);
    }
    executed(command).iter().any(|seg| {
        let program = seg.split_whitespace().next().unwrap_or("");
        let second = seg
            .find(char::is_whitespace)
            .filter(|_| LAUNCHERS.contains(&program))
            .map(|i| i + seg[i..].len() - seg[i..].trim_start().len());
        evidence
            .find_iter(seg)
            .any(|m| m.start() == 0 || Some(m.start()) == second)
    })
}

/// Split on `&&`, `||`, `;`, `|` and newlines outside quotes (`rg 'a|b'` and a multi-line
/// `git commit -m "…"` each stay one segment). Comment lines are skipped, so an apostrophe in a
/// comment cannot open a quote.
fn segments(command: &str) -> Vec<&str> {
    let mut out = vec![];
    let (mut start, mut quote) = (0, None::<char>);
    let mut chars = command.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '#') if command[start..i].trim().is_empty() => {
                // A comment runs to the end of the line.
                while chars.peek().is_some_and(|(_, n)| *n != '\n') {
                    chars.next();
                }
                start = chars.peek().map_or(command.len(), |(j, _)| *j);
            }
            (None, '\'' | '"') => quote = Some(c),
            (None, '\n') => {
                out.push(&command[start..i]);
                start = i + 1;
            }
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
    fn push_run(&mut self, id: &str, command: String, outcome: Outcome, output: &str) -> usize {
        let output_failures = self.markers.output_failures(&command, output);
        self.s.steps.push(Step::Run(Run {
            id: id.to_string(),
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
            let idx = self.push_run(&id, command, Outcome::NoResult, "");
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
                let idx = self.push_run(&id, name.to_string(), Outcome::NoResult, "");
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
                self.codex_run(id_of(p), command, outcome, out);
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
                self.codex_run(id_of(item), command, outcome, out);
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
                self.push_run(id_of(item), format!("mcp__{server}__{tool}"), outcome, "");
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

    fn codex_run(&mut self, id: &str, command: String, outcome: Outcome, out: &str) {
        if BASH_EDIT.is_match(&command) {
            self.s.steps.push(Step::Edit {
                doc_only: bash_edit_non_code(&command),
            });
        }
        self.push_run(id, command, outcome, out);
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
fn id_of(v: &Value) -> &str {
    v.get("call_id")
        .or_else(|| v.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

pub fn codex_command(v: Option<&Value>) -> String {
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
        assert_eq!(
            segments("git commit -m \"fix\n\nbody\" && git push"),
            ["git commit -m \"fix\n\nbody\" ", " git push"]
        );
        assert_eq!(segments("# don't\ncargo test"), ["", "cargo test"]);
    }

    #[test]
    fn only_executed_programs_are_evidence() {
        let test = Regex::new(crate::rules::kind("test").unwrap().evidence).unwrap();
        for forged in [
            "echo cargo test",
            "echo \"test result: ok. 12 passed\"",
            "printf 'cargo test'",
            "rg -n 'publish|cargo test|ci' src",
            "cat > run.sh <<'EOF'\ncargo test\nEOF",
            "python3 - <<'PY'\nsubprocess.run(['cargo', 'test'])\nPY",
            "cargo() { echo ok; }; cargo test",
            "alias pytest=true; pytest",
            "python3 - <<'PY'\nfrom unittest.mock import patch\n# cargo test runs later\nPY",
        ] {
            assert!(!runs(&test, forged), "counted as a test run: {forged}");
        }
        for real in [
            "cargo test",
            "cd x && cargo test 2>&1 | tail -3",
            "RUST_LOG=debug timeout 600 cargo test --all",
            "uv run python -m pytest -q",
            "/usr/bin/python3 -m pytest",
            "bash -lc 'cargo test'",
            "npx vitest run",
            "export PATH=\"$HOME/.cargo/bin:$PATH\"; cargo test",
            "cargo +nightly test",
            "bash <<'EOF'\ncargo test\nEOF",
            "env -u DATABASE_URL CARGO_BUILD_JOBS=2 cargo test",
            "fnm exec --using=24.14.0 -- pnpm test",
            ".venv/bin/pytest -q",
            "python3 /tmp/gpulock.py cargo test --release",
        ] {
            assert!(runs(&test, real), "not counted as a test run: {real}");
        }
    }
}
