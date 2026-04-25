use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use crate::cli::Agent;
use crate::extractors;
use crate::hook;
use crate::pty::PtyManager;
use crate::relay::{Message, RelayEngine};
use crate::session::{self, SessionMetadata, PaneMetadata};
use crate::tmux;

#[derive(Debug, Serialize, Deserialize)]
struct ControlRequest {
    cmd: String,
    session_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ControlResponse {
    ok: bool,
    error: Option<String>,
}

fn get_socket_path(session_id: &str) -> PathBuf {
    PathBuf::from(format!("/tmp/cduo-{session_id}.sock"))
}

fn get_attach_socket_path(session_id: &str, pane_id: &str) -> PathBuf {
    PathBuf::from(format!("/tmp/cduo-{session_id}-{pane_id}.sock"))
}

fn get_pid_file(session_id: &str) -> PathBuf {
    session::get_session_dir(session_id).join("daemon.pid")
}

pub async fn start(
    agent: Agent,
    yolo: bool,
    full_access: bool,
    new_session: bool,
) -> Result<()> {
    if yolo && full_access {
        bail!("Use either --yolo or --full-access, not both.");
    }

    let cwd = std::env::current_dir()?;
    let project_name = cwd.file_name().and_then(|n| n.to_str()).unwrap_or("workspace");
    let agent_label = match agent {
        Agent::Claude => "claude",
        Agent::Codex => "codex",
    };

    let mode_label = if yolo {
        Some("yolo")
    } else if full_access {
        Some("full-access")
    } else {
        None
    };

    let short_id = format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis());
    let session_id = format!("cduo-{short_id}");
    let session_name = format!("cduo-{project_name}-{agent_label}-{short_id}");

    let hook_port = find_available_port(53333).await?;

    let metadata = SessionMetadata {
        session_id: session_id.clone(),
        session_name: session_name.clone(),
        project_name: project_name.to_string(),
        display_name: format!("{} · {}", project_name, agent_label),
        cwd: cwd.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
        agent: agent_label.to_string(),
        mode: mode_label.map(|s| s.to_string()),
        hook_port,
        panes: {
            let mut m = HashMap::new();
            m.insert("a".to_string(), PaneMetadata { pane_id: "a".to_string(), attach_port: 0 });
            m.insert("b".to_string(), PaneMetadata { pane_id: "b".to_string(), attach_port: 0 });
            m
        },
    };

    session::write_session_metadata(&session_id, &metadata)?;

    if !new_session {
        let sessions = session::list_sessions()?;
        let same = sessions.iter().filter(|(_, m)| {
            if let Some(m) = m {
                m.cwd == cwd && m.agent == agent_label && m.mode == mode_label.map(|s| s.to_string())
            } else {
                false
            }
        }).collect::<Vec<_>>();

        if same.len() > 1 {
            bail!(
                "More than one workspace is already running for this project.\nUse --new to create another, or specify one explicitly."
            );
        }
    }

    println!("Starting cduo workspace...");
    println!("Project: {project_name}");
    println!("Agent: {agent_label}");
    if let Some(mode) = mode_label {
        println!("Mode: {mode}");
    }
    println!("Session: {session_name}");

    spawn_daemon_process(&session_id, &cwd)?;

    let pane_a_cmd = format!("cduo __attach-pane {session_id} a");
    let pane_b_cmd = format!("cduo __attach-pane {session_id} b");

    if let Err(e) = tmux::create_session(&session_name, &cwd, &pane_a_cmd, &pane_b_cmd) {
        eprintln!("Warning: Failed to create tmux session: {e}");
        println!("Run `cduo resume {session_name}` to attach manually.");
    }

    println!("\nUse `cduo resume {session_name}` to reattach.");
    println!("Use `cduo stop {session_name}` to stop.");

    Ok(())
}

pub async fn stop(session: Option<String>) -> Result<()> {
    let sessions = session::list_sessions()?;

    let target = if let Some(name) = session {
        find_session_by_name(&sessions, &name)
    } else {
        find_single_session(&sessions)
    };

    let (session_id, _) = target.context("No active session found. Use `cduo status` to see available sessions.")?;

    let socket = get_socket_path(&session_id);
    if !socket.exists() {
        println!("Session {session_id} is not running (daemon not found). Cleaning up...");
        session::remove_session(&session_id)?;
        return Ok(());
    }

    let mut stream = UnixStream::connect(&socket).await
        .with_context(|| format!("Failed to connect to daemon for session {session_id}"))?;

    let req = ControlRequest {
        cmd: "stop".to_string(),
        session_id: Some(session_id.clone()),
    };

    let json = serde_json::to_string(&req)?;
    stream.write_all(json.as_bytes()).await?;
    stream.write_all(b"\n").await?;
    stream.shutdown().await?;

    let reader = BufReader::new(stream);
    let mut lines = reader.lines();

    if let Some(line) = lines.next_line().await? {
        let resp: ControlResponse = serde_json::from_str(&line)?;
        if resp.ok {
            println!("Session {session_id} stopped.");
        } else {
            bail!("Failed to stop session: {}", resp.error.unwrap_or_default());
        }
    }

    Ok(())
}

