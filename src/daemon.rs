use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use crate::cli::Agent;
use crate::hook;
use crate::message::Message;
use crate::message_bus::MessageBus;
use crate::pair_router::PairRouter;
use crate::pty::{PtyManager, PtySession};
use crate::session::{self, PaneMetadata, SessionMetadata};
use crate::tmux;
use crate::transcripts;

type SharedPty = std::sync::Arc<tokio::sync::Mutex<PtySession>>;

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

fn get_log_file(session_id: &str) -> PathBuf {
    session::get_session_dir(session_id).join("daemon.log")
}

fn log_event(path: &Path, message: impl AsRef<str>) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(
            file,
            "{} {}",
            chrono::Utc::now().to_rfc3339(),
            message.as_ref()
        );
    }
}

fn preview(value: &str) -> String {
    value
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .chars()
        .take(160)
        .collect()
}

async fn write_relay_message(pty: &SharedPty, content: &str) {
    let mut pty = pty.lock().await;

    let _ = pty.write(b"\x1b[200~");
    let _ = pty.write(content.as_bytes());
    let _ = pty.write(b"\x1b[201~");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = pty.write(b"\r");
}

fn codex_sessions_root() -> PathBuf {
    std::env::var("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".codex")
        })
        .join("sessions")
}

fn collect_jsonl_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

fn codex_session_meta(path: &Path) -> Option<(PathBuf, chrono::DateTime<chrono::Utc>)> {
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines().take(30) {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        if value.get("type").and_then(serde_json::Value::as_str) != Some("session_meta") {
            continue;
        }

        let payload = value.get("payload")?;
        let cwd = payload.get("cwd").and_then(serde_json::Value::as_str)?;
        let timestamp = payload
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())?
            .with_timezone(&chrono::Utc);
        return Some((PathBuf::from(cwd), timestamp));
    }

    None
}

fn normalize_prompt_text(value: &str) -> String {
    value.replace('\r', "").trim().to_string()
}

fn strip_ansi_escapes(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        let b = input[i];
        if b == 0x1b && i + 1 < input.len() {
            let next = input[i + 1];
            i += 2;
            match next {
                b'[' => {
                    while i < input.len() && (0x30..=0x3f).contains(&input[i]) {
                        i += 1;
                    }
                    while i < input.len() && (0x20..=0x2f).contains(&input[i]) {
                        i += 1;
                    }
                    if i < input.len() && (0x40..=0x7e).contains(&input[i]) {
                        i += 1;
                    }
                }
                b']' | b'P' | b'X' | b'^' | b'_' => {
                    while i < input.len() {
                        let c = input[i];
                        if c == 0x07 {
                            i += 1;
                            break;
                        }
                        if c == 0x1b && i + 1 < input.len() && input[i + 1] == b'\\' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                _ => {}
            }
        } else if b < 0x20 && b != b'\n' && b != b'\r' && b != b'\t' {
            i += 1;
        } else {
            out.push(b);
            i += 1;
        }
    }
    out
}

fn codex_transcript_contains_user_prompt(path: &Path, expected_prompt: &str) -> bool {
    let expected_prompt = normalize_prompt_text(expected_prompt);
    if expected_prompt.is_empty() {
        return false;
    }

    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };

    content
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .any(|entry| {
            codex_user_text_from_entry(&entry)
                .is_some_and(|text| text == expected_prompt || text.contains(&expected_prompt))
        })
}

