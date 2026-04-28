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
    count_claude_stop_hook_summaries, discover_recent_claude_transcript,
    discover_recent_codex_transcript, discover_recent_codex_transcripts, drop_seen_signature,
    log_event, normalize_prompt_text, pane_uses_claude, pane_uses_codex, preview,
    read_claude_transcript_with_retry, submit_delay_for_agent,
};
use crate::transcripts::{self, TranscriptOutput};

#[derive(Debug, Clone)]
pub enum RelayControl {
    ManualRelay {
        pane_id: String,
    },
    SetRoute {
        source: String,
        target: String,
        enabled: bool,
    },
    SetPrefix(Option<String>),
}

pub struct RelayInputs {
    pub cwd: PathBuf,
    pub started_at: DateTime<Utc>,
    pub log_path: PathBuf,
    pub pane_agents: HashMap<String, String>,
    pub hook_rx: mpsc::Receiver<HookEvent>,
    pub control_rx: mpsc::Receiver<RelayControl>,
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
        mut control_rx,
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
    let mut claude_transcripts: HashMap<String, PathBuf> = HashMap::new();
    let mut codex_pending_prompts: HashMap<String, String> = HashMap::new();
    let mut controls = RelayControlState::default();

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
            Some(control) = control_rx.recv() => {
                match control {
                    RelayControl::ManualRelay { pane_id } => {
                        manual_relay(
                            &pane_id,
                            ManualRelayContext {
                                router: &router,
                                controls: &controls,
                                pane_agents: &pane_agents,
                                codex_transcripts: &codex_transcripts,
                                claude_transcripts: &claude_transcripts,
                                pending_prompts: &mut codex_pending_prompts,
                                write_tx: &write_tx,
                                log_path: &log_path,
                            },
                        )
                        .await;
                    }
                    RelayControl::SetRoute { source, target, enabled } => {
                        if controls.set_route_enabled(&source, &target, enabled) {
                            log_event(
                                &log_path,
                                format!("route source={source} target={target} enabled={enabled}"),
                            );
                        }
                    }
                    RelayControl::SetPrefix(prefix) => {
                        controls.set_delivery_prefix(prefix.unwrap_or_default());
                        log_event(
                            &log_path,
                            format!(
                                "prefix {}",
                                controls.delivery_prefix.as_deref().map(preview).unwrap_or_else(|| "off".to_string())
                            ),
                        );
                    }
                }
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
                    publish_transcript_output_with_controls(
                        &mut bus,
                        &router,
                        &log_path,
                        &pane_id,
                        &output,
                        &controls,
                    );
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
                let transcript_path = event
                    .transcript_path
                    .as_deref()
                    .map(PathBuf::from)
                    .or_else(|| {
                        let used_by_other_pane = claude_transcripts
                            .iter()
                            .filter(|(source, _)| source.as_str() != pane_id)
                            .map(|(_, path)| path.clone())
                            .collect::<std::collections::HashSet<_>>();
                        let discovered = discover_recent_claude_transcript(
                            &cwd,
                            started_at,
                            &used_by_other_pane,
                        );
                        if let Some(path) = &discovered {
                            log_event(
                                &log_path,
                                format!(
                                    "claude_transcript_fallback source={pane_id} path={}",
                                    path.display()
                                ),
                            );
                        }
                        discovered
                    });
                if let Some(path) = transcript_path.as_ref() {
                    claude_transcripts.insert(pane_id.clone(), path.clone());
                }

                let output = if let Some(path) = transcript_path.as_deref() {
                    let previous = claude_last_signatures.get(&pane_id).cloned();
                    let previous_stop_count = claude_last_stop_counts
                        .get(&pane_id)
                        .copied()
                        .unwrap_or(0);
                    let output = read_claude_transcript_with_retry(
                        path,
                        previous.as_ref(),
                        previous_stop_count,
                    )
                    .await;
                    let new_stop_count = count_claude_stop_hook_summaries(path);
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
                        transcript_path
                            .as_ref()
                            .map(|path| path.display().to_string())
                            .unwrap_or_default(),
                        output.output.len(),
                        preview(&output.output)
                    ),
                );