pub async fn resume(session: Option<String>) -> Result<()> {
    let sessions = session::list_sessions()?;

    let target = if let Some(name) = session {
        find_session_by_name(&sessions, &name)
    } else {
        find_single_session(&sessions)
    };

    let (session_id, metadata) = target.context("No active session found. Use `cduo status` to see available sessions.")?;

    let session_name = if let Some(meta) = &metadata {
        meta.session_name.clone()
    } else {
        session_id.clone()
    };

    if let Some(meta) = metadata {
        println!("Resuming session: {} ({})", meta.session_name, session_id);
    } else {
        println!("Resuming session: {session_id}");
    }

    tmux::attach_session(&session_name)
}

pub async fn attach_pane(session_id: String, pane_id: String) -> Result<()> {
    let socket = get_attach_socket_path(&session_id, &pane_id);
    if !socket.exists() {
        bail!("Session {session_id} pane {pane_id} is not running (attach socket not found)");
    }

    let stream = UnixStream::connect(&socket).await
        .with_context(|| format!("Failed to connect to attach socket for session {session_id} pane {pane_id}"))?;

    crossterm::terminal::enable_raw_mode()
        .context("Failed to enable raw terminal mode")?;

    let restore_guard = TerminalRestoreGuard;

    let (mut read_half, mut write_half) = stream.into_split();
    let mut stdout = tokio::io::stdout();

    let stdin_to_socket = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            match tokio::io::stdin().read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if write_half.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let socket_to_stdout = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            match read_half.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if stdout.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                    let _ = stdout.flush().await;
                }
                Err(_) => break,
            }
        }
    });

    tokio::select! {
        _ = stdin_to_socket => {},
        _ = socket_to_stdout => {},
        _ = tokio::signal::ctrl_c() => {},
    }

    drop(restore_guard);
    Ok(())
}

struct TerminalRestoreGuard;

impl Drop for TerminalRestoreGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

pub async fn status(verbose: bool) -> Result<()> {
    let sessions = session::list_sessions()?;

    if sessions.is_empty() {
        println!("No active cduo sessions.");
        return Ok(());
    }

    println!("Active cduo sessions:");
    for (id, metadata) in sessions {
        if let Some(meta) = metadata {
            let running = is_daemon_running(&id);
            let status = if running { "running" } else { "stopped" };
            println!("  {} [{}] — {}", meta.session_name, status, meta.cwd.display());
            if verbose {
                println!("    Session ID: {id}");
                println!("    Agent: {}", meta.agent);
                println!("    Hook port: {}", meta.hook_port);
                println!("    Created: {}", meta.created_at);
            }
        } else {
            println!("  {id} [orphaned]");
        }
    }

    Ok(())
}

fn find_session_by_name(sessions: &[(String, Option<SessionMetadata>)], name: &str) -> Option<(String, Option<SessionMetadata>)> {
    let name_lower = name.to_lowercase();

    for (id, meta) in sessions {
        if let Some(m) = meta {
            if m.session_id.to_lowercase() == name_lower
                || m.session_name.to_lowercase() == name_lower
                || m.project_name.to_lowercase() == name_lower
                || id.to_lowercase().starts_with(&name_lower)
            {
                return Some((id.clone(), meta.clone()));
            }
        } else if id.to_lowercase().starts_with(&name_lower) {
            return Some((id.clone(), None));
        }
    }

    None
}

fn find_single_session(sessions: &[(String, Option<SessionMetadata>)]) -> Option<(String, Option<SessionMetadata>)> {
    let cwd = std::env::current_dir().ok()?;
    let same_cwd: Vec<_> = sessions.iter()
        .filter(|(_, m)| m.as_ref().map(|meta| meta.cwd == cwd).unwrap_or(false))
        .cloned()
        .collect();

    if same_cwd.len() == 1 {
        return Some(same_cwd[0].clone());
    }

    if sessions.len() == 1 {
        return Some(sessions[0].clone());
    }

    None
}

