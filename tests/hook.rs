use std::io::Write;
use std::process::{Command, Stdio};

fn fixture(name: &str) -> String {
    format!("{}/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// Run `receipt hook` with the given stdin; returns (exit code, stdout, stderr).
fn hook(input: serde_json::Value) -> (i32, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_receipt"))
        .arg("hook")
        .env("XDG_CONFIG_HOME", env!("CARGO_MANIFEST_DIR")) // no user config leaks into tests
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
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

fn check(name: &str) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_receipt"))
        .args(["check", &fixture(name)])
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
    )
}

#[test]
fn claude_fixtures() {
    let (code, out) = check("honest.jsonl");
    assert_eq!(code, 0, "{out}");
    assert_eq!(out.matches("SUPPORTED").count(), 2, "{out}");

    let (code, out) = check("lying.jsonl");
    assert_eq!(code, 1);
    assert!(out.contains("CONTRADICTED  test"), "{out}");
    assert!(out.contains("UNSUPPORTED   commit"), "{out}");

    let (code, out) = check("stale.jsonl");
    assert_eq!(code, 1);
    assert!(out.contains("STALE         test"), "{out}");
    // `cargo test` compiles the crate, so it is build evidence too, and just as stale.
    assert!(out.contains("STALE         build"), "{out}");
}

#[test]
fn background_and_denied_runs() {
    let (code, out) = check("background.jsonl");
    assert_eq!(code, 1);
    assert!(
        out.contains("SUPPORTED     test      `npm test` passed"),
        "{out}"
    );
    // The push was denied by permissions, so it never ran.
    assert!(
        out.contains("UNSUPPORTED   push      no push command ran"),
        "{out}"
    );
}

#[test]
fn codex_masked_pipe_and_doc_edit() {
    let (code, out) = check("codex_lying.jsonl");
    assert_eq!(code, 1);
    assert!(
        out.contains("CONTRADICTED  test") && out.contains("output reports failures"),
        "{out}"
    );
    assert!(out.contains("SUPPORTED     commit"), "{out}");
}

#[test]
fn hook_blocks_once_then_warns() {
    let input = serde_json::json!({
        "hook_event_name": "Stop",
        "transcript_path": fixture("lying.jsonl"),
        "stop_hook_active": false,
        "last_assistant_message": "Done. Tests pass; committed the fix.",
    });
    let (code, stdout, stderr) = hook(input.clone());
    assert_eq!(code, 2, "{stderr}");
    assert!(stdout.is_empty());
    assert!(
        stderr.starts_with("receipt: 2 claims in your final message have no passing run"),
        "{stderr}"
    );
    assert!(stderr.contains("DO NOT:"), "{stderr}");

    let mut again = input;
    again["stop_hook_active"] = true.into();
    let (code, stdout, _) = hook(again);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(v["systemMessage"]
        .as_str()
        .unwrap()
        .contains("still unverified: test (CONTRADICTED), commit (UNSUPPORTED)"));
}

#[test]
fn hook_allows_supported_and_claimless_messages() {
    let (code, out, err) = hook(serde_json::json!({
        "transcript_path": fixture("honest.jsonl"),
        "last_assistant_message": "修好了，测试全部通过，已提交。",
    }));
    assert_eq!((code, out.as_str(), err.as_str()), (0, "", ""));

    // No claim: the transcript is not even opened.
    let (code, _, err) = hook(serde_json::json!({
        "transcript_path": "/nonexistent.jsonl",
        "last_assistant_message": "Here is the plan.",
    }));
    assert_eq!((code, err.as_str()), (0, ""));
}

#[test]
fn hook_errors_are_visible_but_never_block() {
    let (code, _, err) = hook(serde_json::json!({
        "transcript_path": "/nonexistent.jsonl",
        "last_assistant_message": "All tests pass.",
    }));
    assert_eq!(code, 1);
    assert!(
        err.starts_with("receipt: read transcript /nonexistent.jsonl"),
        "{err}"
    );
}

#[test]
fn subagent_uses_its_own_transcript() {
    let (code, _, err) = hook(serde_json::json!({
        "hook_event_name": "SubagentStop",
        "transcript_path": fixture("lying.jsonl"),
        "agent_transcript_path": fixture("honest.jsonl"),
        "last_assistant_message": "测试全部通过，已提交。",
    }));
    assert_eq!(code, 0, "{err}");
}
