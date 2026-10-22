use receipt::{Checker, Config, Verdict};
use serde_json::json;

fn transcripts(command: &str, output: &str, exit_code: i32) -> Vec<String> {
    vec![
        format!(
            "{}\n{}",
            json!({"type": "assistant", "message": {"content": [{
                "type": "tool_use", "id": "run", "name": "Bash",
                "input": {"command": command}
            }]}}),
            json!({"type": "user", "message": {"content": [{
                "type": "tool_result", "tool_use_id": "run",
                "is_error": exit_code != 0,
                "content": if exit_code == 0 {
                    output.to_string()
                } else {
                    format!("Exit code {exit_code}\n{output}")
                }
            }]}}),
        ),
        json!({"type": "event_msg", "payload": {"type": "item_completed", "item": {
            "type": "CommandExecution", "command": command,
            "exit_code": exit_code, "status": "completed", "aggregated_output": output
        }}})
        .to_string(),
        json!({"type": "event_msg", "payload": {
            "type": "exec_command_end", "command": command,
            "exit_code": exit_code, "aggregated_output": output
        }})
        .to_string(),
    ]
}

fn assert_verdict(checker: &Checker, raw: &str, text: &str, expected: Verdict) {
    let claims = checker.claims(text);
    assert_eq!(claims.len(), 1, "{text}");
    let findings = checker.check_transcript(&claims, raw).unwrap();
    assert_eq!(findings[0].verdict, expected, "{text}: {findings:?}");
    assert_eq!(findings[0].blocks, expected != Verdict::Supported);
}

#[test]
fn clippy_warnings_only_contradict_lint() {
    let checker = Checker::new(&Config::default()).unwrap();
    for raw in transcripts("cargo clippy", "warning: demo (lib) generated 1 warning", 0) {
        assert_verdict(&checker, &raw, "Build succeeds.", Verdict::Supported);
        assert_verdict(&checker, &raw, "Type check passes.", Verdict::Supported);
        assert_verdict(&checker, &raw, "Clippy is clean.", Verdict::Contradicted);
    }
}

#[test]
fn masked_compiler_errors_still_contradict_all_matching_kinds() {
    let checker = Checker::new(&Config::default()).unwrap();
    for raw in transcripts(
        "cargo clippy 2>&1 | tail -3",
        "error[E0308]: mismatched types\nerror: could not compile `demo`",
        0,
    ) {
        for claim in ["Build succeeds.", "Type check passes.", "Clippy is clean."] {
            assert_verdict(&checker, &raw, claim, Verdict::Contradicted);
        }
    }
}

#[test]
fn test_output_failures_do_not_override_build_or_commit_markers() {
    let checker = Checker::new(&Config::default()).unwrap();
    for raw in transcripts(
        "cargo test 2>&1 | tail -3; git commit -am fix",
        "test result: FAILED. 4 passed; 1 failed; 0 ignored",
        0,
    ) {
        assert_verdict(&checker, &raw, "Tests pass.", Verdict::Contradicted);
        assert_verdict(&checker, &raw, "Build succeeds.", Verdict::Supported);
        assert_verdict(&checker, &raw, "Committed the fix.", Verdict::Supported);
    }
}

#[test]
fn nonzero_exit_still_contradicts_every_matching_kind() {
    let checker = Checker::new(&Config::default()).unwrap();
    for raw in transcripts("cargo clippy", "warning: demo (lib) generated 1 warning", 1) {
        for claim in ["Build succeeds.", "Type check passes.", "Clippy is clean."] {
            assert_verdict(&checker, &raw, claim, Verdict::Contradicted);
        }
    }
}

#[test]
fn configured_evidence_and_disabled_kinds_keep_failure_attribution() {
    let cfg: Config = toml::from_str(
        r#"disable = ["test"]
[evidence]
build = ["project-check"]
lint = ["project-check"]"#,
    )
    .unwrap();
    let checker = Checker::new(&cfg).unwrap();
    for raw in transcripts("project-check", "generated 1 warning", 0) {
        assert_verdict(&checker, &raw, "Build succeeds.", Verdict::Supported);
        assert_verdict(&checker, &raw, "Clippy is clean.", Verdict::Contradicted);
    }
}

#[test]
fn background_completion_preserves_kind_specific_output_failures() {
    let checker = Checker::new(&Config::default()).unwrap();
    let raw = transcripts(
        "cargo clippy",
        "Command running in background with ID: bg1.\nwarning: demo (lib) generated 1 warning",
        0,
    )
    .remove(0);
    assert_verdict(&checker, &raw, "Build succeeds.", Verdict::Unsupported);
    let completed = format!(
        "{}\n{}",
        raw,
        json!({"type": "user", "message": {"content":
            "<task-notification><tool-use-id>run</tool-use-id><status>completed</status><summary>Command completed (exit code 0)</summary></task-notification>"
        }}),
    );
    assert_verdict(&checker, &completed, "Build succeeds.", Verdict::Supported);
    assert_verdict(&checker, &completed, "Clippy is clean.", Verdict::Contradicted);
}

#[test]
fn later_clean_run_replaces_failed_output_evidence() {
    let checker = Checker::new(&Config::default()).unwrap();
    let failed = transcripts("cargo clippy", "generated 1 warning", 0);
    let clean = transcripts("cargo clippy", "Finished `dev` profile", 0);
    for (failed, clean) in failed.iter().zip(&clean) {
        let raw = format!("{failed}\n{clean}");
        assert_verdict(&checker, &raw, "Clippy is clean.", Verdict::Supported);
        assert_verdict(&checker, &raw, "Build succeeds.", Verdict::Supported);
    }
}
