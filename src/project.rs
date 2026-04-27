use anyhow::{bail, Result};
use std::fs;
use std::path::{Path, PathBuf};

const ORCHESTRATION_START: &str = "<!-- CDUO_ORCHESTRATION_START -->";
const ORCHESTRATION_END: &str = "<!-- CDUO_ORCHESTRATION_END -->";

fn project_paths(cwd: &Path) -> ProjectPaths {
    ProjectPaths {
        claude_dir: cwd.join(".claude"),
        settings_target: cwd.join(".claude").join("settings.local.json"),
        claude_md_target: cwd.join("CLAUDE.md"),
        backup_root: cwd.join(".cduo").join("backups"),
    }
}

struct ProjectPaths {
    claude_dir: PathBuf,
    settings_target: PathBuf,
    claude_md_target: PathBuf,
    backup_root: PathBuf,
}

fn template_settings() -> Result<serde_json::Value> {
    let template = include_str!("../templates/claude-settings.json");
    Ok(serde_json::from_str(template)?)
}

fn template_orchestration() -> Result<String> {
    let tmpl = include_str!("../templates/orchestration.md");
    Ok(tmpl.to_string())
}

pub fn init(force: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let paths = project_paths(&cwd);

    fs::create_dir_all(&paths.claude_dir)?;

    let settings_changed = ensure_stop_hook(&paths.settings_target, force)?;
    let md_changed = ensure_claude_md(&paths.claude_md_target, force)?;

    if settings_changed {
        println!("✓ .claude/settings.local.json updated");
    } else {
        println!("✓ .claude/settings.local.json already has Stop hook");
    }

    if md_changed {
        println!("✓ CLAUDE.md updated");
    } else {
        println!("✓ CLAUDE.md already has orchestration content");
    }

    println!("\n✅ Initialization complete!");
    println!("Run: cduo claude");
    Ok(())
}

fn ensure_stop_hook(path: &Path, force: bool) -> Result<bool> {
    let template = template_settings()?;
    let template_stop = template.get("hooks").and_then(|h| h.get("Stop")).cloned();

    if !path.exists() || force {
        let mut value = template;
        if path.exists() && !force {
            let existing: serde_json::Value = serde_json::from_str(&fs::read_to_string(path)?)?;
            value = existing;
            if let Some(stop) = template_stop {
                let mut hooks = value.get("hooks").cloned().unwrap_or(serde_json::json!({}));
                if let Some(hooks_obj) = hooks.as_object_mut() {
                    hooks_obj.insert("Stop".to_string(), stop);
                    value["hooks"] = serde_json::Value::Object(hooks_obj.clone());
                } else {
                    let mut new_hooks = serde_json::Map::new();
                    new_hooks.insert("Stop".to_string(), stop);
                    value["hooks"] = serde_json::Value::Object(new_hooks);
                }
            }
        }
        fs::write(path, serde_json::to_string_pretty(&value)?)?;
        return Ok(true);
    }

    let existing: serde_json::Value = serde_json::from_str(&fs::read_to_string(path)?)?;
    if let Some(stop) = existing.get("hooks").and_then(|h| h.get("Stop")) {
        if !stop.is_null() && !stop.as_array().map(|a| a.is_empty()).unwrap_or(true) {
            if Some(stop) == template_stop.as_ref() {
                return Ok(false);
            }
            if !is_cduo_stop_hook(stop) {
                return Ok(false);
            }
        }
    }

    let mut value = existing;
    if let Some(stop) = template_stop {
        let mut hooks = value.get("hooks").cloned().unwrap_or(serde_json::json!({}));
        if let Some(hooks_obj) = hooks.as_object_mut() {
            hooks_obj.insert("Stop".to_string(), stop);
            value["hooks"] = serde_json::Value::Object(hooks_obj.clone());
        } else {
            let mut new_hooks = serde_json::Map::new();
            new_hooks.insert("Stop".to_string(), stop);
            value["hooks"] = serde_json::Value::Object(new_hooks);
        }
    }
    fs::write(path, serde_json::to_string_pretty(&value)?)?;
    Ok(true)
}

fn is_cduo_stop_hook(stop: &serde_json::Value) -> bool {
    let text = stop.to_string();
    text.contains("/hook") && text.contains("terminal_id")
}

fn ensure_claude_md(path: &Path, force: bool) -> Result<bool> {
    let orchestration = template_orchestration()?;

    if !path.exists() {
        fs::write(path, &orchestration)?;
        return Ok(true);
    }

    let existing = fs::read_to_string(path)?;

    if existing.contains(ORCHESTRATION_START) {
        if force {
            fs::write(path, &orchestration)?;
            return Ok(true);
        }
        return Ok(false);
    }

    let updated = format!("{orchestration}\n\n---\n\n{existing}");
    fs::write(path, updated)?;
    Ok(true)
}

pub fn doctor() -> Result<()> {
    let mut failed = false;

    println!("cduo doctor");

    let platform = std::env::consts::OS;
    let supported = platform == "macos" || platform == "linux";
    println!(
        "{} Platform: {platform} {}",
        if supported { "✓" } else { "✗" },
        if supported {
            "(supported)"
        } else {
            "(not supported)"
        }
    );
    if !supported {
        failed = true;
    }

    let claude = which("claude");
    println!(
        "{} Claude CLI: {}",
        if claude.is_some() { "✓" } else { "✗" },
        claude.as_deref().unwrap_or("not found")
    );
    if claude.is_none() {
        failed = true;
    }

    let codex = which("codex");
    println!(
        "{} Codex CLI: {}",
        if codex.is_some() { "✓" } else { "!" },
        codex.as_deref().unwrap_or("not found (optional)")
    );

    if failed {
        bail!("Some checks failed. See above.");
    }

    println!("\n✅ cduo is ready.");
    Ok(())
}

