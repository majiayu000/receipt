//! `receipt install`: add the Stop hook to Claude Code or Codex settings, keeping everything else.

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub fn run(args: &[String]) -> Result<ExitCode> {
    let (mut codex, mut dry_run, mut project) = (false, false, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--codex" => codex = true,
            "--dry-run" => dry_run = true,
            "--project" => {
                project = Some(PathBuf::from(
                    it.next().context("--project needs a directory")?,
                ))
            }
            other => bail!("install: unknown argument `{other}`"),
        }
    }
    let path = target(codex, project.as_deref())?;
    // Claude Code also checks subagents; Codex has no subagent transcript to check.
    let events: &[&str] = if codex {
        &["Stop"]
    } else {
        &["Stop", "SubagentStop"]
    };
    let exe = std::env::current_exe().context("locate the receipt binary")?;
    let command = format!("{} hook", shell_quote(&exe.to_string_lossy()));

    let existing = match std::fs::read_to_string(&path) {
        Ok(s) if !s.trim().is_empty() => Some(s),
        Ok(_) => None,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    let mut settings: Value = match &existing {
        Some(s) => serde_json::from_str(s)
            .with_context(|| format!("{} is not valid JSON", path.display()))?,
        None => json!({}),
    };
    let added = add_hooks(&mut settings, events, &command)
        .with_context(|| format!("update {}", path.display()))?;
    if added.is_empty() {
        println!("receipt is already installed in {}", path.display());
        return Ok(ExitCode::SUCCESS);
    }
    let out = serde_json::to_string_pretty(&settings)? + "\n";
    if dry_run {
        println!(
            "would add {} to {}:\n{out}",
            added.join(" and "),
            path.display()
        );
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    if let Some(old) = &existing {
        let backup = path.with_extension("json.receipt-backup");
        std::fs::write(&backup, old)
            .with_context(|| format!("write backup {}", backup.display()))?;
        println!("backup: {}", backup.display());
    }
    std::fs::write(&path, out).with_context(|| format!("write {}", path.display()))?;
    println!("added {} hook to {}", added.join(" and "), path.display());
    if codex {
        println!("Codex runs a new hook only after you trust it: open Codex and run /hooks.");
    }
    Ok(ExitCode::SUCCESS)
}

fn target(codex: bool, project: Option<&Path>) -> Result<PathBuf> {
    let home = || {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME is not set")
    };
    Ok(match (codex, project) {
        (false, Some(p)) => p.join(".claude/settings.json"),
        (true, Some(p)) => p.join(".codex/hooks.json"),
        (false, None) => match std::env::var_os("CLAUDE_CONFIG_DIR") {
            Some(d) => PathBuf::from(d).join("settings.json"),
            None => home()?.join(".claude/settings.json"),
        },
        (true, None) => match std::env::var_os("CODEX_HOME") {
            Some(d) => PathBuf::from(d).join("hooks.json"),
            None => home()?.join(".codex/hooks.json"),
        },
    })
}

/// Append a receipt group to each event that lacks one. Returns the events changed.
fn add_hooks(settings: &mut Value, events: &[&str], command: &str) -> Result<Vec<String>> {
    let root = settings
        .as_object_mut()
        .context("settings must be a JSON object")?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .context("`hooks` must be an object")?;
    let mut added = vec![];
    for event in events {
        let groups = hooks
            .entry(*event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .with_context(|| format!("`hooks.{event}` must be an array"))?;
        let present = groups.iter().any(|g| {
            g.get("hooks").and_then(Value::as_array).is_some_and(|hs| {
                hs.iter().any(|h| {
                    h.get("command")
                        .and_then(Value::as_str)
                        .is_some_and(|c| c.contains("receipt") && c.trim_end().ends_with(" hook"))
                })
            })
        });
        if !present {
            groups.push(
                json!({ "hooks": [{ "type": "command", "command": command, "timeout": 10 }] }),
            );
            added.push(event.to_string());
        }
    }
    Ok(added)
}

fn shell_quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}
