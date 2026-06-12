use anyhow::{bail, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub use crate::project_files::{backup, uninstall, update};
use crate::project_instructions::*;

pub(crate) const ORCHESTRATION_START: &str = "<!-- CDUO_ORCHESTRATION_START -->";
pub(crate) const ORCHESTRATION_END: &str = "<!-- CDUO_ORCHESTRATION_END -->";
pub(crate) const LEGACY_ORCHESTRATION_REF: &str = "@.cduo/orchestration.md";

pub(crate) fn project_paths(cwd: &Path) -> ProjectPaths {
    ProjectPaths {
        claude_dir: cwd.join(".claude"),
        settings_target: cwd.join(".claude").join("settings.local.json"),
        claude_md_target: cwd.join("CLAUDE.md"),
        agents_md_target: cwd.join("AGENTS.md"),
        orchestration_target: home_orchestration_path()
            .unwrap_or_else(|_| cwd.join(".cduo").join("orchestration.md")),
        legacy_orchestration_target: cwd.join(".cduo").join("orchestration.md"),
        backup_root: cwd.join(".cduo").join("backups"),
    }
}

pub(crate) struct ProjectPaths {
    pub(crate) claude_dir: PathBuf,
    pub(crate) settings_target: PathBuf,
    pub(crate) claude_md_target: PathBuf,
    pub(crate) agents_md_target: PathBuf,
    pub(crate) orchestration_target: PathBuf,
    pub(crate) legacy_orchestration_target: PathBuf,
    pub(crate) backup_root: PathBuf,
}

fn template_settings() -> Result<serde_json::Value> {
    let template = include_str!("../templates/claude-settings.json");
    Ok(serde_json::from_str(template)?)
}

fn template_orchestration() -> Result<String> {
    let tmpl = include_str!("../templates/orchestration.md");
    Ok(tmpl.to_string())
}

fn cduo_home_dir() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("CDUO_HOME") {
        return Ok(PathBuf::from(home));
    }
    let Some(home) = std::env::var_os("HOME") else {
        bail!("HOME is not set; cannot locate cduo home directory");
    };
    Ok(PathBuf::from(home).join(".cduo"))
}

fn home_orchestration_path() -> Result<PathBuf> {
    Ok(cduo_home_dir()?.join("orchestration-guide.md"))
}

pub(crate) fn orchestration_ref() -> Result<String> {
    Ok(format!("@{}", home_orchestration_path()?.display()))
}

pub fn init(force: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let paths = project_paths(&cwd);

    fs::create_dir_all(&paths.claude_dir)?;

    let settings_changed = ensure_stop_hook(&paths.settings_target, force)?;
    let orchestration_changed = ensure_orchestration_file(&paths.orchestration_target, force)?;
    remove_legacy_orchestration_file(&paths)?;
    let claude_md_changed = ensure_instruction_reference(&paths.claude_md_target, force)?;
    let agents_md_changed = ensure_instruction_reference(&paths.agents_md_target, force)?;

    if settings_changed {
        println!("✓ .claude/settings.local.json updated");
    } else {
        println!("✓ .claude/settings.local.json already has Stop hook");
    }

    if orchestration_changed {
        println!("✓ ~/.cduo/orchestration-guide.md updated");
    } else {
        println!("✓ ~/.cduo/orchestration-guide.md already up to date");
    }

    if claude_md_changed {
        println!("✓ CLAUDE.md references cduo orchestration");
    } else {
        println!("✓ CLAUDE.md already references cduo orchestration");
    }

    if agents_md_changed {
        println!("✓ AGENTS.md references cduo orchestration");
    } else {
        println!("✓ AGENTS.md already references cduo orchestration");
    }

    println!("\n✅ Initialization complete!");
    println!("Run: cduo claude");
    Ok(())
}

fn ensure_stop_hook(path: &Path, force: bool) -> Result<bool> {
    let template = template_settings()?;
    let Some(template_stop) = template.get("hooks").and_then(|h| h.get("Stop")).cloned() else {
        return Ok(false);
    };

    if !path.exists() {
        fs::write(path, serde_json::to_string_pretty(&template)?)?;
        return Ok(true);
    }

    let existing: serde_json::Value = serde_json::from_str(&fs::read_to_string(path)?)?;
    if !force {
        if let Some(stop) = existing.get("hooks").and_then(|h| h.get("Stop")) {
            if stop == &template_stop || (!is_empty_stop_hook(stop) && !is_cduo_stop_hook(stop)) {
                return Ok(false);
            }
        }
    }

    let mut value = if force { template } else { existing };
    set_stop_hook(&mut value, template_stop);
    fs::write(path, serde_json::to_string_pretty(&value)?)?;
    Ok(true)
}

fn is_empty_stop_hook(stop: &serde_json::Value) -> bool {
    stop.is_null() || stop.as_array().is_some_and(|entries| entries.is_empty())
}

fn set_stop_hook(value: &mut serde_json::Value, stop: serde_json::Value) {
    if !value.is_object() {
        *value = serde_json::json!({});
    }

    let root = value.as_object_mut().expect("settings value is object");
    let hooks = root
        .entry("hooks".to_string())
        .or_insert_with(|| serde_json::json!({}));

    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }

    hooks
        .as_object_mut()
        .expect("hooks value is object")
        .insert("Stop".to_string(), stop);
}

fn is_cduo_stop_hook(stop: &serde_json::Value) -> bool {
    let text = stop.to_string();
    text.contains("/hook") && text.contains("terminal_id")
}

pub(crate) fn remove_cduo_stop_hooks_from_settings(value: &mut serde_json::Value) -> bool {
    let mut changed = false;

    if let Some(hooks) = value.get_mut("hooks") {
        if let Some(obj) = hooks.as_object_mut() {
            let remove_stop = if let Some(stop) = obj.get_mut("Stop") {
                if let Some(entries) = stop.as_array_mut() {
                    let before = entries.len();
                    entries.retain(|entry| !is_cduo_stop_hook(entry));
                    changed = entries.len() != before;
                    entries.is_empty()
                } else if is_cduo_stop_hook(stop) {
                    changed = true;
                    true
                } else {
                    false
                }
            } else {
                false
            };

            if remove_stop {
                obj.remove("Stop");
            }
        }
    }

    if value
        .get("hooks")
        .and_then(serde_json::Value::as_object)
        .is_some_and(serde_json::Map::is_empty)
    {
        if let Some(root) = value.as_object_mut() {
            root.remove("hooks");
        }
    }

    changed
}

fn ensure_orchestration_file(path: &Path, force: bool) -> Result<bool> {
    let orchestration = template_orchestration()?;

    if !path.exists() || force || fs::read_to_string(path)? != orchestration {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, orchestration)?;
        return Ok(true);
    }

    Ok(false)
}

fn remove_legacy_orchestration_file(paths: &ProjectPaths) -> Result<()> {
    if paths.legacy_orchestration_target == paths.orchestration_target {
        return Ok(());
    }
    if paths.legacy_orchestration_target.exists() {
        fs::remove_file(&paths.legacy_orchestration_target)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "project_tests.rs"]
mod tests;
