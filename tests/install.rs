use std::process::Command;

fn install(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_receipt"))
        .arg("install")
        .args(args)
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap() + &String::from_utf8(out.stderr).unwrap();
    (out.status.code().unwrap(), text)
}

fn temp_project(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("receipt-install-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn claude_install_keeps_existing_settings_and_is_idempotent() {
    let dir = temp_project("claude");
    let settings = dir.join(".claude/settings.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    let before = r#"{"model": "opus", "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "other-tool"}]}]}, "env": {"A": "1"}}"#;
    std::fs::write(&settings, before).unwrap();

    let (code, out) = install(&["--project", dir.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("added Stop, SubagentStop, PostToolUse, PostToolUseFailure hooks"),
        "{out}"
    );
    let after = std::fs::read_to_string(&settings).unwrap();
    let v: serde_json::Value = serde_json::from_str(&after).unwrap();
    assert_eq!(v["model"], "opus");
    assert_eq!(v["env"]["A"], "1");
    assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "other-tool");
    assert!(v["hooks"]["Stop"][1]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .ends_with("receipt hook"));
    assert!(v["hooks"]["SubagentStop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .ends_with(" hook"));
    // Key order is the user's, not alphabetical.
    assert!(after.find("\"model\"").unwrap() < after.find("\"hooks\"").unwrap());
    assert!(after.find("\"hooks\"").unwrap() < after.find("\"env\"").unwrap());
    assert_eq!(
        std::fs::read_to_string(dir.join(".claude/settings.json.receipt-backup")).unwrap(),
        before
    );

    let (code, out) = install(&["--project", dir.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(out.contains("already installed"), "{out}");
    assert_eq!(std::fs::read_to_string(&settings).unwrap(), after);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn dry_run_writes_nothing_and_codex_gets_stop_only() {
    let dir = temp_project("codex");
    let (code, out) = install(&["--codex", "--project", dir.to_str().unwrap(), "--dry-run"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("would add Stop, PostToolUse to"), "{out}");
    assert!(!dir.join(".codex/hooks.json").exists());

    let (code, out) = install(&["--codex", "--project", dir.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("/hooks"), "{out}");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(".codex/hooks.json")).unwrap())
            .unwrap();
    assert!(v["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .ends_with(" hook"));
    assert!(v["hooks"].get("SubagentStop").is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn invalid_settings_are_reported_not_overwritten() {
    let dir = temp_project("bad");
    let settings = dir.join(".claude/settings.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    std::fs::write(&settings, "{ not json").unwrap();
    let (code, out) = install(&["--project", dir.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(out.contains("is not valid JSON"), "{out}");
    assert_eq!(std::fs::read_to_string(&settings).unwrap(), "{ not json");
    std::fs::remove_dir_all(&dir).unwrap();
}