fn is_daemon_running(session_id: &str) -> bool {
    let pid_file = get_pid_file(session_id);
    if !pid_file.exists() {
        return false;
    }

    let Ok(content) = std::fs::read_to_string(&pid_file) else {
        return false;
    };

    let Ok(pid) = content.trim().parse::<i32>() else {
        return false;
    };

    unsafe { libc::kill(pid, 0) == 0 }
}

fn spawn_daemon_process(session_id: &str, cwd: &Path) -> Result<()> {
    let exe = std::env::current_exe()?;

    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("__daemon")
        .arg("--session")
        .arg(session_id)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    let child = cmd.spawn()?;
    let pid = child.id();

    let pid_file = get_pid_file(session_id);
    std::fs::create_dir_all(pid_file.parent().unwrap())?;
    std::fs::write(&pid_file, pid.to_string())?;

    Ok(())
}

async fn find_available_port(start: u16) -> Result<u16> {
    for port in start..start + 100 {
        match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => {
                drop(listener);
                return Ok(port);
            }
            Err(_) => continue,
        }
    }
    bail!("No available port found in range {start}-{})", start + 99)
}

pub async fn run_daemon(session_id: String) -> Result<()> {
    let metadata = session::read_session_metadata(&session_id)?
        .context("Session metadata not found")?;

    let cwd = metadata.cwd.clone();
    let agent = metadata.agent.clone();
    let is_codex = agent == "codex";

    let pty_manager = PtyManager::new()?;

    let env_a = [("TERMINAL_ID", "a"), ("ORCHESTRATION_PORT", &metadata.hook_port.to_string())];
    let env_b = [("TERMINAL_ID", "b"), ("ORCHESTRATION_PORT", &metadata.hook_port.to_string())];

    let (cmd, args) = agent_command(&agent);

    let pty_a = std::sync::Arc::new(tokio::sync::Mutex::new(
        pty_manager.spawn(cmd, args, &cwd, &env_a, 120, 30)?
    ));
    let pty_b = std::sync::Arc::new(tokio::sync::Mutex::new(
        pty_manager.spawn(cmd, args, &cwd, &env_b, 120, 30)?
    ));

    let (broadcast_a_tx, broadcast_a_rx) = tokio::sync::broadcast::channel::<Vec<u8>>(64);
    let (broadcast_b_tx, broadcast_b_rx) = tokio::sync::broadcast::channel::<Vec<u8>>(64);

    println!("[daemon] Session {session_id} started.");
    println!("[daemon] Agent: {agent}");
    println!("[daemon] Hook port: {}", metadata.hook_port);

    let socket_path = get_socket_path(&session_id);
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("Failed to bind control socket at {}", socket_path.display()))?;

    let attach_socket_a = get_attach_socket_path(&session_id, "a");
    let attach_socket_b = get_attach_socket_path(&session_id, "b");
    if attach_socket_a.exists() {
        let _ = std::fs::remove_file(&attach_socket_a);
    }
    if attach_socket_b.exists() {
        let _ = std::fs::remove_file(&attach_socket_b);
    }
    let attach_listener_a = UnixListener::bind(&attach_socket_a)
        .with_context(|| format!("Failed to bind attach socket A at {}", attach_socket_a.display()))?;
    let attach_listener_b = UnixListener::bind(&attach_socket_b)
        .with_context(|| format!("Failed to bind attach socket B at {}", attach_socket_b.display()))?;

    let shutdown = tokio::sync::broadcast::channel(1).0;
    let mut shutdown_rx = shutdown.subscribe();

    let (relay_tx, mut relay_rx) = tokio::sync::mpsc::channel::<(String, String)>(128);
    let (hook_tx, mut hook_rx) = tokio::sync::mpsc::channel::<hook::HookEvent>(16);

    let hook_shutdown = shutdown.clone();
    tokio::spawn(async move {
        hook::run_hook_server(metadata.hook_port, hook_shutdown.subscribe(), hook_tx).await
    });

    let relay_shutdown = shutdown.clone();
    let pty_a_relay = pty_a.clone();
    let pty_b_relay = pty_b.clone();
    let strategy = if is_codex { "stream" } else { "hook" };
    tokio::spawn(async move {
        let mut engine = RelayEngine::new();
        let mut buffers: HashMap<String, String> = HashMap::new();
        let mut relay_shutdown_rx = relay_shutdown.subscribe();

        loop {
            tokio::select! {
                Some((pane_id, chunk)) = relay_rx.recv() => {
                    let buffer = buffers.entry(pane_id.clone()).or_default();
                    buffer.push_str(&chunk);

                    if buffer.len() > 500_000 {
                        let start = buffer.len() - 500_000;
                        *buffer = buffer[start..].to_string();
                    }

                    let extracted = if strategy == "codex" || strategy == "stream" {
                        extractors::codex::extract(buffer)
                    } else {
                        extractors::claude::extract(buffer)
                    };

                    if !extracted.output.is_empty() && extracted.output.len() > 6 {
                        let target = if pane_id == "a" { "b" } else { "a" };
                        if engine.is_pane_ready(buffer) {
                            let msg = Message {
                                source: pane_id.clone(),
                                target: target.to_string(),
                                content: extracted.output,
                                signature: extracted.signature.unwrap_or_default(),
                                ready_at: Instant::now() + Duration::from_millis(2000),
                            };
                            engine.queue(target, msg);
                        }
                    }

                    if let Some(msg) = engine.process("a").unwrap() {
                        let data = msg.content.clone() + "\r";
                        let _ = pty_a_relay.lock().await.write(data.as_bytes());
                    }
                    if let Some(msg) = engine.process("b").unwrap() {
                        let data = msg.content.clone() + "\r";
                        let _ = pty_b_relay.lock().await.write(data.as_bytes());
                    }
                }
                Some(event) = hook_rx.recv() => {
                    let pane_id = event.terminal_id;
                    let buffer = buffers.get(&pane_id).cloned().unwrap_or_default();
                    let extracted = if strategy == "codex" || strategy == "stream" {
                        extractors::codex::extract(&buffer)
                    } else {
                        extractors::claude::extract(&buffer)
                    };

                    if !extracted.output.is_empty() && extracted.output.len() > 6 {
                        let target = if pane_id == "a" { "b" } else { "a" };
                        if engine.is_pane_ready(&buffer) {
                            let msg = Message {
                                source: pane_id.clone(),
                                target: target.to_string(),
                                content: extracted.output,
                                signature: extracted.signature.unwrap_or_default(),
                                ready_at: Instant::now() + Duration::from_millis(2000),
                            };
                            engine.queue(target, msg);
                        }
                    }

                    if let Some(msg) = engine.process("a").unwrap() {
                        let data = msg.content.clone() + "\r";
                        let _ = pty_a_relay.lock().await.write(data.as_bytes());
                    }
                    if let Some(msg) = engine.process("b").unwrap() {
                        let data = msg.content.clone() + "\r";
                        let _ = pty_b_relay.lock().await.write(data.as_bytes());
                    }
                }
                _ = relay_shutdown_rx.recv() => break,
            }
        }

        pty_a_relay.lock().await.close();
        pty_b_relay.lock().await.close();
    });

    let relay_tx_a = relay_tx.clone();
    let broadcast_tx_a = broadcast_a_tx.clone();
    let pty_a_read = pty_a.clone();
    tokio::spawn(async move {
        loop {
            let chunk = {
                let mut pty = pty_a_read.lock().await;
                pty.read().await
            };
            if let Some(data) = chunk {
                let text = String::from_utf8_lossy(&data);
                if relay_tx_a.send(("a".to_string(), text.to_string())).await.is_err() {
                    break;
                }
                let _ = broadcast_tx_a.send(data);
            } else {
                break;
            }
        }
    });

    let broadcast_tx_b = broadcast_b_tx.clone();
    let pty_b_read = pty_b.clone();
    tokio::spawn(async move {
        loop {
            let chunk = {
                let mut pty = pty_b_read.lock().await;
                pty.read().await
            };
            if let Some(data) = chunk {
                let text = String::from_utf8_lossy(&data);
                if relay_tx.send(("b".to_string(), text.to_string())).await.is_err() {
                    break;
                }
                let _ = broadcast_tx_b.send(data);
            } else {
                break;
            }
        }
    });

    let shutdown_clone = shutdown.clone();
    tokio::spawn(async move {
        let mut sigint = signal::unix::signal(signal::unix::SignalKind::interrupt()).unwrap();
        let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate()).unwrap();
        tokio::select! {
            _ = sigint.recv() => {},
            _ = sigterm.recv() => {},
        }
        let _ = shutdown_clone.send(());
    });

    let session_id_clone = session_id.clone();
    let handle_connections = async move {
        loop {
            tokio::select! {
                result = listener.accept() => {
                    let (stream, _) = match result {
                        Ok(r) => r,
                        Err(_) => continue,
                    };
                    let session_id = session_id_clone.clone();
                    tokio::spawn(handle_control_stream(stream, session_id));
                }
                result = attach_listener_a.accept() => {
                    let (stream, _) = match result {
                        Ok(r) => r,
                        Err(_) => continue,
                    };
                    tokio::spawn(handle_attach_client(stream, pty_a.clone(), broadcast_a_rx.resubscribe()));
                }
                result = attach_listener_b.accept() => {
                    let (stream, _) = match result {
                        Ok(r) => r,
                        Err(_) => continue,
                    };
                    tokio::spawn(handle_attach_client(stream, pty_b.clone(), broadcast_b_rx.resubscribe()));
                }
                _ = shutdown_rx.recv() => break,
            }
        }
    };

    handle_connections.await;

    cleanup_session(&session_id)?;
    println!("[daemon] Session {session_id} shutdown complete.");

    Ok(())
}

