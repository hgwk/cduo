use anyhow::{bail, Result};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const ORCHESTRATION_START: &str = "<!-- CDUO_ORCHESTRATION_START -->";
const ORCHESTRATION_END: &str = "<!-- CDUO_ORCHESTRATION_END -->";
const ORCHESTRATION_REF: &str = "@.cduo/orchestration.md";

fn project_paths(cwd: &Path) -> ProjectPaths {
    ProjectPaths {
        claude_dir: cwd.join(".claude"),
        settings_target: cwd.join(".claude").join("settings.local.json"),
        claude_md_target: cwd.join("CLAUDE.md"),
        agents_md_target: cwd.join("AGENTS.md"),
        orchestration_target: cwd.join(".cduo").join("orchestration.md"),
        backup_root: cwd.join(".cduo").join("backups"),
    }
}

struct ProjectPaths {
    claude_dir: PathBuf,
    settings_target: PathBuf,
    claude_md_target: PathBuf,
    agents_md_target: PathBuf,
    orchestration_target: PathBuf,
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
    let orchestration_changed = ensure_orchestration_file(&paths.orchestration_target, force)?;
    let claude_md_changed = ensure_instruction_reference(&paths.claude_md_target, force)?;
    let agents_md_skipped = should_skip_existing_agents_reference(&paths.agents_md_target, force)?;
    let agents_md_changed = ensure_agents_reference(&paths.agents_md_target, force)?;

    if settings_changed {
        println!("✓ .claude/settings.local.json updated");
    } else {
        println!("✓ .claude/settings.local.json already has Stop hook");
    }

    if orchestration_changed {
        println!("✓ .cduo/orchestration.md updated");
    } else {
        println!("✓ .cduo/orchestration.md already up to date");
    }

    if claude_md_changed {
        println!("✓ CLAUDE.md references cduo orchestration");
    } else {
        println!("✓ CLAUDE.md already references cduo orchestration");
    }

    if agents_md_skipped {
        println!("✓ AGENTS.md left unchanged; use --force to add cduo reference");
    } else if agents_md_changed {
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

fn ensure_instruction_reference(path: &Path, force: bool) -> Result<bool> {
    if !path.exists() {
        fs::write(path, format!("{ORCHESTRATION_REF}\n"))?;
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
        format!("{ORCHESTRATION_REF}\n")
    } else {
        format!("{ORCHESTRATION_REF}\n\n---\n\n{body}\n")
    };

    if updated == existing {
        return Ok(false);
    }

    fs::write(path, updated)?;
    Ok(true)
}

fn ensure_agents_reference(path: &Path, force: bool) -> Result<bool> {
    if !path.exists() || force {
        return ensure_instruction_reference(path, force);
    }

    let existing = fs::read_to_string(path)?;
    if has_instruction_reference(&existing) || has_legacy_orchestration(&existing) {
        return ensure_instruction_reference(path, false);
    }

    Ok(false)
}

fn should_skip_existing_agents_reference(path: &Path, force: bool) -> Result<bool> {
    if force || !path.exists() {
        return Ok(false);
    }
    let existing = fs::read_to_string(path)?;
    Ok(!has_instruction_reference(&existing) && !has_legacy_orchestration(&existing))
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
    let lines = content.lines().collect::<Vec<_>>();
    lines
        .iter()
        .position(|line| line.trim() == ORCHESTRATION_REF)
        .filter(|pos| lines[..*pos].iter().all(|line| line.trim().is_empty()))
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
    if paths.agents_md_target.exists() {
        files.push((&paths.agents_md_target, "AGENTS.md"));
    }
    if paths.orchestration_target.exists() {
        files.push((&paths.orchestration_target, "orchestration.md"));
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

    if paths.orchestration_target.exists() {
        fs::remove_file(&paths.orchestration_target)?;
        println!("✓ Removed .cduo/orchestration.md");
        changed = true;
    }

    if !changed {
        println!("✓ Nothing to uninstall");
    }

    Ok(())
}

fn uninstall_targets_exist(paths: &ProjectPaths) -> Result<bool> {
    if paths.orchestration_target.exists() {
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

fn which(command: &str) -> Option<String> {
    if command.contains(std::path::MAIN_SEPARATOR) {
        return executable_path(Path::new(command)).map(|path| path.display().to_string());
    }

    std::env::var_os("PATH")?
        .to_string_lossy()
        .split(':')
        .filter(|segment| !segment.is_empty())
        .map(|segment| Path::new(segment).join(command))
        .find_map(|path| executable_path(&path).map(|path| path.display().to_string()))
}

fn executable_path(path: &Path) -> Option<&Path> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }

    #[cfg(unix)]
    {
        (metadata.permissions().mode() & 0o111 != 0).then_some(path)
    }

    #[cfg(not(unix))]
    {
        Some(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &Path) {}

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

        let changed = ensure_instruction_reference(&path, false).unwrap();
        assert!(changed);

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, format!("{ORCHESTRATION_REF}\n"));
    }

    #[test]
    fn test_ensure_instruction_reference_prepends_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        fs::write(&path, "# My Project\n\nExisting content.").unwrap();

        let changed = ensure_instruction_reference(&path, false).unwrap();
        assert!(changed);

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(ORCHESTRATION_REF));
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
        assert!(content.starts_with(ORCHESTRATION_REF));
        assert!(!content.contains("legacy"));
        assert!(content.contains("My Project"));
    }

