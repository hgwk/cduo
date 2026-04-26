use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

pub fn has_session(name: &str) -> bool {
    Command::new("tmux")
        .arg("has-session")
        .arg("-t")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn create_session(name: &str, cwd: &Path, pane_a_cmd: &str, pane_b_cmd: &str) -> Result<()> {
    ensure_tmux_installed()?;

    if has_session(name) {
        bail!("tmux session '{name}' already exists");
    }

    let cwd_str = cwd.to_str().context("Invalid cwd path")?;

    let status = Command::new("tmux")
        .args([
            "new-session",
            "-d",
            "-s",
            name,
            "-c",
            cwd_str,
            "-x",
            "200",
            "-y",
            "50",
        ])
        .status()
        .context("Failed to create tmux session")?;

    if !status.success() {
        bail!("tmux new-session failed for '{name}'");
    }

    let status = Command::new("tmux")
        .args(["split-window", "-h", "-t", name])
        .status()
        .context("Failed to split tmux window")?;

    if !status.success() {
        bail!("tmux split-window failed for '{name}'");
    }

    let status = Command::new("tmux")
        .args(["select-layout", "-t", name, "even-horizontal"])
        .status()
        .context("Failed to set tmux layout")?;

    if !status.success() {
        bail!("tmux select-layout failed for '{name}'");
    }

    let status = Command::new("tmux")
        .args(["set-option", "-t", name, "remain-on-exit", "on"])
        .status()
        .context("Failed to set remain-on-exit")?;

    if !status.success() {
        bail!("tmux set-option remain-on-exit failed for '{name}'");
    }

    let status = Command::new("tmux")
        .args(["send-keys", "-t", &format!("{name}.0"), pane_a_cmd, "Enter"])
        .status()
        .context("Failed to send keys to pane A")?;

    if !status.success() {
        bail!("tmux send-keys failed for pane A in '{name}'");
    }

    let status = Command::new("tmux")
        .args(["send-keys", "-t", &format!("{name}.1"), pane_b_cmd, "Enter"])
        .status()
        .context("Failed to send keys to pane B")?;

    if !status.success() {
        bail!("tmux send-keys failed for pane B in '{name}'");
    }

    Ok(())
}

pub fn attach_session(name: &str) -> Result<()> {
    ensure_tmux_installed()?;

    if !has_session(name) {
        bail!("tmux session '{name}' does not exist");
    }

    let status = Command::new("tmux")
        .args(["attach-session", "-t", name])
        .status()
        .context("Failed to attach to tmux session")?;

    if !status.success() {
        bail!("tmux attach-session failed for '{name}'");
    }

    Ok(())
}

pub fn kill_session(name: &str) -> Result<()> {
    ensure_tmux_installed()?;

    if !has_session(name) {
        return Ok(());
    }

    let status = Command::new("tmux")
        .args(["kill-session", "-t", name])
        .status()
        .context("Failed to kill tmux session")?;

    if !status.success() {
        bail!("tmux kill-session failed for '{name}'");
    }

    Ok(())
}

fn ensure_tmux_installed() -> Result<()> {
    match Command::new("tmux").arg("-V").output() {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("tmux is not installed. Install tmux to use cduo workspaces (e.g., `brew install tmux` on macOS, `sudo apt install tmux` on Ubuntu)");
        }
        Err(e) => bail!("Failed to check tmux installation: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_session_nonexistent() {
        let result = has_session("cduo-nonexistent-session-xyz123");
        assert!(!result);
    }

    #[test]
    fn test_kill_session_nonexistent_is_ok() {
        let result = kill_session("cduo-nonexistent-session-xyz123");
        assert!(result.is_ok());
    }

    #[test]
    fn test_attach_session_nonexistent_fails() {
        let result = attach_session("cduo-nonexistent-session-xyz123");
        assert!(result.is_err());
    }

    #[test]
    fn test_create_session_fails_if_exists() {
        let result = create_session(
            "cduo-test-session-should-not-exist",
            &std::env::current_dir().unwrap(),
            "echo test",
            "echo test",
        );
        if result.is_ok() {
            let _ = kill_session("cduo-test-session-should-not-exist");
        }
    }
}
