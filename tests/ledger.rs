//! The run ledger: `receipt record` (PostToolUse) feeding `receipt hook` (Stop).
//! Payload shapes are the ones Claude Code 2.1.300 sends (see docs/design.md).

use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

struct Env {
    dir: PathBuf,
}

impl Env {
    fn new(name: &str) -> Env {
        let dir =
            std::env::temp_dir().join(format!("receipt-ledger-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Env { dir }
    }

    /// Run a receipt subcommand with this test's state dir; returns (exit, stdout, stderr).
    fn receipt(&self, sub: &str, input: &Value) -> (i32, String, String) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_receipt"))
            .arg(sub)
            .env("RECEIPT_STATE_DIR", self.dir.join("state"))
            .env("XDG_CONFIG_HOME", &self.dir)
            .env_remove("RECEIPT_OFF")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.to_string().as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        let text = |b: Vec<u8>| String::from_utf8(b).unwrap();
        (
            out.status.code().unwrap(),
            text(out.stdout),
            text(out.stderr),
        )
    }

    fn ok(&self, id: &str, command: &str, stdout: &str, agent: Option<&str>) {
        let mut input = json!({
            "session_id": "s1", "hook_event_name": "PostToolUse", "tool_name": "Bash",
            "tool_input": { "command": command }, "tool_use_id": id,
            "tool_response": { "stdout": stdout, "stderr": "", "interrupted": false, "isImage": false },
        });
        if let Some(a) = agent {
            input["agent_id"] = a.into();
        }
        assert_eq!(self.receipt("record", &input).0, 0);
    }

    fn failed(&self, id: &str, command: &str, error: &str, agent: Option<&str>) {
        let mut input = json!({
            "session_id": "s1", "hook_event_name": "PostToolUseFailure", "tool_name": "Bash",
            "tool_input": { "command": command }, "tool_use_id": id, "error": error, "is_interrupt": false,
        });
        if let Some(a) = agent {
            input["agent_id"] = a.into();
        }
        assert_eq!(self.receipt("record", &input).0, 0);
    }

    fn edit(&self, id: &str, file: &str) {
        let input = json!({
            "session_id": "s1", "hook_event_name": "PostToolUse", "tool_name": "Edit",
            "tool_input": { "file_path": file }, "tool_use_id": id, "tool_response": {},
        });
        assert_eq!(self.receipt("record", &input).0, 0);
    }

    /// A Claude Code transcript with these Bash calls, all exiting 0.
    fn transcript(&self, name: &str, runs: &[(&str, &str)]) -> PathBuf {
        let mut lines = vec![];
        for (id, command) in runs {
            lines.push(json!({"type": "assistant", "message": {"content": [
                {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": command}}]}}));
            lines.push(json!({"type": "user", "message": {"content": [
                {"type": "tool_result", "tool_use_id": id, "content": "ok"}]}}));
        }
        let path = self.dir.join(name);
        let body: Vec<String> = lines.iter().map(Value::to_string).collect();
        std::fs::write(&path, body.join("\n")).unwrap();
        path
    }

    fn stop(&self, transcript: &Path, message: &str) -> (i32, String, String) {
        self.receipt(
            "hook",
            &json!({ "session_id": "s1", "hook_event_name": "Stop", "transcript_path": transcript,
                     "stop_hook_active": false, "last_assistant_message": message }),
        )
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn echoed_test_results_are_not_evidence() {
    let env = Env::new("echo");
    let runs = [
        ("t1", "echo cargo test"),
        ("t2", "echo \"test result: ok. 12 passed; 0 failed\""),
    ];
    env.ok("t1", runs[0].1, "cargo test", None);
    env.ok(
        "t2",
        runs[1].1,
        "test result: ok. 12 passed; 0 failed",
        None,
    );
    let t = env.transcript("echo.jsonl", &runs);

    let (code, _, stderr) = env.stop(&t, "All 12 tests passed.");
    assert_eq!(code, 2, "{stderr}");
    assert!(
        stderr.contains("UNSUPPORTED: no test command ran"),
        "{stderr}"
    );
    assert!(stderr.contains("receipt's own record"), "{stderr}");

    // Without a ledger the transcript gives the same answer: echo is never a test run.
    let empty = Env::new("echo-no-ledger");
    let (code, _, stderr) = empty.stop(&t, "All 12 tests passed.");
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("no independent run record"), "{stderr}");
}

#[test]
fn real_runs_in_the_ledger_pass() {
    let env = Env::new("real");
    env.edit("e1", "src/lib.rs");
    env.ok(
        "t1",
        "cargo test 2>&1 | tail -2",
        "test result: ok. 3 passed; 0 failed",
        None,
    );
    let t = env.transcript("real.jsonl", &[("t1", "cargo test 2>&1 | tail -2")]);
    let (code, stdout, stderr) = env.stop(&t, "All tests pass.");
    assert_eq!((code, stdout.as_str(), stderr.as_str()), (0, "", ""));

    // The recorded summary still catches a piped failure that exited 0.
    let env = Env::new("masked");
    env.ok(
        "t1",
        "cargo test 2>&1 | tail -2",
        "test result: FAILED. 2 passed; 1 failed",
        None,
    );
    let t = env.transcript("masked.jsonl", &[("t1", "cargo test 2>&1 | tail -2")]);
    let (code, _, stderr) = env.stop(&t, "All tests pass.");
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("output reports failures"), "{stderr}");
}

#[test]
fn transcript_runs_missing_from_the_ledger_do_not_count() {
    let env = Env::new("missing");
    env.ok("t1", "ls", "Cargo.toml", None);
    // The transcript shows a passing `cargo test` that the recorder never saw.
    let t = env.transcript("missing.jsonl", &[("t1", "ls"), ("t2", "cargo test")]);
    let (code, _, stderr) = env.stop(&t, "All tests pass.");
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("UNSUPPORTED"), "{stderr}");
}

#[test]
fn a_ledger_that_started_mid_session_is_not_trusted() {
    let env = Env::new("partial");
    env.ok("t2", "git status", "clean", None);
    let t = env.transcript(
        "partial.jsonl",
        &[("t1", "cargo test"), ("t2", "git status")],
    );
    // The transcript's passing `cargo test` predates the record, so the transcript decides.
    let (code, stdout, stderr) = env.stop(&t, "All tests pass.");
    assert_eq!((code, stdout.as_str(), stderr.as_str()), (0, "", ""));
}

#[test]
fn subagent_runs_and_edits_count_for_the_parent_and_the_subagent() {
    let env = Env::new("sub");
    // A subagent ran the tests and they failed; the parent's transcript shows no runs at all.
    env.failed(
        "a1",
        "cargo test",
        "Exit code 101\ntest result: FAILED. 1 failed",
        Some("agent-1"),
    );
    let parent = env.transcript("parent.jsonl", &[]);
    let (code, _, stderr) = env.stop(&parent, "Tests pass.");
    assert_eq!(code, 2, "{stderr}");
    assert!(
        stderr.contains("CONTRADICTED") && stderr.contains("exit 101"),
        "{stderr}"
    );

    // The subagent fixes the code and re-runs: now the parent's claim is backed.
    env.edit("a2", "src/lib.rs");
    env.ok(
        "a3",
        "cargo test",
        "test result: ok. 4 passed",
        Some("agent-1"),
    );
    let (code, _, stderr) = env.stop(&parent, "Tests pass.");
    assert_eq!(code, 0, "{stderr}");

    // SubagentStop judges only that subagent's own calls.
    let sub_t = env.transcript(
        "agent-1.jsonl",
        &[("a1", "cargo test"), ("a3", "cargo test")],
    );
    let (code, _, stderr) = env.receipt(
        "hook",
        &json!({ "session_id": "s1", "hook_event_name": "SubagentStop", "agent_id": "agent-2",
                 "transcript_path": parent, "agent_transcript_path": sub_t,
                 "last_assistant_message": "Tests pass." }),
    );
    assert_eq!(
        code, 2,
        "another subagent's run must not back agent-2: {stderr}"
    );
}

#[test]
fn unrelated_tools_are_not_recorded() {
    let env = Env::new("read");
    let input = json!({ "session_id": "s1", "hook_event_name": "PostToolUse", "tool_name": "Read",
                        "tool_input": { "file_path": "a.rs" }, "tool_response": {} });
    assert_eq!(env.receipt("record", &input).0, 0);
    assert!(!env.dir.join("state/sessions/s1.jsonl").exists());
}