    #[test]
    fn test_ensure_instruction_reference_force_preserves_existing_body() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        fs::write(&path, format!("{ORCHESTRATION_REF}\n\n---\n\n# Keep Me\n")).unwrap();

        let changed = ensure_instruction_reference(&path, true).unwrap();
        assert!(!changed);

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(ORCHESTRATION_REF));
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
        assert!(content.starts_with(ORCHESTRATION_REF));
        assert!(content.contains("---\ntitle: Keep\n---\n# Body"));
    }

    #[test]
    fn test_ensure_instruction_reference_preserves_body_reference_as_content() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        fs::write(
            &path,
            format!("# Body\n\nDocument mentions {ORCHESTRATION_REF} inline.\n"),
        )
        .unwrap();

        let changed = ensure_instruction_reference(&path, false).unwrap();
        assert!(changed);

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(ORCHESTRATION_REF));
        assert!(content.contains(&format!("Document mentions {ORCHESTRATION_REF} inline.")));
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
        let original = format!("# Body\n\nDocument mentions {ORCHESTRATION_REF} inline.\n");
        fs::write(&path, &original).unwrap();

        assert!(!remove_instruction_reference(&path).unwrap());

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, original);
    }

    #[test]
    fn test_ensure_agents_reference_skips_existing_project_policy_without_force() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        fs::write(&path, "# Existing Policy\n").unwrap();

        assert!(!ensure_agents_reference(&path, false).unwrap());

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "# Existing Policy\n");
    }

    #[test]
    fn test_ensure_agents_reference_skips_body_reference_without_force() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        let original = format!("# Existing Policy\n\nMention {ORCHESTRATION_REF} only.\n");
        fs::write(&path, &original).unwrap();

        assert!(!ensure_agents_reference(&path, false).unwrap());

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, original);
    }

    #[test]
    fn test_ensure_agents_reference_force_preserves_existing_project_policy() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        fs::write(&path, "# Existing Policy\n").unwrap();

        assert!(ensure_agents_reference(&path, true).unwrap());

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(ORCHESTRATION_REF));
        assert!(content.contains("# Existing Policy"));
    }

    #[test]
    fn which_finds_executable_on_path_without_shell() {
        let _guard = env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let command_path = tmp.path().join("fake-cduo-command");
        fs::write(&command_path, "#!/bin/sh\nexit 0\n").unwrap();
        make_executable(&command_path);
        let previous_path = std::env::var_os("PATH");
        std::env::set_var("PATH", tmp.path());

        assert_eq!(
            which("fake-cduo-command").as_deref(),
            Some(command_path.to_str().unwrap())
        );

        if let Some(path) = previous_path {
            std::env::set_var("PATH", path);
        } else {
            std::env::remove_var("PATH");
        }
    }

    #[test]
    fn which_ignores_non_executable_files() {
        let _guard = env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let command_path = tmp.path().join("fake-cduo-command");
        fs::write(&command_path, "not executable").unwrap();
        let previous_path = std::env::var_os("PATH");
        std::env::set_var("PATH", tmp.path());

        assert_eq!(which("fake-cduo-command"), None);

        if let Some(path) = previous_path {
            std::env::set_var("PATH", path);
        } else {
            std::env::remove_var("PATH");
        }
    }

    #[test]
    fn test_uninstall_removes_orchestration() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        fs::write(&path, format!("{ORCHESTRATION_REF}\n\n---\n\n# Existing\n")).unwrap();

        assert!(remove_instruction_reference(&path).unwrap());

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "# Existing\n");
    }

    #[test]
    fn test_uninstall_does_not_backup_unrelated_agents_policy() {
        let _guard = env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        fs::write(&path, "# Existing Policy\n").unwrap();

        let previous_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        let result = uninstall();
        std::env::set_current_dir(previous_dir).unwrap();

        assert!(result.is_ok());
        assert_eq!(fs::read_to_string(&path).unwrap(), "# Existing Policy\n");
        assert!(!tmp.path().join(".cduo").join("backups").exists());
    }
}
