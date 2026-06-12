use std::fs;

use anyhow::{bail, Result};

use crate::project::{project_paths, remove_cduo_stop_hooks_from_settings, ProjectPaths};
use crate::project_instructions::{
    instruction_removal_target_exists, remove_instruction_reference,
};

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
