use anyhow::{bail, Result};
use std::env;
use std::net::TcpListener;
use std::path::PathBuf;

pub fn doctor_runtime() -> Result<()> {
    let port = preferred_hook_port();
    let port_state = if port_available(port) {
        "available"
    } else {
        "busy"
    };

    println!("cduo doctor runtime");
    println!(
        "Claude CLI: {}",
        which("claude").unwrap_or_else(|| "not found".to_string())
    );
    println!(
        "Codex CLI: {}",
        which("codex").unwrap_or_else(|| "not found".to_string())
    );
    println!("cduo home: {}", cduo_home_dir()?.display());
    println!("hook port: {port} ({port_state})");
    println!(
        "CODEX_HOME sessions: {}",
        runtime_path_status(codex_sessions_dir()?)
    );
    println!(
        "CLAUDE_HOME projects: {}",
        runtime_path_status(claude_projects_dir()?)
    );
    for name in [
        "CDUO_HOME",
        "CDUO_PORT",
        "PORT",
        "CDUO_RELAY_PREFIX",
        "CDUO_MAX_RELAY_TURNS",
        "CDUO_STOP_TOKEN",
    ] {
        println!("{name}: {}", env_status(name));
    }
    Ok(())
}

fn cduo_home_dir() -> Result<PathBuf> {
    if let Some(home) = env::var_os("CDUO_HOME") {
        return Ok(PathBuf::from(home));
    }
    let Some(home) = env::var_os("HOME") else {
        bail!("HOME is not set; cannot locate cduo home directory");
    };
    Ok(PathBuf::from(home).join(".cduo"))
}

fn codex_sessions_dir() -> Result<PathBuf> {
    if let Some(home) = env::var_os("CODEX_HOME") {
        return Ok(PathBuf::from(home).join("sessions"));
    }
    let Some(home) = env::var_os("HOME") else {
        bail!("HOME is not set; cannot locate Codex sessions directory");
    };
    Ok(PathBuf::from(home).join(".codex").join("sessions"))
}

fn claude_projects_dir() -> Result<PathBuf> {
    if let Some(home) = env::var_os("CLAUDE_HOME") {
        return Ok(PathBuf::from(home).join("projects"));
    }
    let Some(home) = env::var_os("HOME") else {
        bail!("HOME is not set; cannot locate Claude projects directory");
    };
    Ok(PathBuf::from(home).join(".claude").join("projects"))
}

fn env_status(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| "unset".to_string())
}

fn preferred_hook_port() -> u16 {
    preferred_hook_port_from(|name| env::var(name).ok())
}

fn preferred_hook_port_from(mut get: impl FnMut(&str) -> Option<String>) -> u16 {
    for name in ["CDUO_PORT", "PORT"] {
        if let Some(value) = get(name).and_then(|value| value.parse::<u16>().ok()) {
            return value;
        }
    }
    53333
}

fn port_available(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

fn runtime_path_status(path: PathBuf) -> String {
    let state = if path.exists() { "exists" } else { "missing" };
    format!("{} ({state})", path.display())
}

fn which(command: &str) -> Option<String> {
    let path = env::var_os("PATH")?;
    for dir in env::split_paths(&path) {
        let candidate = dir.join(command);
        if candidate.is_file() {
            return Some(candidate.display().to_string());
        }
    }
    None
}

#[cfg(test)]
#[path = "project_doctor_runtime_tests.rs"]
mod tests;
