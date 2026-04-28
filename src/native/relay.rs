//! Native runtime relay loop.
//!
//! Reacts to Claude Stop hook events and a 250ms codex polling tick, extracts
//! the latest assistant text via `crate::relay_core`, deduplicates it, and
//! publishes through the in-process `MessageBus`. Because the UI thread owns
//! the pane PTY writers, relay output is sent as `(pane_id, bytes)` tuples on
//! a tokio mpsc channel; the UI loop drains the channel and writes to the
//! correct pane.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::{broadcast, mpsc};

use crate::hook::HookEvent;
use crate::message::Message;
use crate::message_bus::MessageBus;
use crate::pair_router::PairRouter;
use crate::relay_core::{
    count_claude_stop_hook_summaries, discover_recent_codex_transcript,
    discover_recent_codex_transcripts, drop_seen_signature, log_event, normalize_prompt_text,
    pane_uses_claude, pane_uses_codex, preview, publish_transcript_output,
    read_claude_transcript_with_retry, submit_delay_for_agent,
};
use crate::transcripts::{self, TranscriptOutput};

pub struct RelayInputs {
    pub cwd: PathBuf,
    pub started_at: DateTime<Utc>,
    pub log_path: PathBuf,
    pub pane_agents: HashMap<String, String>,
    pub hook_rx: mpsc::Receiver<HookEvent>,
    pub input_rx: mpsc::Receiver<(String, String)>,
    pub write_tx: mpsc::Sender<(String, Vec<u8>)>,
    pub shutdown_rx: broadcast::Receiver<()>,
}

pub async fn run(inputs: RelayInputs) {
    let RelayInputs {
        cwd,
        started_at,
        log_path,
        pane_agents,
        mut hook_rx,
        mut input_rx,
        write_tx,
        mut shutdown_rx,
    } = inputs;

    let mut bus = MessageBus::new();
    let router = PairRouter::new("a", "b");
    let mut rx_a = bus.subscribe("a");
    let mut rx_b = bus.subscribe("b");

    let mut codex_transcripts: HashMap<String, PathBuf> = HashMap::new();
    let mut codex_last_signatures: HashMap<String, String> = HashMap::new();
    let mut claude_last_signatures: HashMap<String, String> = HashMap::new();
    let mut claude_last_stop_counts: HashMap<String, usize> = HashMap::new();
    let mut codex_pending_prompts: HashMap<String, String> = HashMap::new();

    log_event(&log_path, "native_relay_start");

    loop {
        tokio::select! {
            Some((pane_id, prompt)) = input_rx.recv() => {
                let prompt = normalize_prompt_text(&prompt);
                if pane_uses_codex(&pane_agents, &pane_id) && !prompt.is_empty() {
                    log_event(
                        &log_path,
                        format!("codex_input source={pane_id} text=\"{}\"", preview(&prompt)),
                    );
                    codex_pending_prompts.insert(pane_id.clone(), prompt);
                }
                deliver_via_channel(
                    &log_path,
                    &mut rx_a,
                    &mut rx_b,
                    &write_tx,
                    &pane_agents,
                    &mut codex_pending_prompts,
                ).await;
            }
            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                for pane in ["a", "b"] {
                    if !pane_uses_codex(&pane_agents, pane) {
                        continue;
                    }
                    let pane_id = pane.to_string();
                    ensure_codex_transcript_local(
                        &pane_id,
                        &mut codex_transcripts,
                        &codex_pending_prompts,
                        &cwd,
                        started_at,
                        &log_path,
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
                    publish_transcript_output(&mut bus, &router, &log_path, &pane_id, &output);
                }
                deliver_via_channel(
                    &log_path,
                    &mut rx_a,
                    &mut rx_b,
                    &write_tx,
                    &pane_agents,
                    &mut codex_pending_prompts,
                ).await;
            }
            Some(event) = hook_rx.recv() => {
                let pane_id = event.terminal_id;
                if !pane_uses_claude(&pane_agents, &pane_id) {
                    continue;
                }

                let output = if let Some(path) = event.transcript_path.as_deref() {
                    let previous = claude_last_signatures.get(&pane_id).cloned();
                    let previous_stop_count = claude_last_stop_counts
                        .get(&pane_id)
                        .copied()
                        .unwrap_or(0);
                    let transcript = std::path::Path::new(path);
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
                    TranscriptOutput::empty()
                };

                log_event(
                    &log_path,
                    format!(
                        "hook_event source={pane_id} transcript={} output_len={} text=\"{}\"",
                        event.transcript_path.as_deref().unwrap_or(""),
                        output.output.len(),
                        preview(&output.output)
                    ),
                );

                publish_transcript_output(&mut bus, &router, &log_path, &pane_id, &output);
                deliver_via_channel(
                    &log_path,
                    &mut rx_a,
                    &mut rx_b,
                    &write_tx,
                    &pane_agents,
                    &mut codex_pending_prompts,
                ).await;
            }
            _ = shutdown_rx.recv() => break,
        }
    }
    log_event(&log_path, "native_relay_stop");
}