fn agent_command(agent: &str) -> (&str, &[&str]) {
    match agent {
        "codex" => ("codex", &[]),
        _ => ("claude", &[]),
    }
}

async fn handle_control_stream(mut stream: UnixStream, session_id: String) {
    let (reader, mut writer) = stream.split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    match reader.read_line(&mut line).await {
        Ok(0) | Err(_) => return,
        Ok(_) => {}
    }

    let resp = match serde_json::from_str::<ControlRequest>(&line) {
        Ok(req) => {
            match req.cmd.as_str() {
                "stop" => {
                    let _ = cleanup_session(&session_id);
                    ControlResponse { ok: true, error: None }
                }
                "attach" => {
                    ControlResponse { ok: true, error: None }
                }
                _ => {
                    ControlResponse { ok: false, error: Some(format!("Unknown command: {}", req.cmd)) }
                }
            }
        }
        Err(e) => ControlResponse { ok: false, error: Some(format!("Invalid request: {e}")) },
    };

    let json = serde_json::to_string(&resp).unwrap_or_default();
    let _ = writer.write_all(json.as_bytes()).await;
    let _ = writer.write_all(b"\n").await;
}

async fn handle_attach_client(
    stream: UnixStream,
    pty: std::sync::Arc<tokio::sync::Mutex<crate::pty::PtySession>>,
    mut broadcast_rx: tokio::sync::broadcast::Receiver<Vec<u8>>,
) {
    let (mut read_half, mut write_half) = stream.into_split();

    let pty_write = pty.clone();
    let client_to_pty = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            match read_half.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let _ = pty_write.lock().await.write(&buf[..n]);
                }
                Err(_) => break,
            }
        }
    });

    let broadcast_to_client = tokio::spawn(async move {
        while let Ok(data) = broadcast_rx.recv().await {
            if write_half.write_all(&data).await.is_err() {
                break;
            }
        }
    });

    tokio::select! {
        _ = client_to_pty => {},
        _ = broadcast_to_client => {},
    }
}

