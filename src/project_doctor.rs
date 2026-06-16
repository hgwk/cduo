use anyhow::{bail, Result};
use std::fs;
use std::path::{Path, PathBuf};

#[path = "project_doctor_hooks.rs"]
mod project_doctor_hooks;
use project_doctor_hooks::{
    claude_settings_candidates, claude_startup_hooks_report, claude_stop_pair_id_report,
    count_hook_commands, display_path,
};

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
    println!(
        "{}",
        claude_stop_pair_id_report(&claude_settings_candidates(&cwd))
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
    println!("{}", claude_stop_pair_id_report(&candidates));
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