async fn deliver_via_channel(
    log_path: &std::path::Path,
    rx_a: &mut mpsc::Receiver<Message>,
    rx_b: &mut mpsc::Receiver<Message>,
    write_tx: &mpsc::Sender<(String, Vec<u8>)>,
    pane_agents: &HashMap<String, String>,
    pending_prompts: &mut HashMap<String, String>,
) {
    while let Ok(msg) = rx_a.try_recv() {
        log_deliver(log_path, "a", &msg.content);
        pending_prompts.insert("a".to_string(), normalize_prompt_text(&msg.content));
        let agent = pane_agents.get("a").map(String::as_str).unwrap_or("claude");
        send_relay_via_channel(write_tx, "a", &msg.content, agent).await;
    }
    while let Ok(msg) = rx_b.try_recv() {
        log_deliver(log_path, "b", &msg.content);
        pending_prompts.insert("b".to_string(), normalize_prompt_text(&msg.content));
        let agent = pane_agents.get("b").map(String::as_str).unwrap_or("claude");
        send_relay_via_channel(write_tx, "b", &msg.content, agent).await;
    }
}

fn log_deliver(log_path: &std::path::Path, target: &str, content: &str) {
    log_event(
        log_path,
        format!(
            "deliver target={target} len={} text=\"{}\"",
            content.len(),
            preview(content)
        ),
    );
}

async fn send_relay_via_channel(
    write_tx: &mpsc::Sender<(String, Vec<u8>)>,
    target: &str,
    content: &str,
    target_agent: &str,
) {
    let mut bundle = Vec::with_capacity(content.len() + 8);
    bundle.extend_from_slice(b"\x1b[200~");
    bundle.extend_from_slice(content.as_bytes());
    bundle.extend_from_slice(b"\x1b[201~");
    let _ = write_tx.send((target.to_string(), bundle)).await;

    let delay = submit_delay_for_agent(target_agent);
    tokio::time::sleep(Duration::from_millis(delay)).await;

    let _ = write_tx.send((target.to_string(), b"\r".to_vec())).await;
}