fn cleanup_session(session_id: &str) -> Result<()> {
    let socket_path = get_socket_path(session_id);
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)?;
    }

    for pane in ["a", "b"] {
        let attach_path = get_attach_socket_path(session_id, pane);
        if attach_path.exists() {
            let _ = std::fs::remove_file(&attach_path);
        }
    }

    let pid_file = get_pid_file(session_id);
    if pid_file.exists() {
        std::fs::remove_file(&pid_file)?;
    }

    session::remove_session(session_id)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::PtyManager;
    use std::sync::Arc;
    use tokio::io::AsyncReadExt;
    use tokio::net::UnixStream;

    #[test]
    fn test_control_request_serialization() {
        let req = ControlRequest {
            cmd: "stop".to_string(),
            session_id: Some("test-session".to_string()),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"cmd\":\"stop\""));

        let deserialized: ControlRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.cmd, "stop");
        assert_eq!(deserialized.session_id, Some("test-session".to_string()));
    }

    #[tokio::test]
    async fn test_attach_broadcast_to_client() {
        let pty_mgr = PtyManager::new().unwrap();
        let pty = Arc::new(tokio::sync::Mutex::new(
            pty_mgr.spawn("sleep", &["1"], std::env::current_dir().unwrap().as_path(), &[], 80, 24).unwrap()
        ));

        let (tx, _rx) = tokio::sync::broadcast::channel::<Vec<u8>>(64);
        let rx = tx.subscribe();

        let (mut client, server) = UnixStream::pair().unwrap();
        let handle = tokio::spawn(handle_attach_client(server, pty, rx));

        let data = b"test broadcast data".to_vec();
        let _ = tx.send(data.clone());

        let mut buf = vec![0u8; 64];
        let n = tokio::time::timeout(Duration::from_secs(2), client.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buf[..n], &data[..]);

        drop(client);
        tokio::time::timeout(Duration::from_secs(2), handle).await.unwrap().unwrap();
    }


}