fn codex_user_text_from_entry(entry: &serde_json::Value) -> Option<String> {
    if entry.get("type").and_then(serde_json::Value::as_str) != Some("response_item") {
        return None;
    }

    let payload = entry.get("payload")?;
    if payload.get("type").and_then(serde_json::Value::as_str) != Some("message")
        || payload.get("role").and_then(serde_json::Value::as_str) != Some("user")
    {
        return None;
    }

    let content = payload.get("content")?;
    let text = match content {
        serde_json::Value::String(text) => normalize_prompt_text(text),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter_map(|part| {
                if part.get("type").and_then(serde_json::Value::as_str) == Some("input_text") {
                    part.get("text")
                        .and_then(serde_json::Value::as_str)
                        .map(normalize_prompt_text)
                        .filter(|text| !text.is_empty())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };

    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn discover_recent_codex_transcript(
    cwd: &Path,
    started_at: chrono::DateTime<chrono::Utc>,
    excluded: &HashSet<PathBuf>,
    expected_prompt: &str,
) -> Option<PathBuf> {
    let mut files = Vec::new();
    collect_jsonl_files(&codex_sessions_root(), &mut files);

    files
        .into_iter()
        .filter(|path| !excluded.contains(path))
        .filter(|path| codex_transcript_contains_user_prompt(path, expected_prompt))
        .filter_map(|path| {
            let (session_cwd, session_started_at) = codex_session_meta(&path)?;
            if session_cwd != cwd || session_started_at < started_at {
                return None;
            }
            let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
            Some((modified, path))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

async fn read_codex_transcript_with_retry(
    path: &Path,
    previous_signature: Option<&String>,
) -> transcripts::TranscriptOutput {
    for _ in 0..100 {
        let output = transcripts::codex::read_last_assistant(path);
        if !output.output.is_empty() && output.signature.as_ref() != previous_signature {
            return output;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    transcripts::TranscriptOutput::empty()
}

fn count_claude_stop_hook_summaries(path: &Path) -> usize {
    let Ok(content) = std::fs::read_to_string(path) else {
        return 0;
    };
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|entry| {
            entry.get("subtype").and_then(serde_json::Value::as_str) == Some("stop_hook_summary")
        })
        .count()
}

async fn read_claude_transcript_with_retry(
    path: &Path,
    previous_signature: Option<&String>,
    previous_stop_count: usize,
) -> transcripts::TranscriptOutput {
    for _ in 0..100 {
        let current_count = count_claude_stop_hook_summaries(path);
        if current_count > previous_stop_count {
            let output = transcripts::claude::read_last_assistant(path);
            if !output.output.is_empty() && output.signature.as_ref() != previous_signature {
                return output;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    transcripts::TranscriptOutput::empty()
}

fn drop_seen_signature(
    pane_id: &str,
    output: transcripts::TranscriptOutput,
    last_signatures: &mut HashMap<String, String>,
) -> transcripts::TranscriptOutput {
    let Some(signature) = &output.signature else {
        return output;
    };

    if last_signatures.get(pane_id) == Some(signature) {
        transcripts::TranscriptOutput::empty()
    } else {
        last_signatures.insert(pane_id.to_string(), signature.clone());
        output
    }
}

fn ensure_codex_transcript(
    pane_id: &str,
    transcripts: &mut HashMap<String, PathBuf>,
    assigned_transcripts: &mut HashSet<PathBuf>,
    pending_prompts: &HashMap<String, String>,
    cwd: &Path,
    started_at: chrono::DateTime<chrono::Utc>,
    log_path: &Path,
) {
    if transcripts.contains_key(pane_id) {
        return;
    }

    let Some(expected_prompt) = pending_prompts.get(pane_id) else {
        return;
    };

    let Some(path) =
        discover_recent_codex_transcript(cwd, started_at, assigned_transcripts, expected_prompt)
    else {
        return;
    };

    log_event(
        log_path,
        format!(
            "codex_transcript source={pane_id} path={} prompt=\"{}\"",
            path.display(),
            preview(expected_prompt)
        ),
    );
    assigned_transcripts.insert(path.clone());
    transcripts.insert(pane_id.to_string(), path);
}

fn publish_transcript_output(
    bus: &mut MessageBus,
    router: &PairRouter,
    log_path: &Path,
    pane_id: &str,
    output: &transcripts::TranscriptOutput,
) {
    if output.output.is_empty() || output.output.len() <= 6 {
        return;
    }

    let agent_msg = Message::new_agent(pane_id, &output.output);
    let Some(relay_msg) = router.route(&agent_msg) else {
        return;
    };

    let target = relay_msg.target_node_id.clone();
    if bus.publish(relay_msg) {
        log_event(
            log_path,
            format!(
                "publish source={pane_id} target={target} len={} text=\"{}\"",
                output.output.len(),
                preview(&output.output)
            ),
        );
    } else {
        log_event(
            log_path,
            format!(
                "dedup source={pane_id} target={target} len={} text=\"{}\"",
                output.output.len(),
                preview(&output.output)
            ),
        );
    }
}

async fn deliver_pending_messages(
    log_path: &Path,
    rx_a: &mut tokio::sync::mpsc::Receiver<Message>,
    rx_b: &mut tokio::sync::mpsc::Receiver<Message>,
    pty_a: &SharedPty,
    pty_b: &SharedPty,
    pending_prompts: &mut HashMap<String, String>,
) {
    while let Ok(msg) = rx_a.try_recv() {
        log_event(
            log_path,
            format!(
                "deliver target=a len={} text=\"{}\"",
                msg.content.len(),
                preview(&msg.content)
            ),
        );
        pending_prompts.insert("a".to_string(), normalize_prompt_text(&msg.content));
        write_relay_message(pty_a, &msg.content).await;
    }
    while let Ok(msg) = rx_b.try_recv() {
        log_event(
            log_path,
            format!(
                "deliver target=b len={} text=\"{}\"",
                msg.content.len(),
                preview(&msg.content)
            ),
        );
        pending_prompts.insert("b".to_string(), normalize_prompt_text(&msg.content));
        write_relay_message(pty_b, &msg.content).await;
    }
}

pub async fn start(agent: Agent, yolo: bool, full_access: bool, new_session: bool) -> Result<()> {
    if yolo && full_access {
        bail!("Use either --yolo or --full-access, not both.");
    }

    let cwd = std::env::current_dir()?;
    let project_name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace");
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

    let short_id = format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis()
    );
    let session_id = format!("cduo-{short_id}");
    let session_name = format!("cduo-{project_name}-{agent_label}-{short_id}");

    cleanup_stale_sessions()?;

    let existing = matching_sessions(&cwd, agent_label, mode_label)?;
    if !new_session {
        match existing.len() {
            0 => {}
            1 => {
                let (_, meta) = &existing[0];
                println!("Resuming existing workspace: {}", meta.session_name);
                if is_interactive_terminal() {
                    return tmux::attach_session(&meta.session_name);
                }
                println!("Use `cduo resume {}` to reattach.", meta.session_name);
                return Ok(());
            }
            _ => {
                bail!(
                    "More than one workspace is already running for this project.\nStop the duplicates or run with --new to replace them, then try again."
                );
            }
        }
    } else {
        for (old_session_id, old_meta) in existing {
            println!("Stopping existing workspace: {}", old_meta.session_name);
            cleanup_session_artifacts(&old_session_id, true, true)?;
        }
    }

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
            m.insert(
                "a".to_string(),
                PaneMetadata {
                    pane_id: "a".to_string(),
                    attach_port: 0,
                },
            );
            m.insert(
                "b".to_string(),
                PaneMetadata {
                    pane_id: "b".to_string(),
                    attach_port: 0,
                },
            );
            m
        },
    };

    session::write_session_metadata(&session_id, &metadata)?;

    println!("Starting cduo workspace...");
    println!("Project: {project_name}");
    println!("Agent: {agent_label}");
    if let Some(mode) = mode_label {
        println!("Mode: {mode}");
    }
    println!("Session: {session_name}");

    if let Err(e) = spawn_daemon_process(&session_id, &cwd) {
        let _ = cleanup_session_artifacts(&session_id, false, false);
        return Err(e);
    }

    let pane_a_cmd = format!("cduo __attach-pane {session_id} a");
    let pane_b_cmd = format!("cduo __attach-pane {session_id} b");

    let created_tmux = match tmux::create_session(&session_name, &cwd, &pane_a_cmd, &pane_b_cmd) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("Warning: Failed to create tmux session: {e}");
            println!("Run `cduo resume {session_name}` to attach manually.");
            false
        }
    };

    if created_tmux && is_interactive_terminal() {
        return tmux::attach_session(&session_name);
    }

    if created_tmux {
        println!("\nWorkspace started in the background.");
    }

    println!("Use `cduo resume {session_name}` to reattach.");
    println!("Use `cduo stop {session_name}` to stop.");

    Ok(())
}

fn is_interactive_terminal() -> bool {
    unsafe { libc::isatty(libc::STDIN_FILENO) == 1 && libc::isatty(libc::STDOUT_FILENO) == 1 }
}

pub async fn stop(session: Option<String>) -> Result<()> {
    let sessions = session::list_sessions()?;

    let target = if let Some(name) = session {
        find_session_by_name(&sessions, &name)
    } else {
        find_single_session(&sessions)
    };

    let (session_id, _) =
        target.context("No active session found. Use `cduo status` to see available sessions.")?;

    let socket = get_socket_path(&session_id);
    if !socket.exists() {
        println!("Session {session_id} is not running (daemon not found). Cleaning up...");
        cleanup_session_artifacts(&session_id, true, true)?;
        return Ok(());
    }

    let mut stream = UnixStream::connect(&socket)
        .await
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

    let mut stopped = false;
    if let Some(line) = lines.next_line().await? {
        let resp: ControlResponse = serde_json::from_str(&line)?;
        if resp.ok {
            println!("Session {session_id} stopped.");
            stopped = true;
        } else {
            bail!("Failed to stop session: {}", resp.error.unwrap_or_default());
        }
    }

    if stopped {
        wait_for_daemon_exit(&session_id, Duration::from_secs(5));
        cleanup_session_artifacts(&session_id, true, true)?;
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

    let (session_id, metadata) =
        target.context("No active session found. Use `cduo status` to see available sessions.")?;

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

    let stream = UnixStream::connect(&socket).await.with_context(|| {
        format!("Failed to connect to attach socket for session {session_id} pane {pane_id}")
    })?;

    crossterm::terminal::enable_raw_mode().context("Failed to enable raw terminal mode")?;

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
            println!(
                "  {} [{}] — {}",
                meta.session_name,
                status,
                meta.cwd.display()
            );
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

fn find_session_by_name(
    sessions: &[(String, Option<SessionMetadata>)],
    name: &str,
) -> Option<(String, Option<SessionMetadata>)> {
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

fn find_single_session(
    sessions: &[(String, Option<SessionMetadata>)],
) -> Option<(String, Option<SessionMetadata>)> {
    let cwd = std::env::current_dir().ok()?;
    let same_cwd: Vec<_> = sessions
        .iter()
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

fn matching_sessions(
    cwd: &Path,
    agent: &str,
    mode: Option<&str>,
) -> Result<Vec<(String, SessionMetadata)>> {
    let mode = mode.map(str::to_string);
    let sessions = session::list_sessions()?;
    Ok(sessions
        .into_iter()
        .filter_map(|(session_id, metadata)| {
            let metadata = metadata?;
            if metadata.cwd == cwd && metadata.agent == agent && metadata.mode == mode {
                Some((session_id, metadata))
            } else {
                None
            }
        })
        .collect())
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

fn cleanup_stale_sessions() -> Result<()> {
    for (session_id, metadata) in session::list_sessions()? {
        if is_daemon_running(&session_id) {
            continue;
        }

        if let Some(meta) = metadata {
            let _ = tmux::kill_session(&meta.session_name);
        }

        cleanup_session_artifacts(&session_id, false, false)?;
    }

    Ok(())
}

fn terminate_daemon_pid(session_id: &str) {
    let pid_file = get_pid_file(session_id);
    let Ok(content) = std::fs::read_to_string(&pid_file) else {
        return;
    };
    let Ok(pid) = content.trim().parse::<i32>() else {
        return;
    };

    if pid == std::process::id() as i32 {
        return;
    }

    unsafe {
        let _ = libc::kill(pid, libc::SIGTERM);
    }

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        unsafe {
            if libc::kill(pid, 0) != 0 {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    unsafe {
        let _ = libc::kill(pid, libc::SIGKILL);
    }
}

fn wait_for_daemon_exit(session_id: &str, timeout: Duration) {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if !is_daemon_running(session_id) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn cleanup_session_artifacts(
    session_id: &str,
    kill_tmux: bool,
    kill_daemon_pid: bool,
) -> Result<()> {
    let metadata = session::read_session_metadata(session_id).ok().flatten();

    if kill_tmux {
        if let Some(meta) = &metadata {
            let _ = tmux::kill_session(&meta.session_name);
        }
    }

    if kill_daemon_pid {
        terminate_daemon_pid(session_id);
    }

    let socket_path = get_socket_path(session_id);
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }

    for pane in ["a", "b"] {
        let attach_path = get_attach_socket_path(session_id, pane);
        if attach_path.exists() {
            let _ = std::fs::remove_file(&attach_path);
        }
    }

    let pid_file = get_pid_file(session_id);
    if pid_file.exists() {
        let _ = std::fs::remove_file(&pid_file);
    }

    session::remove_session(session_id)?;
    Ok(())
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
    let metadata =
        session::read_session_metadata(&session_id)?.context("Session metadata not found")?;
    let log_path = get_log_file(&session_id);

    let cwd = metadata.cwd.clone();
    let agent = metadata.agent.clone();
    let _tmux_session_name = metadata.session_name.clone();
    let is_codex = agent == "codex";
    let daemon_started_at = chrono::Utc::now();

    let pty_manager = PtyManager::new()?;

    let env_a = [
        ("TERMINAL_ID", "a"),
        ("ORCHESTRATION_PORT", &metadata.hook_port.to_string()),
    ];
    let env_b = [
        ("TERMINAL_ID", "b"),
        ("ORCHESTRATION_PORT", &metadata.hook_port.to_string()),
    ];

    let (cmd, args) = agent_command(&agent);

    let pty_a = std::sync::Arc::new(tokio::sync::Mutex::new(
        pty_manager.spawn(cmd, args, &cwd, &env_a, 120, 30)?,
    ));
    let pty_b = std::sync::Arc::new(tokio::sync::Mutex::new(
        pty_manager.spawn(cmd, args, &cwd, &env_b, 120, 30)?,
    ));
    let (broadcast_a_tx, broadcast_a_rx) = tokio::sync::broadcast::channel::<Vec<u8>>(64);
    let (broadcast_b_tx, broadcast_b_rx) = tokio::sync::broadcast::channel::<Vec<u8>>(64);

    println!("[daemon] Session {session_id} started.");
    println!("[daemon] Agent: {agent}");
    println!("[daemon] Hook port: {}", metadata.hook_port);
    log_event(
        &log_path,
        format!(
            "daemon_start session={session_id} agent={agent} hook_port={}",
            metadata.hook_port
        ),
    );

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
    let attach_listener_a = UnixListener::bind(&attach_socket_a).with_context(|| {
        format!(
            "Failed to bind attach socket A at {}",
            attach_socket_a.display()
        )
    })?;
    let attach_listener_b = UnixListener::bind(&attach_socket_b).with_context(|| {
        format!(
            "Failed to bind attach socket B at {}",
            attach_socket_b.display()
        )
    })?;

    let shutdown = tokio::sync::broadcast::channel(1).0;
    let mut shutdown_rx = shutdown.subscribe();

    let (relay_tx, mut relay_rx) = tokio::sync::mpsc::channel::<(String, String)>(128);
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<(String, String)>(128);
    let (hook_tx, mut hook_rx) = tokio::sync::mpsc::channel::<hook::HookEvent>(16);

    let hook_shutdown = shutdown.clone();
    tokio::spawn(async move {
        hook::run_hook_server(metadata.hook_port, hook_shutdown.subscribe(), hook_tx).await
    });

    let relay_shutdown = shutdown.clone();
    let pty_a_relay = pty_a.clone();
    let pty_b_relay = pty_b.clone();
    let strategy = if is_codex { "stream" } else { "hook" };
    let relay_log_path = log_path.clone();
    let codex_cwd = cwd.clone();
    let codex_started_at = daemon_started_at;
    tokio::spawn(async move {
        let mut bus = MessageBus::new();
        let router = PairRouter::new("a", "b");
        let mut rx_a = bus.subscribe("a");
        let mut rx_b = bus.subscribe("b");
        let mut codex_transcripts: HashMap<String, PathBuf> = HashMap::new();
        let mut codex_assigned_transcripts: HashSet<PathBuf> = HashSet::new();
        let mut codex_last_signatures: HashMap<String, String> = HashMap::new();
        let mut claude_last_signatures: HashMap<String, String> = HashMap::new();
        let mut claude_last_stop_counts: HashMap<String, usize> = HashMap::new();
        let mut codex_pending_prompts: HashMap<String, String> = HashMap::new();
        let mut relay_shutdown_rx = relay_shutdown.subscribe();

        loop {
            tokio::select! {
                Some((pane_id, prompt)) = input_rx.recv() => {
                    let prompt = normalize_prompt_text(&prompt);
                    if strategy == "stream" && !prompt.is_empty() {
                        log_event(
                            &relay_log_path,
                            format!("codex_input source={pane_id} text=\"{}\"", preview(&prompt)),
                        );
                        codex_pending_prompts.insert(pane_id.clone(), prompt);
                        codex_transcripts.remove(&pane_id);
                    }
                    deliver_pending_messages(
                        &relay_log_path,
                        &mut rx_a,
                        &mut rx_b,
                        &pty_a_relay,
                        &pty_b_relay,
                        &mut codex_pending_prompts,
                    ).await;
                }
                Some((pane_id, chunk)) = relay_rx.recv() => {
                    drop(chunk);

                    let output = if strategy == "stream" {
                        ensure_codex_transcript(
                            &pane_id,
                            &mut codex_transcripts,
                            &mut codex_assigned_transcripts,
                            &codex_pending_prompts,
                            &codex_cwd,
                            codex_started_at,
                            &relay_log_path,
                        );

                        if let Some(path) = codex_transcripts.get(&pane_id) {
                            let previous = codex_last_signatures.get(&pane_id).cloned();
                            let output = read_codex_transcript_with_retry(path, previous.as_ref()).await;
                            drop_seen_signature(&pane_id, output, &mut codex_last_signatures)
                        } else {
                            transcripts::TranscriptOutput::empty()
                        }
                    } else {
                        transcripts::TranscriptOutput::empty()
                    };

                    publish_transcript_output(
                        &mut bus,
                        &router,
                        &relay_log_path,
                        &pane_id,
                        &output,
                    );
                    deliver_pending_messages(
                        &relay_log_path,
                        &mut rx_a,
                        &mut rx_b,
                        &pty_a_relay,
                        &pty_b_relay,
                        &mut codex_pending_prompts,
                    ).await;
                }
                _ = tokio::time::sleep(Duration::from_millis(250)) => {
                    if strategy == "stream" {
                        for pane in ["a", "b"] {
                            let pane_id = pane.to_string();
                            ensure_codex_transcript(
                                &pane_id,
                                &mut codex_transcripts,
                                &mut codex_assigned_transcripts,
                                &codex_pending_prompts,
                                &codex_cwd,
                                codex_started_at,
                                &relay_log_path,
                            );

                            let Some(path) = codex_transcripts.get(&pane_id) else {
                                continue;
                            };
                            let output = drop_seen_signature(
                                &pane_id,
                                transcripts::codex::read_last_assistant(path),
                                &mut codex_last_signatures,
                            );
                            if output.output.is_empty() || output.output.len() <= 6 {
                                continue;
                            }
                            publish_transcript_output(
                                &mut bus,
                                &router,
                                &relay_log_path,
                                &pane_id,
                                &output,
                            );
                        }
                    }

                    deliver_pending_messages(
                        &relay_log_path,
                        &mut rx_a,
                        &mut rx_b,
                        &pty_a_relay,
                        &pty_b_relay,
                        &mut codex_pending_prompts,
                    ).await;
                }
                Some(event) = hook_rx.recv() => {
                    if strategy != "hook" {
                        continue;
                    }

                    let pane_id = event.terminal_id;
                    let output = if let Some(transcript_path) = event.transcript_path.as_deref() {
                        let previous = claude_last_signatures.get(&pane_id).cloned();
                        let previous_stop_count = claude_last_stop_counts
                            .get(&pane_id)
                            .copied()
                            .unwrap_or(0);
                        let transcript = Path::new(transcript_path);
                        let output = read_claude_transcript_with_retry(
                            transcript,
                            previous.as_ref(),
                            previous_stop_count,
                        )
                        .await;
                        let new_stop_count = count_claude_stop_hook_summaries(transcript);
                        if new_stop_count > previous_stop_count {
                            claude_last_stop_counts.insert(pane_id.clone(), new_stop_count);
                        }
                        drop_seen_signature(&pane_id, output, &mut claude_last_signatures)
                    } else {
                        transcripts::TranscriptOutput::empty()
                    };

                    log_event(
                        &relay_log_path,
                        format!(
                            "hook_event source={pane_id} transcript={} output_len={} text=\"{}\"",
                            event.transcript_path.as_deref().unwrap_or(""),
                            output.output.len(),
                            preview(&output.output)
                        ),
                    );

                    publish_transcript_output(
                        &mut bus,
                        &router,
                        &relay_log_path,
                        &pane_id,
                        &output,
                    );
                    deliver_pending_messages(
                        &relay_log_path,
                        &mut rx_a,
                        &mut rx_b,
                        &pty_a_relay,
                        &pty_b_relay,
                        &mut codex_pending_prompts,
                    ).await;
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
                tokio::time::timeout(Duration::from_millis(50), pty.read()).await
            };
            match chunk {
                Ok(Some(data)) => {
                    let text = String::from_utf8_lossy(&data);
                    if relay_tx_a
                        .send(("a".to_string(), text.to_string()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                    let _ = broadcast_tx_a.send(data);
                }
                Ok(None) => break,
                Err(_) => continue,
            }
        }
    });

    let broadcast_tx_b = broadcast_b_tx.clone();
    let pty_b_read = pty_b.clone();
    tokio::spawn(async move {
        loop {
            let chunk = {
                let mut pty = pty_b_read.lock().await;
                tokio::time::timeout(Duration::from_millis(50), pty.read()).await
            };
            match chunk {
                Ok(Some(data)) => {
                    let text = String::from_utf8_lossy(&data);
                    if relay_tx
                        .send(("b".to_string(), text.to_string()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                    let _ = broadcast_tx_b.send(data);
                }
                Ok(None) => break,
                Err(_) => continue,
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

    let control_shutdown = shutdown.clone();
    let pty_a_attach = pty_a.clone();
    let pty_b_attach = pty_b.clone();
    let handle_connections = async move {
        loop {
            tokio::select! {
                result = listener.accept() => {
                    let (stream, _) = match result {
                        Ok(r) => r,
                        Err(_) => continue,
                    };
                    let shutdown = control_shutdown.clone();
                    tokio::spawn(handle_control_stream(stream, shutdown));
                }
                result = attach_listener_a.accept() => {
                    let (stream, _) = match result {
                        Ok(r) => r,
                        Err(_) => continue,
                    };
                    tokio::spawn(handle_attach_client(
                        stream,
                        "a".to_string(),
                        pty_a_attach.clone(),
                        broadcast_a_rx.resubscribe(),
                        input_tx.clone(),
                    ));
                }
                result = attach_listener_b.accept() => {
                    let (stream, _) = match result {
                        Ok(r) => r,
                        Err(_) => continue,
                    };
                    tokio::spawn(handle_attach_client(
                        stream,
                        "b".to_string(),
                        pty_b_attach.clone(),
                        broadcast_b_rx.resubscribe(),
                        input_tx.clone(),
                    ));
                }
                _ = shutdown_rx.recv() => break,
            }
        }
    };

    handle_connections.await;

    {
        let mut pty = pty_a.lock().await;
        pty.close();
    }
    {
        let mut pty = pty_b.lock().await;
        pty.close();
    }

    cleanup_session_artifacts(&session_id, true, false)?;
    println!("[daemon] Session {session_id} shutdown complete.");

    Ok(())
}

fn agent_command(agent: &str) -> (&str, &[&str]) {
    match agent {
        "codex" => ("codex", &[]),
        _ => ("claude", &[]),
    }
}

async fn handle_control_stream(
    mut stream: UnixStream,
    shutdown: tokio::sync::broadcast::Sender<()>,
) {
    let (reader, mut writer) = stream.split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    match reader.read_line(&mut line).await {
        Ok(0) | Err(_) => return,
        Ok(_) => {}
    }

    let mut should_shutdown = false;
    let resp = match serde_json::from_str::<ControlRequest>(&line) {
        Ok(req) => match req.cmd.as_str() {
            "stop" => {
                should_shutdown = true;
                ControlResponse {
                    ok: true,
                    error: None,
                }
            }
            "attach" => ControlResponse {
                ok: true,
                error: None,
            },
            _ => ControlResponse {
                ok: false,
                error: Some(format!("Unknown command: {}", req.cmd)),
            },
        },
        Err(e) => ControlResponse {
            ok: false,
            error: Some(format!("Invalid request: {e}")),
        },
    };

    let json = serde_json::to_string(&resp).unwrap_or_default();
    let _ = writer.write_all(json.as_bytes()).await;
    let _ = writer.write_all(b"\n").await;

    if should_shutdown {
        let _ = shutdown.send(());
    }
}

async fn handle_attach_client(
    stream: UnixStream,
    pane_id: String,
    pty: std::sync::Arc<tokio::sync::Mutex<crate::pty::PtySession>>,
    mut broadcast_rx: tokio::sync::broadcast::Receiver<Vec<u8>>,
    input_tx: tokio::sync::mpsc::Sender<(String, String)>,
) {
    let (mut read_half, mut write_half) = stream.into_split();

    let pty_write = pty.clone();
    let client_to_pty = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        let mut pending_input = Vec::<u8>::new();
        loop {
            match read_half.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    pending_input.extend_from_slice(&buf[..n]);
                    while let Some(pos) = pending_input
                        .iter()
                        .position(|byte| *byte == b'\n' || *byte == b'\r')
                    {
                        let line = pending_input.drain(..=pos).collect::<Vec<_>>();
                        let stripped = strip_ansi_escapes(&line);
                        let text = normalize_prompt_text(&String::from_utf8_lossy(&stripped));
                        if !text.is_empty() {
                            let _ = input_tx.send((pane_id.clone(), text)).await;
                        }
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::PtyManager;
    use std::sync::Arc;
    use tokio::io::AsyncReadExt;
    use tokio::net::UnixStream;

    #[test]
    fn strip_ansi_escapes_removes_da_and_osc_responses() {
        let raw = b"\x1b[?1;2;4c\x1b]10;rgb:eded/ecec/eeee\x1b\\\x1b]11;rgb:1515/1414/1b1b\x1b\\\xed\x95\x98\xec\x9d\xb4\r";
        let stripped = strip_ansi_escapes(raw);
        let text = String::from_utf8_lossy(&stripped);
        let normalized = normalize_prompt_text(&text);
        assert_eq!(normalized, "하이");
    }

    #[test]
    fn strip_ansi_escapes_preserves_plain_text() {
        let stripped = strip_ansi_escapes(b"hello world\r");
        assert_eq!(String::from_utf8_lossy(&stripped), "hello world\r");
    }

    #[test]
    fn strip_ansi_escapes_handles_osc_with_bel_terminator() {
        let raw = b"\x1b]0;title text\x07payload";
        let stripped = strip_ansi_escapes(raw);
        assert_eq!(String::from_utf8_lossy(&stripped), "payload");
    }

    #[test]
    fn counts_stop_hook_summary_with_json_spacing() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"type": "system", "subtype": "stop_hook_summary"}}"#
        )
        .unwrap();
        writeln!(file, r#"{{"type":"system","subtype":"stop_hook_summary"}}"#).unwrap();

        assert_eq!(count_claude_stop_hook_summaries(file.path()), 2);
    }

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
            pty_mgr
                .spawn(
                    "sleep",
                    &["1"],
                    std::env::current_dir().unwrap().as_path(),
                    &[],
                    80,
                    24,
                )
                .unwrap(),
        ));

        let (tx, _rx) = tokio::sync::broadcast::channel::<Vec<u8>>(64);
        let rx = tx.subscribe();
        let (input_tx, _input_rx) = tokio::sync::mpsc::channel::<(String, String)>(16);

        let (mut client, server) = UnixStream::pair().unwrap();
        let handle = tokio::spawn(handle_attach_client(
            server,
            "a".to_string(),
            pty,
            rx,
            input_tx,
        ));

        let data = b"test broadcast data".to_vec();
        let _ = tx.send(data.clone());

        let mut buf = vec![0u8; 64];
        let n = tokio::time::timeout(Duration::from_secs(2), client.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buf[..n], &data[..]);

        drop(client);
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn test_attach_client_writes_to_pty() {
        let pty_mgr = PtyManager::new().unwrap();
        let pty = Arc::new(tokio::sync::Mutex::new(
            pty_mgr
                .spawn(
                    "cat",
                    &[],
                    std::env::current_dir().unwrap().as_path(),
                    &[],
                    80,
                    24,
                )
                .unwrap(),
        ));

        let (tx, _rx) = tokio::sync::broadcast::channel::<Vec<u8>>(64);
        let rx = tx.subscribe();
        let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<(String, String)>(16);

        let (mut client, server) = UnixStream::pair().unwrap();
        let handle = tokio::spawn(handle_attach_client(
            server,
            "a".to_string(),
            pty.clone(),
            rx,
            input_tx,
        ));

        client.write_all(b"relay-check\r\n").await.unwrap();
        let input = tokio::time::timeout(Duration::from_secs(2), input_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(input, ("a".to_string(), "relay-check".to_string()));

        let mut output = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline {
            let chunk = {
                let mut pty = pty.lock().await;
                tokio::time::timeout(Duration::from_millis(250), pty.read()).await
            };

            match chunk {
                Ok(Some(data)) => {
                    output.extend(data);
                    if output.windows(11).any(|w| w == b"relay-check") {
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => {}
            }
        }

        let output_str = String::from_utf8_lossy(&output);
        assert!(
            output_str.contains("relay-check"),
            "expected attach input to reach PTY, got: {:?}",
            output_str
        );

        drop(client);
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .unwrap()
            .unwrap();
    }
}