// Bind a codex rollout file to `pane_id` once a pending user prompt for that
// pane appears in any rollout under `~/.codex/sessions/`. Logs the binding so
// we can audit which rollout served which pane.
fn ensure_codex_transcript_local(
    pane_id: &str,
    transcripts: &mut HashMap<String, PathBuf>,
    pending_prompts: &HashMap<String, String>,
    cwd: &std::path::Path,
    started_at: DateTime<Utc>,
    log_path: &std::path::Path,
) {
    let Some(expected_prompt) = pending_prompts.get(pane_id) else {
        return;
    };

    if transcripts.get(pane_id).is_some_and(|path| {
        crate::relay_core::codex_transcript_contains_user_prompt(path, expected_prompt)
    }) {
        return;
    }

    let used_by_other_pane = transcripts
        .iter()
        .filter(|(source, _)| source.as_str() != pane_id)
        .map(|(_, path)| path.clone())
        .collect::<std::collections::HashSet<_>>();
    let excluded = std::collections::HashSet::new();
    let Some(path) = discover_recent_codex_transcript(cwd, started_at, &excluded, expected_prompt)
    else {
        let fallback = discover_recent_codex_transcripts(cwd, started_at)
            .into_iter()
            .rev()
            .find(|path| !used_by_other_pane.contains(path));
        if let Some(path) = fallback {
            log_event(
                log_path,
                format!(
                    "codex_transcript_fallback source={pane_id} path={} prompt=\"{}\"",
                    path.display(),
                    preview(expected_prompt)
                ),
            );
            transcripts.insert(pane_id.to_string(), path);
        }
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
    transcripts.insert(pane_id.to_string(), path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;
    use std::time::Duration;

    use tempfile::tempdir;
    use tokio::sync::Mutex;
    use tokio::time::timeout;

    /// `codex_sessions_root` reads `CODEX_HOME` env. Serialize tests that
    /// mutate that env var so parallel test threads do not interleave. We use
    /// a tokio Mutex so it can be held across `.await` points safely.
    fn codex_home_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn drain_writes(rx: &mut mpsc::Receiver<(String, Vec<u8>)>) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        while let Ok(item) = rx.try_recv() {
            out.push(item);
        }
        out
    }

    /// Wait until `rx` has produced at least one bracketed-paste body, or the
    /// deadline expires. Returns whatever was collected.
    async fn collect_writes(
        rx: &mut mpsc::Receiver<(String, Vec<u8>)>,
        within: Duration,
    ) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        let deadline = tokio::time::Instant::now() + within;
        while tokio::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match timeout(remaining, rx.recv()).await {
                Ok(Some(item)) => {
                    out.push(item);
                    if out.iter().any(|(_, bytes)| bytes == b"\r") {
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }
        // Soak up any straggling bytes that arrive immediately after.
        out.extend(drain_writes(rx));
        out
    }

    fn write_claude_transcript(path: &std::path::Path, assistant_text: &str) {
        let assistant_json = serde_json::to_string(assistant_text).unwrap();
        let assistant_line = format!(
            r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":{assistant_json}}}]}}}}"#,
        );
        let body = format!(
            "{user_line}\n{assistant_line}\n{stop_line}\n",
            user_line = r#"{"type":"user","message":{"role":"user","content":"hello"}}"#,
            stop_line = r#"{"type":"system","subtype":"stop_hook_summary"}"#,
        );
        std::fs::write(path, body).unwrap();
    }

    fn write_codex_rollout(
        path: &std::path::Path,
        cwd: &std::path::Path,
        timestamp: chrono::DateTime<chrono::Utc>,
        user_prompt: &str,
        assistant_text: &str,
    ) {
        let cwd_json = serde_json::to_string(&cwd.to_string_lossy()).unwrap();
        let ts = timestamp.to_rfc3339();
        let user_json = serde_json::to_string(user_prompt).unwrap();
        let assistant_json = serde_json::to_string(assistant_text).unwrap();
        let body = format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":{cwd_json},\"timestamp\":\"{ts}\"}}}}\n\
             {{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":{user_json}}}]}}}}\n\
             {{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"assistant\",\"phase\":\"final_answer\",\"content\":[{{\"type\":\"output_text\",\"text\":{assistant_json}}}]}}}}\n",
        );
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[tokio::test]
    async fn relay_publishes_claude_hook_payload_to_b() {
        let temp = tempdir().unwrap();
        let log_path = temp.path().join("relay.log");
        let transcript_path = temp.path().join("claude.jsonl");
        let answer = "RELAY_TEST_CLAUDE_TO_B";
        write_claude_transcript(&transcript_path, answer);

        let pane_agents = HashMap::from([
            ("a".to_string(), "claude".to_string()),
            ("b".to_string(), "claude".to_string()),
        ]);

        let (hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
        let (_input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
        let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(16);
        let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

        let inputs = RelayInputs {
            cwd: std::env::current_dir().unwrap(),
            started_at: chrono::Utc::now(),
            log_path,
            pane_agents,
            hook_rx,
            input_rx,
            write_tx,
            shutdown_rx: shutdown_tx.subscribe(),
        };

        let handle = tokio::spawn(run(inputs));

        hook_tx
            .send(HookEvent {
                terminal_id: "a".to_string(),
                transcript_path: Some(transcript_path.to_string_lossy().into_owned()),
            })
            .await
            .unwrap();

        let writes = collect_writes(&mut write_rx, Duration::from_secs(5)).await;
        let _ = shutdown_tx.send(());
        let _ = timeout(Duration::from_secs(2), handle).await;

        assert!(
            !writes.is_empty(),
            "expected relay to forward something, got nothing"
        );
        for (target, _) in &writes {
            assert_eq!(target, "b", "Claude pane A should relay only to pane B");
        }
        let body = writes
            .iter()
            .find_map(|(_, bytes)| {
                let s = String::from_utf8_lossy(bytes);
                s.contains("\x1b[200~").then_some(bytes.clone())
            })
            .expect("expected at least one bracketed-paste bundle");
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains(answer), "paste body missing answer: {body:?}");
        assert!(
            writes.iter().any(|(_, b)| b == b"\r"),
            "expected trailing Enter byte"
        );
    }

    #[tokio::test]
    async fn relay_publishes_codex_polling_to_a() {
        let _guard = codex_home_lock().lock().await;

        let temp = tempdir().unwrap();
        let codex_home = temp.path().join("codex");
        let cwd = temp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        let prev_codex_home = std::env::var_os("CODEX_HOME");
        std::env::set_var("CODEX_HOME", &codex_home);

        let started_at = chrono::Utc::now() - chrono::Duration::seconds(10);
        let session_ts = chrono::Utc::now() + chrono::Duration::seconds(1);
        let rollout = codex_home
            .join("sessions")
            .join("2026")
            .join("04")
            .join("27")
            .join("rollout-test.jsonl");
        let prompt = "RELAY_TEST_PROMPT";
        let answer = "RELAY_TEST_CODEX_TO_A";
        write_codex_rollout(&rollout, &cwd, session_ts, prompt, answer);

        let pane_agents = HashMap::from([
            ("a".to_string(), "claude".to_string()),
            ("b".to_string(), "codex".to_string()),
        ]);

        let (_hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
        let (input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
        let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(16);
        let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

        let inputs = RelayInputs {
            cwd: cwd.clone(),
            started_at,
            log_path: temp.path().join("relay.log"),
            pane_agents,
            hook_rx,
            input_rx,
            write_tx,
            shutdown_rx: shutdown_tx.subscribe(),
        };

        let handle = tokio::spawn(run(inputs));

        // Pretend the user typed the prompt into pane B; this primes the
        // pending-prompt match so the relay can bind the rollout file.
        input_tx
            .send(("b".to_string(), prompt.to_string()))
            .await
            .unwrap();

        let writes = collect_writes(&mut write_rx, Duration::from_secs(8)).await;
        let _ = shutdown_tx.send(());
        let _ = timeout(Duration::from_secs(2), handle).await;

        if let Some(prev) = prev_codex_home {
            std::env::set_var("CODEX_HOME", prev);
        } else {
            std::env::remove_var("CODEX_HOME");
        }

        assert!(
            !writes.is_empty(),
            "expected codex relay to forward something, got nothing"
        );
        for (target, _) in &writes {
            assert_eq!(
                target, "a",
                "Codex pane B should relay only to pane A, got target {target}"
            );
        }
        let body = writes
            .iter()
            .find_map(|(_, bytes)| {
                let s = String::from_utf8_lossy(bytes);
                s.contains("\x1b[200~").then_some(bytes.clone())
            })
            .expect("expected at least one bracketed-paste bundle");
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains(answer), "paste body missing answer: {body:?}");
        assert!(
            writes.iter().any(|(_, b)| b == b"\r"),
            "expected trailing Enter byte"
        );
    }

    #[tokio::test]
    async fn relay_publishes_codex_polling_from_a_to_claude_b() {
        let _guard = codex_home_lock().lock().await;

        let temp = tempdir().unwrap();
        let codex_home = temp.path().join("codex");
        let cwd = temp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        let prev_codex_home = std::env::var_os("CODEX_HOME");
        std::env::set_var("CODEX_HOME", &codex_home);

        let started_at = chrono::Utc::now() - chrono::Duration::seconds(10);
        let session_ts = chrono::Utc::now() + chrono::Duration::seconds(1);
        let rollout = codex_home
            .join("sessions")
            .join("2026")
            .join("04")
            .join("27")
            .join("rollout-codex-a.jsonl");
        let prompt = "RELAY_TEST_PROMPT_FROM_A";
        let answer = "RELAY_TEST_CODEX_A_TO_CLAUDE_B";
        write_codex_rollout(&rollout, &cwd, session_ts, prompt, answer);

        let pane_agents = HashMap::from([
            ("a".to_string(), "codex".to_string()),
            ("b".to_string(), "claude".to_string()),
        ]);

        let (_hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
        let (input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
        let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(16);
        let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

        let inputs = RelayInputs {
            cwd: cwd.clone(),
            started_at,
            log_path: temp.path().join("relay.log"),
            pane_agents,
            hook_rx,
            input_rx,
            write_tx,
            shutdown_rx: shutdown_tx.subscribe(),
        };

        let handle = tokio::spawn(run(inputs));

        input_tx
            .send(("a".to_string(), prompt.to_string()))
            .await
            .unwrap();

        let writes = collect_writes(&mut write_rx, Duration::from_secs(8)).await;
        let _ = shutdown_tx.send(());
        let _ = timeout(Duration::from_secs(2), handle).await;

        if let Some(prev) = prev_codex_home {
            std::env::set_var("CODEX_HOME", prev);
        } else {
            std::env::remove_var("CODEX_HOME");
        }

        assert!(
            !writes.is_empty(),
            "expected codex relay to forward something, got nothing"
        );
        for (target, _) in &writes {
            assert_eq!(
                target, "b",
                "Codex pane A should relay only to pane B, got target {target}"
            );
        }
        let body = writes
            .iter()
            .find_map(|(_, bytes)| {
                let s = String::from_utf8_lossy(bytes);
                s.contains("\x1b[200~").then_some(bytes.clone())
            })
            .expect("expected at least one bracketed-paste bundle");
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains(answer), "paste body missing answer: {body:?}");
        assert!(
            writes.iter().any(|(_, b)| b == b"\r"),
            "expected trailing Enter byte"
        );
    }

    #[tokio::test]
    async fn codex_manual_input_keeps_existing_transcript_binding() {
        let _guard = codex_home_lock().lock().await;

        let temp = tempdir().unwrap();
        let codex_home = temp.path().join("codex");
        let cwd = temp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        let prev_codex_home = std::env::var_os("CODEX_HOME");
        std::env::set_var("CODEX_HOME", &codex_home);

        let started_at = chrono::Utc::now() - chrono::Duration::seconds(10);
        let session_ts = chrono::Utc::now() + chrono::Duration::seconds(1);
        let rollout = codex_home
            .join("sessions")
            .join("2026")
            .join("04")
            .join("27")
            .join("rollout-manual.jsonl");
        let first_prompt = "FIRST_PROMPT";
        let first_answer = "FIRST_CODEX_TO_A";
        write_codex_rollout(&rollout, &cwd, session_ts, first_prompt, first_answer);

        let pane_agents = HashMap::from([
            ("a".to_string(), "claude".to_string()),
            ("b".to_string(), "codex".to_string()),
        ]);

        let (_hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
        let (input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
        let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(32);
        let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

        let inputs = RelayInputs {
            cwd: cwd.clone(),
            started_at,
            log_path: temp.path().join("relay.log"),
            pane_agents,
            hook_rx,
            input_rx,
            write_tx,
            shutdown_rx: shutdown_tx.subscribe(),
        };

        let handle = tokio::spawn(run(inputs));

        input_tx
            .send(("b".to_string(), first_prompt.to_string()))
            .await
            .unwrap();
        let first_writes = collect_writes(&mut write_rx, Duration::from_secs(8)).await;
        assert!(
            first_writes
                .iter()
                .any(|(_, bytes)| String::from_utf8_lossy(bytes).contains(first_answer)),
            "expected first codex answer to relay"
        );

        let second_prompt = "MANUAL_INTERVENTION_PROMPT";
        let second_answer = "SECOND_CODEX_TO_A";
        write_codex_rollout(&rollout, &cwd, session_ts, second_prompt, second_answer);
        input_tx
            .send(("b".to_string(), second_prompt.to_string()))
            .await
            .unwrap();
        let second_writes = collect_writes(&mut write_rx, Duration::from_secs(8)).await;

        let _ = shutdown_tx.send(());
        let _ = timeout(Duration::from_secs(2), handle).await;

        if let Some(prev) = prev_codex_home {
            std::env::set_var("CODEX_HOME", prev);
        } else {
            std::env::remove_var("CODEX_HOME");
        }

        assert!(
            second_writes.iter().any(|(target, bytes)| target == "a"
                && String::from_utf8_lossy(bytes).contains(second_answer)),
            "expected manual Codex input to keep the existing rollout binding and relay the next answer"
        );
    }

    #[tokio::test]
    async fn codex_rebinds_when_next_prompt_appears_in_new_rollout() {
        let _guard = codex_home_lock().lock().await;

        let temp = tempdir().unwrap();
        let codex_home = temp.path().join("codex");
        let cwd = temp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        let prev_codex_home = std::env::var_os("CODEX_HOME");
        std::env::set_var("CODEX_HOME", &codex_home);

        let started_at = chrono::Utc::now() - chrono::Duration::seconds(10);
        let first_ts = chrono::Utc::now() + chrono::Duration::seconds(1);
        let second_ts = chrono::Utc::now() + chrono::Duration::seconds(2);
        let first_rollout = codex_home
            .join("sessions")
            .join("2026")
            .join("04")
            .join("27")
            .join("rollout-first.jsonl");
        let second_rollout = codex_home
            .join("sessions")
            .join("2026")
            .join("04")
            .join("27")
            .join("rollout-second.jsonl");
        let first_prompt = "FIRST_ROLLOUT_PROMPT";
        let first_answer = "FIRST_ROLLOUT_ANSWER";
        write_codex_rollout(&first_rollout, &cwd, first_ts, first_prompt, first_answer);

        let pane_agents = HashMap::from([
            ("a".to_string(), "claude".to_string()),
            ("b".to_string(), "codex".to_string()),
        ]);

        let (_hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
        let (input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
        let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(32);
        let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

        let handle = tokio::spawn(run(RelayInputs {
            cwd: cwd.clone(),
            started_at,
            log_path: temp.path().join("relay.log"),
            pane_agents,
            hook_rx,
            input_rx,
            write_tx,
            shutdown_rx: shutdown_tx.subscribe(),
        }));

        input_tx
            .send(("b".to_string(), first_prompt.to_string()))
            .await
            .unwrap();
        let first_writes = collect_writes(&mut write_rx, Duration::from_secs(8)).await;
        assert!(
            first_writes
                .iter()
                .any(|(_, bytes)| String::from_utf8_lossy(bytes).contains(first_answer)),
            "expected first rollout answer to relay"
        );

        let second_prompt = "SECOND_ROLLOUT_PROMPT";
        let second_answer = "SECOND_ROLLOUT_ANSWER";
        write_codex_rollout(
            &second_rollout,
            &cwd,
            second_ts,
            second_prompt,
            second_answer,
        );
        input_tx
            .send(("b".to_string(), second_prompt.to_string()))
            .await
            .unwrap();
        let second_writes = collect_writes(&mut write_rx, Duration::from_secs(8)).await;

        let _ = shutdown_tx.send(());
        let _ = timeout(Duration::from_secs(2), handle).await;

        if let Some(prev) = prev_codex_home {
            std::env::set_var("CODEX_HOME", prev);
        } else {
            std::env::remove_var("CODEX_HOME");
        }

        assert!(
            second_writes.iter().any(|(target, bytes)| target == "a"
                && String::from_utf8_lossy(bytes).contains(second_answer)),
            "expected Codex pane to rebind to the rollout containing the latest prompt"
        );
    }
}
