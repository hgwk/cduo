use anyhow::{bail, Result};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub fn doctor() -> Result<()> {
    let mut failed = false;
    let cwd = std::env::current_dir()?;

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

    println!(
        "{}",
        claude_startup_hooks_report(&claude_settings_candidates(&cwd))
    );

    if failed {
        bail!("Some checks failed. See above.");
    }

    println!("\n✅ cduo is ready.");
    Ok(())
}

pub fn doctor_paths() -> Result<()> {
    let cwd = std::env::current_dir()?;

    println!("cduo doctor paths");
    println!("project root: {}", cwd.display());
    println!("cduo home: {}", cduo_home_dir()?.display());
    println!("home-local guide: {}", home_orchestration_path()?.display());
    println!(
        "project Claude settings: {}",
        cwd.join(".claude").join("settings.local.json").display()
    );
    println!("AGENTS.md: {}", cwd.join("AGENTS.md").display());
    println!("CLAUDE.md: {}", cwd.join("CLAUDE.md").display());
    println!(
        "Claude CLI: {}",
        which("claude").unwrap_or_else(|| "not found".to_string())
    );
    println!(
        "Codex CLI: {}",
        which("codex").unwrap_or_else(|| "not found".to_string())
    );
    Ok(())
}

pub fn doctor_hooks() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let candidates = claude_settings_candidates(&cwd);

    println!("cduo doctor hooks");
    println!("{}", claude_startup_hooks_report(&candidates));
    for path in candidates {
        let Ok(content) = fs::read_to_string(&path) else {
            println!("- {}: missing", display_path(&path));
            continue;
        };
        let Ok(settings) = serde_json::from_str::<serde_json::Value>(&content) else {
            println!("- {}: invalid JSON", display_path(&path));
            continue;
        };
        println!(
            "- {}: SessionStart={}, Stop={}",
            display_path(&path),
            count_hook_commands(&settings, "SessionStart"),
            count_hook_commands(&settings, "Stop")
        );
    }
    Ok(())
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

fn claude_settings_candidates(cwd: &Path) -> Vec<PathBuf> {
    let mut paths = vec![cwd.join(".claude").join("settings.local.json")];
    if let Some(home) = std::env::var_os("HOME") {
        let home_claude = PathBuf::from(home).join(".claude");
        paths.push(home_claude.join("settings.json"));
        paths.push(home_claude.join("settings.local.json"));
    }
    paths
}

fn claude_startup_hooks_report(paths: &[PathBuf]) -> String {
    let mut found = Vec::new();
    let mut invalid = Vec::new();
    for path in paths {
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        let Ok(settings) = serde_json::from_str::<serde_json::Value>(&content) else {
            invalid.push(display_path(path));
            continue;
        };
        let count = count_hook_commands(&settings, "SessionStart");
        if count > 0 {
            found.push(format!("{} ({count})", display_path(path)));
        }
    }

    let count = found
        .iter()
        .filter_map(|entry| {
            entry
                .rsplit_once('(')
                .and_then(|(_, count)| count.trim_end_matches(')').parse::<usize>().ok())
        })
        .sum::<usize>();
    let hooks = if found.is_empty() {
        "! Claude startup hooks: none found in checked settings".to_string()
    } else {
        format!(
            "! Claude startup hooks: found {} command(s) in {}",
            count,
            found.join(", ")
        )
    };
    let hooks = if count > 1 {
        format!(
            "{hooks}\n  hint: multiple Claude SessionStart hooks can run duplicate startup work; review the listed settings file(s) and keep only intentional hooks. cduo relay uses the project Stop hook installed by `cduo init`."
        )
    } else {
        hooks
    };
    if invalid.is_empty() {
        hooks
    } else {
        format!("{hooks}; invalid JSON in {}", invalid.join(", "))
    }
}

fn count_hook_commands(settings: &serde_json::Value, hook_name: &str) -> usize {
    settings
        .get("hooks")
        .and_then(|hooks| hooks.get(hook_name))
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("hooks").and_then(serde_json::Value::as_array))
                .map(|hooks| {
                    hooks
                        .iter()
                        .filter(|hook| {
                            hook.get("type").and_then(serde_json::Value::as_str) == Some("command")
                                && hook
                                    .get("command")
                                    .and_then(serde_json::Value::as_str)
                                    .is_some_and(|command| !command.trim().is_empty())
                        })
                        .count()
                })
                .sum()
        })
        .unwrap_or(0)
}

fn display_path(path: &Path) -> String {
    let Ok(cwd) = std::env::current_dir() else {
        return path.display().to_string();
    };
    path.strip_prefix(&cwd)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
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
#[path = "project_doctor_tests.rs"]
mod tests;
