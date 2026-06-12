use anyhow::{bail, Result};
use std::fs;
use std::path::{Path, PathBuf};

const ORCHESTRATION_START: &str = "<!-- CDUO_ORCHESTRATION_START -->";
const ORCHESTRATION_END: &str = "<!-- CDUO_ORCHESTRATION_END -->";
const LEGACY_ORCHESTRATION_REF: &str = "@.cduo/orchestration.md";

fn project_paths(cwd: &Path) -> ProjectPaths {
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

struct ProjectPaths {
    claude_dir: PathBuf,
    settings_target: PathBuf,
    claude_md_target: PathBuf,
    agents_md_target: PathBuf,
    orchestration_target: PathBuf,
    legacy_orchestration_target: PathBuf,
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

fn orchestration_ref() -> Result<String> {
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

fn remove_cduo_stop_hooks_from_settings(value: &mut serde_json::Value) -> bool {
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

fn ensure_instruction_reference(path: &Path, force: bool) -> Result<bool> {
    let orchestration_ref = orchestration_ref()?;
    if !path.exists() {
        fs::write(path, format!("{orchestration_ref}\n"))?;
        return Ok(true);
    }

    let existing = fs::read_to_string(path)?;
    if !force && has_instruction_reference(&existing) && !has_legacy_orchestration(&existing) {
        return Ok(false);
    }

    let (without_ref, _) = remove_reference_prelude(&existing);
    let (without_legacy, _) = remove_orchestration_block(&without_ref);
    let body = without_legacy.trim();
    let updated = if body.is_empty() {
        format!("{orchestration_ref}\n")
    } else {
        format!("{orchestration_ref}\n\n---\n\n{body}\n")
    };

    if updated == existing {
        return Ok(false);
    }

    fs::write(path, updated)?;
    Ok(true)
}

fn has_instruction_reference(content: &str) -> bool {
    reference_prelude_position(content).is_some()
}

fn has_legacy_orchestration(content: &str) -> bool {
    content.contains(ORCHESTRATION_START) && content.contains(ORCHESTRATION_END)
}

fn remove_orchestration_block(content: &str) -> (String, bool) {
    let Some(start) = content.find(ORCHESTRATION_START) else {
        return (content.to_string(), false);
    };
    let Some(end) = content[start..]
        .find(ORCHESTRATION_END)
        .map(|offset| start + offset + ORCHESTRATION_END.len())
    else {
        return (content.to_string(), false);
    };
    let before = &content[..start];
    let mut after = content[end..].to_string();
    if before.trim().is_empty() {
        after = strip_leading_cduo_separator(&after);
    }
    (format!("{before}{after}").trim().to_string(), true)
}

fn remove_reference_prelude(content: &str) -> (String, bool) {
    let lines = content.lines().collect::<Vec<_>>();
    let Some(pos) = reference_prelude_position(content) else {
        return (content.to_string(), false);
    };

    let mut remaining = lines
        .into_iter()
        .enumerate()
        .filter_map(|(idx, line)| (idx != pos).then_some(line))
        .collect::<Vec<_>>()
        .join("\n");
    remaining = strip_leading_cduo_separator(&remaining);
    (remaining.trim().to_string(), true)
}

fn reference_prelude_position(content: &str) -> Option<usize> {
    let refs = known_orchestration_refs();
    let lines = content.lines().collect::<Vec<_>>();
    lines
        .iter()
        .position(|line| refs.iter().any(|reference| line.trim() == reference))
        .filter(|pos| lines[..*pos].iter().all(|line| line.trim().is_empty()))
}

fn known_orchestration_refs() -> Vec<String> {
    let mut refs = vec![LEGACY_ORCHESTRATION_REF.to_string()];
    if let Ok(reference) = orchestration_ref() {
        refs.insert(0, reference);
    }
    refs
}

fn strip_leading_cduo_separator(content: &str) -> String {
    let trimmed = content.trim_start();
    if trimmed == "---" {
        return String::new();
    }
    if let Some(rest) = trimmed.strip_prefix("---\n") {
        return rest.trim_start().to_string();
    }
    trimmed.to_string()
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
    if paths.agents_md_target.exists() {
        files.push((&paths.agents_md_target, "AGENTS.md"));
    }
    if paths.legacy_orchestration_target.exists() {
        files.push((&paths.legacy_orchestration_target, "orchestration.md"));
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
    println!("cduo update — run `npm install -g @hgwk/cduo@latest`");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let paths = project_paths(&cwd);

    if uninstall_targets_exist(&paths)? {
        backup()?;
    } else {
        println!("✓ Nothing to uninstall");
        return Ok(());
    }

    let mut changed = false;

    if paths.settings_target.exists() {
        let content = fs::read_to_string(&paths.settings_target)?;
        let mut value: serde_json::Value = serde_json::from_str(&content)?;

        if remove_cduo_stop_hooks_from_settings(&mut value) {
            changed = true;
        }

        if !changed {
            println!("✓ No cduo Stop hook found in .claude/settings.local.json");
        } else if let Some(root) = value.as_object() {
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
    }

    if remove_instruction_reference(&paths.claude_md_target)? {
        println!("✓ Removed cduo reference from CLAUDE.md");
        changed = true;
    }

    if remove_instruction_reference(&paths.agents_md_target)? {
        println!("✓ Removed cduo reference from AGENTS.md");
        changed = true;
    }

    if paths.legacy_orchestration_target.exists() {
        fs::remove_file(&paths.legacy_orchestration_target)?;
        println!("✓ Removed .cduo/orchestration.md");
        changed = true;
    }

    if !changed {
        println!("✓ Nothing to uninstall");
    }

    Ok(())
}

fn uninstall_targets_exist(paths: &ProjectPaths) -> Result<bool> {
    if paths.legacy_orchestration_target.exists() {
        return Ok(true);
    }
    if instruction_removal_target_exists(&paths.claude_md_target)?
        || instruction_removal_target_exists(&paths.agents_md_target)?
    {
        return Ok(true);
    }
    if paths.settings_target.exists() {
        let content = fs::read_to_string(&paths.settings_target)?;
        let mut value: serde_json::Value = serde_json::from_str(&content)?;
        if remove_cduo_stop_hooks_from_settings(&mut value) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn instruction_removal_target_exists(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let content = fs::read_to_string(path)?;
    Ok(has_instruction_reference(&content) || has_legacy_orchestration(&content))
}

fn remove_instruction_reference(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }

    let content = fs::read_to_string(path)?;
    let (without_ref, ref_removed) = remove_reference_prelude(&content);
    let (without_block, legacy_removed) = remove_orchestration_block(&without_ref);
    let cleaned = without_block.trim().to_string();

    if !ref_removed && !legacy_removed {
        return Ok(false);
    }

    if cleaned.is_empty() {
        fs::remove_file(path)?;
    } else {
        fs::write(path, format!("{cleaned}\n"))?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

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
    fn test_ensure_stop_hook_force_overwrites_non_cduo_stop_hook() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        fs::write(
            &path,
            serde_json::json!({
                "hooks": {
                    "Stop": [{
                        "matcher": ".*",
                        "hooks": [{"type": "command", "command": "python3 custom.py"}]
                    }]
                }
            })
            .to_string(),
        )
        .unwrap();

        let changed = ensure_stop_hook(&path, true).unwrap();
        assert!(changed);
        let content: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(is_cduo_stop_hook(&content["hooks"]["Stop"]));
        assert!(!content.to_string().contains("custom.py"));
    }

    #[test]
    fn test_ensure_stop_hook_preserves_non_cduo_stop_hook_without_force() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        let custom = serde_json::json!({
            "hooks": {
                "Stop": [{
                    "matcher": ".*",
                    "hooks": [{"type": "command", "command": "python3 custom.py"}]
                }]
            }
        });
        fs::write(&path, custom.to_string()).unwrap();

        let changed = ensure_stop_hook(&path, false).unwrap();
        assert!(!changed);
        let content: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content, custom);
    }

    #[test]
    fn test_remove_cduo_stop_hooks_preserves_non_cduo_stop_hook() {
        let mut settings = serde_json::json!({
            "hooks": {
                "Stop": [
                    {
                        "matcher": ".*",
                        "hooks": [
                            {
                                "type": "command",
                                "command": "python3 custom_stop_hook.py"
                            }
                        ]
                    }
                ]
            }
        });

        assert!(!remove_cduo_stop_hooks_from_settings(&mut settings));
        assert_eq!(
            settings["hooks"]["Stop"][0]["hooks"][0]["command"],
            "python3 custom_stop_hook.py"
        );
    }

    #[test]
    fn test_remove_cduo_stop_hooks_preserves_mixed_non_cduo_entries() {
        let template = template_settings().unwrap();
        let cduo_entry = template["hooks"]["Stop"][0].clone();
        let custom_entry = serde_json::json!({
            "matcher": ".*",
            "hooks": [
                {
                    "type": "command",
                    "command": "python3 custom_stop_hook.py"
                }
            ]
        });
        let mut settings = serde_json::json!({
            "permissions": { "defaultMode": "accept" },
            "hooks": {
                "Stop": [cduo_entry, custom_entry.clone()],
                "PreToolUse": [{ "matcher": ".*", "hooks": [] }]
            }
        });

        assert!(remove_cduo_stop_hooks_from_settings(&mut settings));
        assert_eq!(settings["hooks"]["Stop"], serde_json::json!([custom_entry]));
        assert!(settings["hooks"].get("PreToolUse").is_some());
        assert_eq!(settings["permissions"]["defaultMode"], "accept");
    }

    #[test]
    fn test_remove_cduo_stop_hooks_removes_empty_hooks_object() {
        let template = template_settings().unwrap();
        let mut settings = serde_json::json!({
            "hooks": {
                "Stop": template["hooks"]["Stop"].clone()
            }
        });

        assert!(remove_cduo_stop_hooks_from_settings(&mut settings));
        assert!(settings.get("hooks").is_none());
    }

    #[test]
    fn test_ensure_orchestration_file_creates_new() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".cduo").join("orchestration.md");

        let changed = ensure_orchestration_file(&path, false).unwrap();
        assert!(changed);
        assert!(path.exists());
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("cduo Collaboration Mode"));
    }

    #[test]
    fn test_ensure_instruction_reference_creates_new() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        let reference = orchestration_ref().unwrap();

        let changed = ensure_instruction_reference(&path, false).unwrap();
        assert!(changed);

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, format!("{reference}\n"));
    }

    #[test]
    fn test_ensure_instruction_reference_prepends_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        fs::write(&path, "# My Project\n\nExisting content.").unwrap();

        let changed = ensure_instruction_reference(&path, false).unwrap();
        assert!(changed);

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(&orchestration_ref().unwrap()));
        assert!(content.contains("My Project"));
    }

    #[test]
    fn test_ensure_instruction_reference_replaces_legacy_block() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        fs::write(
            &path,
            format!(
                "{}\nlegacy\n{}\n\n---\n\n# My Project\n",
                ORCHESTRATION_START, ORCHESTRATION_END
            ),
        )
        .unwrap();

        let changed = ensure_instruction_reference(&path, false).unwrap();
        assert!(changed);

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(&orchestration_ref().unwrap()));
        assert!(!content.contains("legacy"));
        assert!(content.contains("My Project"));
    }

    #[test]
    fn test_ensure_instruction_reference_force_preserves_existing_body() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        fs::write(
            &path,
            format!("{}\n\n---\n\n# Keep Me\n", orchestration_ref().unwrap()),
        )
        .unwrap();

        let changed = ensure_instruction_reference(&path, true).unwrap();
        assert!(!changed);

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(&orchestration_ref().unwrap()));
        assert!(content.contains("# Keep Me"));
    }

    #[test]
    fn test_ensure_instruction_reference_preserves_front_matter() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        fs::write(&path, "---\ntitle: Keep\n---\n# Body\n").unwrap();

        let changed = ensure_instruction_reference(&path, false).unwrap();
        assert!(changed);

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(&orchestration_ref().unwrap()));
        assert!(content.contains("---\ntitle: Keep\n---\n# Body"));
    }

    #[test]
    fn test_ensure_instruction_reference_preserves_body_reference_as_content() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        let legacy_ref = LEGACY_ORCHESTRATION_REF;
        fs::write(
            &path,
            format!("# Body\n\nDocument mentions {legacy_ref} inline.\n"),
        )
        .unwrap();

        let changed = ensure_instruction_reference(&path, false).unwrap();
        assert!(changed);

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(&orchestration_ref().unwrap()));
        assert!(content.contains(&format!("Document mentions {legacy_ref} inline.")));
    }

    #[test]
    fn test_remove_instruction_reference_preserves_front_matter_without_cduo_ref() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        fs::write(&path, "---\ntitle: Keep\n---\n# Body\n").unwrap();

        assert!(!remove_instruction_reference(&path).unwrap());

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "---\ntitle: Keep\n---\n# Body\n");
    }

    #[test]
    fn test_remove_instruction_reference_preserves_body_reference_without_prelude() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        let original = format!("# Body\n\nDocument mentions {LEGACY_ORCHESTRATION_REF} inline.\n");
        fs::write(&path, &original).unwrap();

        assert!(!remove_instruction_reference(&path).unwrap());

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, original);
    }

    #[test]
    fn test_ensure_instruction_reference_prepends_existing_agents_policy() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        fs::write(&path, "# Existing Policy\n").unwrap();

        assert!(ensure_instruction_reference(&path, false).unwrap());

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(&orchestration_ref().unwrap()));
        assert!(content.contains("# Existing Policy"));
    }

    #[test]
    fn test_ensure_instruction_reference_preserves_body_reference_in_agents() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        let original = format!("# Existing Policy\n\nMention {LEGACY_ORCHESTRATION_REF} only.\n");
        fs::write(&path, &original).unwrap();

        assert!(ensure_instruction_reference(&path, false).unwrap());

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(&orchestration_ref().unwrap()));
        assert!(content.contains(&format!("Mention {LEGACY_ORCHESTRATION_REF} only.")));
    }

    #[test]
    fn test_uninstall_removes_orchestration() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        fs::write(
            &path,
            format!("{}\n\n---\n\n# Existing\n", orchestration_ref().unwrap()),
        )
        .unwrap();

        assert!(remove_instruction_reference(&path).unwrap());

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "# Existing\n");
    }

    #[test]
    fn test_uninstall_removes_agents_reference_but_keeps_body() {
        let _guard = env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        fs::write(
            &path,
            format!(
                "{}\n\n---\n\n# Existing Policy\n",
                orchestration_ref().unwrap()
            ),
        )
        .unwrap();

        let previous_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        let result = uninstall();
        std::env::set_current_dir(previous_dir).unwrap();

        assert!(result.is_ok());
        assert_eq!(fs::read_to_string(&path).unwrap(), "# Existing Policy\n");
        assert!(tmp.path().join(".cduo").join("backups").exists());
    }
}