pub fn backup() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let paths = project_paths(&cwd);

    let mut files = Vec::new();
    if paths.settings_target.exists() {
        files.push((&paths.settings_target, "settings.local.json"));
    }
    if paths.claude_md_target.exists() {
        files.push((&paths.claude_md_target, "CLAUDE.md"));
    }

    if files.is_empty() {
        bail!("Nothing to backup in the current directory.");
    }

    let timestamp = chrono::Local::now().format("%Y-%m-%dT%H-%M-%S").to_string();
    let backup_dir = paths.backup_root.join(&timestamp);
    fs::create_dir_all(&backup_dir)?;

    for (src, name) in files {
        fs::copy(src, backup_dir.join(name))?;
    }

    println!("✓ Backup created at {}", backup_dir.display());
    Ok(())
}

pub fn update() -> Result<()> {
    println!("cduo update — run `cargo install cduo` or reinstall via npm");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let paths = project_paths(&cwd);

    backup()?;

    let mut changed = false;

    if paths.settings_target.exists() {
        let content = fs::read_to_string(&paths.settings_target)?;
        let mut value: serde_json::Value = serde_json::from_str(&content)?;

        if let Some(hooks) = value.get_mut("hooks") {
            if let Some(obj) = hooks.as_object_mut() {
                obj.remove("Stop");
                if obj.is_empty() {
                    if let Some(root) = value.as_object_mut() {
                        root.remove("hooks");
                    }
                }
            }
        }

        if let Some(root) = value.as_object() {
            if root.is_empty() {
                fs::remove_file(&paths.settings_target)?;
                println!("✓ Removed .claude/settings.local.json");
            } else {
                fs::write(
                    &paths.settings_target,
                    serde_json::to_string_pretty(&value)?,
                )?;
                println!("✓ Removed Stop hook from .claude/settings.local.json");
            }
        }
        changed = true;
    }

    if paths.claude_md_target.exists() {
        let content = fs::read_to_string(&paths.claude_md_target)?;
        let orchestration = template_orchestration()?;

        if content == orchestration {
            fs::remove_file(&paths.claude_md_target)?;
            println!("✓ Removed CLAUDE.md");
            changed = true;
        } else if content.starts_with(&format!("{orchestration}\n\n---\n\n")) {
            let remainder = &content[orchestration.len() + "\n\n---\n\n".len()..];
            fs::write(&paths.claude_md_target, remainder)?;
            println!("✓ Removed orchestration content from CLAUDE.md");
            changed = true;
        } else if let Some(start) = content.find(ORCHESTRATION_START) {
            if let Some(end) = content.find(ORCHESTRATION_END) {
                let before = &content[..start];
                let after = &content[end + ORCHESTRATION_END.len()..];
                let result = format!("{before}{after}").trim().to_string();
                if result.is_empty() {
                    fs::remove_file(&paths.claude_md_target)?;
                    println!("✓ Removed CLAUDE.md");
                } else {
                    fs::write(&paths.claude_md_target, result)?;
                    println!("✓ Removed orchestration content from CLAUDE.md");
                }
                changed = true;
            }
        }
    }

    if !changed {
        println!("✓ Nothing to uninstall");
    }

    Ok(())
}

fn which(command: &str) -> Option<String> {
    let output = std::process::Command::new("sh")
        .args(["-c", &format!("command -v {command}")])
        .output()
        .ok()?;

    if output.status.success() {
        String::from_utf8(output.stdout)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ensure_stop_hook_creates_new() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");

        let changed = ensure_stop_hook(&path, false).unwrap();
        assert!(changed);
        assert!(path.exists());

        let content: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(content.get("hooks").and_then(|h| h.get("Stop")).is_some());
    }

    #[test]
    fn test_ensure_stop_hook_merges_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        fs::write(&path, r#"{"permissions":{"defaultMode":"accept"}}"#).unwrap();

        let changed = ensure_stop_hook(&path, false).unwrap();
        assert!(changed);

        let content: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["permissions"]["defaultMode"], "accept");
        assert!(content["hooks"]["Stop"].is_array());
    }

    #[test]
    fn test_ensure_claude_md_creates_new() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");

        let changed = ensure_claude_md(&path, false).unwrap();
        assert!(changed);
        assert!(path.exists());
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("cduo Collaboration Mode"));
    }

    #[test]
    fn test_ensure_claude_md_prepends_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        fs::write(&path, "# My Project\n\nExisting content.").unwrap();

        let changed = ensure_claude_md(&path, false).unwrap();
        assert!(changed);

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("cduo Collaboration Mode"));
        assert!(content.contains("My Project"));
    }

    #[test]
    fn test_uninstall_removes_orchestration() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        let orch = template_orchestration().unwrap();
        fs::write(&path, &orch).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("cduo Collaboration Mode"));

        fs::remove_file(&path).unwrap();
        assert!(!path.exists());
    }
}