                publish_transcript_output_with_controls(
                    &mut bus,
                    &router,
                    &log_path,
                    &pane_id,
                    &output,
                    &controls,
                );
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

struct ManualRelayContext<'a> {
    router: &'a PairRouter,
    controls: &'a RelayControlState,
    pane_agents: &'a HashMap<String, String>,
    codex_transcripts: &'a HashMap<String, PathBuf>,
    claude_transcripts: &'a HashMap<String, PathBuf>,
    pending_prompts: &'a mut HashMap<String, String>,
    write_tx: &'a mpsc::Sender<(String, Vec<u8>)>,
    log_path: &'a std::path::Path,
}

async fn manual_relay(pane_id: &str, ctx: ManualRelayContext<'_>) {
    let Some(target) = ctx.router.counterpart(pane_id) else {
        log_event(
            ctx.log_path,
            format!("manual source={pane_id} skipped=unknown"),
        );
        return;
    };

    if !ctx.controls.route_enabled(pane_id, target) {
        log_event(
            ctx.log_path,
            format!("manual source={pane_id} target={target} skipped=route_disabled"),
        );
        return;
    }

    let output = if pane_uses_codex(ctx.pane_agents, pane_id) {
        ctx.codex_transcripts
            .get(pane_id)
            .map(|path| transcripts::codex::read_last_assistant(path))
            .unwrap_or_else(TranscriptOutput::empty)
    } else if pane_uses_claude(ctx.pane_agents, pane_id) {
        ctx.claude_transcripts
            .get(pane_id)
            .map(|path| transcripts::claude::read_last_assistant(path))
            .unwrap_or_else(TranscriptOutput::empty)
    } else {
        TranscriptOutput::empty()
    };

    if output.output.is_empty() || output.output.len() <= 6 {
        log_event(
            ctx.log_path,
            format!("manual source={pane_id} target={target} skipped=no_output"),
        );
        return;
    }

    let content = ctx.controls.delivered_content(&output.output);
    ctx.pending_prompts
        .insert(target.to_string(), normalize_prompt_text(&content));
    let target_agent = ctx
        .pane_agents
        .get(target)
        .map(String::as_str)
        .unwrap_or("claude");
    log_event(
        ctx.log_path,
        format!(
            "manual source={pane_id} target={target} len={} text=\"{}\"",
            content.len(),
            preview(&content)
        ),
    );
    send_relay_via_channel(ctx.write_tx, target, &content, target_agent).await;
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

#[derive(Debug, Clone)]
struct RelayControlState {
    a_to_b_enabled: bool,
    b_to_a_enabled: bool,
    delivery_prefix: Option<String>,
}

impl Default for RelayControlState {
    fn default() -> Self {
        Self {
            a_to_b_enabled: true,
            b_to_a_enabled: true,
            delivery_prefix: None,
        }
    }
}

impl RelayControlState {
    fn set_route_enabled(&mut self, source: &str, target: &str, enabled: bool) -> bool {
        match (source, target) {
            ("a", "b") => {
                self.a_to_b_enabled = enabled;
                true
            }
            ("b", "a") => {
                self.b_to_a_enabled = enabled;
                true
            }
            _ => false,
        }
    }

    fn route_enabled(&self, source: &str, target: &str) -> bool {
        match (source, target) {
            ("a", "b") => self.a_to_b_enabled,
            ("b", "a") => self.b_to_a_enabled,
            _ => false,
        }
    }

    fn set_delivery_prefix(&mut self, prefix: impl Into<String>) {
        let prefix = prefix.into();
        self.delivery_prefix = (!prefix.is_empty()).then_some(prefix);
    }

    fn delivered_content(&self, content: &str) -> String {
        match &self.delivery_prefix {
            Some(prefix) => format!("{prefix}{content}"),
            None => content.to_string(),
        }
    }
}

fn publish_transcript_output_with_controls(
    bus: &mut MessageBus,
    router: &PairRouter,
    log_path: &std::path::Path,
    pane_id: &str,
    output: &TranscriptOutput,
    controls: &RelayControlState,
) -> bool {
    if output.output.is_empty() || output.output.len() <= 6 {
        return false;
    }

    let agent_msg = Message::new_agent(pane_id, &output.output);
    let Some(relay_msg) = router.route(&agent_msg) else {
        return false;
    };

    let source = relay_msg.source_node_id;
    let target = relay_msg.target_node_id;
    if !controls.route_enabled(&source, &target) {
        log_event(
            log_path,
            format!(
                "route_disabled source={source} target={target} len={} text=\"{}\"",
                output.output.len(),
                preview(&output.output)
            ),
        );
        return false;
    }

    let content = controls.delivered_content(&output.output);
    let delivered_len = content.len();
    let delivered_preview = preview(&content);
    let relay_msg = Message::new_relay(&source, &target, &content);
    let published = bus.publish(relay_msg);
    log_event(
        log_path,
        format!(
            "{} source={source} target={target} len={delivered_len} text=\"{delivered_preview}\"",
            if published { "publish" } else { "dedup" }
        ),
    );
    published
}

#[cfg(test)]
fn publish_bound_codex_transcript_with_controls(
    bus: &mut MessageBus,
    router: &PairRouter,
    log_path: &std::path::Path,
    pane_id: &str,
    transcripts: &HashMap<String, PathBuf>,
    last_signatures: &mut HashMap<String, String>,
    controls: &RelayControlState,
) -> bool {
    let Some(path) = transcripts.get(pane_id) else {
        return false;
    };

    let output = drop_seen_signature(
        pane_id,
        transcripts::codex::read_last_assistant(path),
        last_signatures,
    );
    publish_transcript_output_with_controls(bus, router, log_path, pane_id, &output, controls)
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

    fn assert_relay_writes(writes: &[(String, Vec<u8>)], expected_target: &str, expected: &str) {
        assert!(
            !writes.is_empty(),
            "expected relay to forward something, got nothing"
        );
        for (target, _) in writes {
            assert_eq!(
                target, expected_target,
                "relay should target pane {expected_target}, got target {target}"
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
        assert!(
            body.contains(expected),
            "paste body missing expected content: {body:?}"
        );
        assert!(
            writes.iter().any(|(_, b)| b == b"\r"),
            "expected trailing Enter byte"
        );
    }

    fn restore_codex_home(previous: Option<std::ffi::OsString>) {
        if let Some(prev) = previous {
            std::env::set_var("CODEX_HOME", prev);
        } else {
            std::env::remove_var("CODEX_HOME");
        }
    }

    fn restore_claude_home(previous: Option<std::ffi::OsString>) {
        if let Some(prev) = previous {
            std::env::set_var("CLAUDE_HOME", prev);
        } else {
            std::env::remove_var("CLAUDE_HOME");
        }
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

    fn write_claude_project_transcript(
        path: &std::path::Path,
        cwd: &std::path::Path,
        timestamp: chrono::DateTime<chrono::Utc>,
        assistant_text: &str,
    ) {
        let cwd_json = serde_json::to_string(&cwd.to_string_lossy()).unwrap();
        let ts = timestamp.to_rfc3339();
        let assistant_json = serde_json::to_string(assistant_text).unwrap();
        let body = format!(
            "{{\"type\":\"assistant\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":{assistant_json}}}]}},\"cwd\":{cwd_json},\"timestamp\":\"{ts}\"}}\n\
             {{\"type\":\"system\",\"subtype\":\"stop_hook_summary\",\"cwd\":{cwd_json},\"timestamp\":\"{ts}\"}}\n",
        );
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
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

    fn transcript_output(text: &str) -> TranscriptOutput {
        TranscriptOutput::new(text.to_string(), format!("test-signature-{text}"))
    }

    #[test]
    fn relay_route_control_disables_and_enables_a_to_b() {
        let temp = tempdir().unwrap();
        let router = PairRouter::new("a", "b");
        let mut bus = MessageBus::new();
        let mut rx_b = bus.subscribe("b");
        let output = transcript_output("CONTROLLED_A_TO_B_OUTPUT");
        let mut controls = RelayControlState::default();

        assert!(controls.set_route_enabled("a", "b", false));
        assert!(!publish_transcript_output_with_controls(
            &mut bus,
            &router,
            &temp.path().join("relay.log"),
            "a",
            &output,
            &controls,
        ));
        assert!(
            rx_b.try_recv().is_err(),
            "disabled A->B route should not deliver"
        );

        assert!(controls.set_route_enabled("a", "b", true));
        assert!(publish_transcript_output_with_controls(
            &mut bus,
            &router,
            &temp.path().join("relay.log"),
            "a",
            &output,
            &controls,
        ));
        let msg = rx_b.try_recv().expect("enabled A->B route should deliver");
        assert_eq!(msg.source_node_id, "a");
        assert_eq!(msg.target_node_id, "b");
        assert_eq!(msg.content, output.output);
    }

    #[test]
    fn relay_route_control_disables_and_enables_b_to_a() {
        let temp = tempdir().unwrap();
        let router = PairRouter::new("a", "b");
        let mut bus = MessageBus::new();
        let mut rx_a = bus.subscribe("a");
        let output = transcript_output("CONTROLLED_B_TO_A_OUTPUT");
        let mut controls = RelayControlState::default();

        assert!(controls.set_route_enabled("b", "a", false));
        assert!(!publish_transcript_output_with_controls(
            &mut bus,
            &router,
            &temp.path().join("relay.log"),
            "b",
            &output,
            &controls,
        ));
        assert!(
            rx_a.try_recv().is_err(),
            "disabled B->A route should not deliver"
        );

        assert!(controls.set_route_enabled("b", "a", true));
        assert!(publish_transcript_output_with_controls(
            &mut bus,
            &router,
            &temp.path().join("relay.log"),
            "b",
            &output,
            &controls,
        ));
        let msg = rx_a.try_recv().expect("enabled B->A route should deliver");
        assert_eq!(msg.source_node_id, "b");
        assert_eq!(msg.target_node_id, "a");
        assert_eq!(msg.content, output.output);
    }

    #[test]
    fn manual_relay_request_reads_bound_codex_transcript() {
        let temp = tempdir().unwrap();
        let cwd = temp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        let rollout = temp.path().join("rollout-bound.jsonl");
        let answer = "MANUAL_BOUND_CODEX_RESPONSE";
        write_codex_rollout(
            &rollout,
            &cwd,
            chrono::Utc::now(),
            "BOUND_CODEX_PROMPT",
            answer,
        );

        let router = PairRouter::new("a", "b");
        let mut bus = MessageBus::new();
        let mut rx_a = bus.subscribe("a");
        let transcripts = HashMap::from([("b".to_string(), rollout)]);
        let mut last_signatures = HashMap::new();
        let controls = RelayControlState::default();

        assert!(publish_bound_codex_transcript_with_controls(
            &mut bus,
            &router,
            &temp.path().join("relay.log"),
            "b",
            &transcripts,
            &mut last_signatures,
            &controls,
        ));
        let msg = rx_a
            .try_recv()
            .expect("manual request should deliver from bound Codex rollout");
        assert_eq!(msg.source_node_id, "b");
        assert_eq!(msg.target_node_id, "a");
        assert_eq!(msg.content, answer);

        assert!(!publish_bound_codex_transcript_with_controls(
            &mut bus,
            &router,
            &temp.path().join("relay.log"),
            "b",
            &transcripts,
            &mut last_signatures,
            &controls,
        ));
        assert!(
            rx_a.try_recv().is_err(),
            "manual relay should deduplicate the already delivered transcript output"
        );
    }

    #[tokio::test]
    async fn manual_relay_primes_codex_target_prompt_binding() {
        let temp = tempdir().unwrap();
        let log_path = temp.path().join("relay.log");
        let transcript_path = temp.path().join("claude.jsonl");
        let answer = "MANUAL_CLAUDE_TO_CODEX_PROMPT";
        write_claude_transcript(&transcript_path, answer);

        let router = PairRouter::new("a", "b");
        let controls = RelayControlState::default();
        let pane_agents = HashMap::from([
            ("a".to_string(), "claude".to_string()),
            ("b".to_string(), "codex".to_string()),
        ]);
        let codex_transcripts = HashMap::new();
        let claude_transcripts = HashMap::from([("a".to_string(), transcript_path.to_path_buf())]);
        let mut pending_prompts = HashMap::new();
        let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(8);

        manual_relay(
            "a",
            ManualRelayContext {
                router: &router,
                controls: &controls,
                pane_agents: &pane_agents,
                codex_transcripts: &codex_transcripts,
                claude_transcripts: &claude_transcripts,
                pending_prompts: &mut pending_prompts,
                write_tx: &write_tx,
                log_path: &log_path,
            },
        )
        .await;

        assert_eq!(
            pending_prompts.get("b").map(String::as_str),
            Some(answer),
            "manual relay should prime Codex transcript binding for the target pane"
        );
        let writes = collect_writes(&mut write_rx, Duration::from_secs(1)).await;
        assert_relay_writes(&writes, "b", answer);
    }

    #[test]
    fn delivered_content_can_be_prefixed_before_publish() {
        let temp = tempdir().unwrap();
        let router = PairRouter::new("a", "b");
        let mut bus = MessageBus::new();
        let mut rx_b = bus.subscribe("b");
        let output = transcript_output("PREFIXED_DELIVERY_BODY");
        let mut controls = RelayControlState::default();
        controls.set_delivery_prefix("[relay from peer] ");

        assert!(publish_transcript_output_with_controls(
            &mut bus,
            &router,
            &temp.path().join("relay.log"),
            "a",
            &output,
            &controls,
        ));
        let msg = rx_b.try_recv().expect("prefixed delivery should publish");
        assert_eq!(msg.source_node_id, "a");
        assert_eq!(msg.target_node_id, "b");
        assert_eq!(msg.content, "[relay from peer] PREFIXED_DELIVERY_BODY");
        assert_eq!(output.output, "PREFIXED_DELIVERY_BODY");
    }

    #[tokio::test]
    async fn communication_gate_claude_to_claude() {
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
        let (_control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
        let (_input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
        let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(16);
        let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

        let inputs = RelayInputs {
            cwd: std::env::current_dir().unwrap(),
            started_at: chrono::Utc::now(),
            log_path,
            pane_agents,
            hook_rx,
            control_rx,
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
    async fn communication_gate_claude_to_codex() {
        let temp = tempdir().unwrap();
        let log_path = temp.path().join("relay.log");
        let transcript_path = temp.path().join("claude-to-codex.jsonl");
        let answer = "COMM_GATE_CLAUDE_TO_CODEX";
        write_claude_transcript(&transcript_path, answer);

        let pane_agents = HashMap::from([
            ("a".to_string(), "claude".to_string()),
            ("b".to_string(), "codex".to_string()),
        ]);

        let (hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
        let (_control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
        let (_input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
        let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(16);
        let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

        let handle = tokio::spawn(run(RelayInputs {
            cwd: std::env::current_dir().unwrap(),
            started_at: chrono::Utc::now(),
            log_path,
            pane_agents,
            hook_rx,
            control_rx,
            input_rx,
            write_tx,
            shutdown_rx: shutdown_tx.subscribe(),
        }));

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

        assert_relay_writes(&writes, "b", answer);
    }

    #[tokio::test]
    async fn communication_gate_claude_to_codex_without_hook_transcript_path() {
        let _guard = codex_home_lock().lock().await;

        let temp = tempdir().unwrap();
        let claude_home = temp.path().join("claude");
        let cwd = temp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        let prev_claude_home = std::env::var_os("CLAUDE_HOME");
        std::env::set_var("CLAUDE_HOME", &claude_home);

        let started_at = chrono::Utc::now() - chrono::Duration::seconds(10);
        let transcript_ts = chrono::Utc::now() + chrono::Duration::seconds(1);
        let transcript_path = claude_home
            .join("projects")
            .join("-tmp-project")
            .join("claude-fallback.jsonl");
        let answer = "COMM_GATE_CLAUDE_TO_CODEX_FALLBACK";
        write_claude_project_transcript(&transcript_path, &cwd, transcript_ts, answer);

        let pane_agents = HashMap::from([
            ("a".to_string(), "claude".to_string()),
            ("b".to_string(), "codex".to_string()),
        ]);

        let (hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
        let (_control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
        let (_input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
        let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(16);
        let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

        let handle = tokio::spawn(run(RelayInputs {
            cwd: cwd.clone(),
            started_at,
            log_path: temp.path().join("relay.log"),
            pane_agents,
            hook_rx,
            control_rx,
            input_rx,
            write_tx,
            shutdown_rx: shutdown_tx.subscribe(),
        }));

        hook_tx
            .send(HookEvent {
                terminal_id: "a".to_string(),
                transcript_path: None,
            })
            .await
            .unwrap();

        let writes = collect_writes(&mut write_rx, Duration::from_secs(5)).await;
        let _ = shutdown_tx.send(());
        let _ = timeout(Duration::from_secs(2), handle).await;

        restore_claude_home(prev_claude_home);

        assert_relay_writes(&writes, "b", answer);
    }

    #[tokio::test]
    async fn communication_gate_codex_to_codex() {
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
            .join("rollout-codex-to-codex.jsonl");
        let prompt = "COMM_GATE_CODEX_CODEX_PROMPT";
        let answer = "COMM_GATE_CODEX_TO_CODEX";
        write_codex_rollout(&rollout, &cwd, session_ts, prompt, answer);

        let pane_agents = HashMap::from([
            ("a".to_string(), "codex".to_string()),
            ("b".to_string(), "codex".to_string()),
        ]);

        let (_hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
        let (_control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
        let (input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
        let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(16);
        let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

        let handle = tokio::spawn(run(RelayInputs {
            cwd: cwd.clone(),
            started_at,
            log_path: temp.path().join("relay.log"),
            pane_agents,
            hook_rx,
            control_rx,
            input_rx,
            write_tx,
            shutdown_rx: shutdown_tx.subscribe(),
        }));

        input_tx
            .send(("a".to_string(), prompt.to_string()))
            .await
            .unwrap();

        let writes = collect_writes(&mut write_rx, Duration::from_secs(8)).await;
        let _ = shutdown_tx.send(());
        let _ = timeout(Duration::from_secs(2), handle).await;

        restore_codex_home(prev_codex_home);

        assert_relay_writes(&writes, "b", answer);
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
        let (_control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
        let (input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
        let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(16);
        let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

        let inputs = RelayInputs {
            cwd: cwd.clone(),
            started_at,
            log_path: temp.path().join("relay.log"),
            pane_agents,
            hook_rx,
            control_rx,
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
    async fn communication_gate_codex_to_claude() {
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
        let (_control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
        let (input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
        let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(16);
        let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

        let inputs = RelayInputs {
            cwd: cwd.clone(),
            started_at,
            log_path: temp.path().join("relay.log"),
            pane_agents,
            hook_rx,
            control_rx,
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
        let (_control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
        let (input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
        let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(32);
        let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

        let inputs = RelayInputs {
            cwd: cwd.clone(),
            started_at,
            log_path: temp.path().join("relay.log"),
            pane_agents,
            hook_rx,
            control_rx,
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
        let (_control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
        let (input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
        let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(32);
        let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

        let handle = tokio::spawn(run(RelayInputs {
            cwd: cwd.clone(),
            started_at,
            log_path: temp.path().join("relay.log"),
            pane_agents,
            hook_rx,
            control_rx,
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
