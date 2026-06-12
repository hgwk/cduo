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
use crate::native::relay_control::*;
use crate::pair_router::PairRouter;
use crate::relay_core::{
    count_claude_stop_hook_summaries, discover_recent_claude_transcript,
    discover_recent_codex_transcript, drop_seen_signature, log_event, normalize_prompt_text,
    pane_uses_claude, pane_uses_codex, preview, read_claude_transcript_with_retry,
    submit_delay_for_agent,
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
    ResetStop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayStatus {
    pub auto_stopped: bool,
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
    pub status_tx: Option<mpsc::Sender<RelayStatus>>,
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
        status_tx,
        mut shutdown_rx,
    } = inputs;

    let mut bus = MessageBus::new();
    let router = PairRouter::new("a", "b");
    let rx_a = bus.subscribe("a");
    let rx_b = bus.subscribe("b");

    let codex_transcripts: HashMap<String, PathBuf> = HashMap::new();
    let codex_last_signatures: HashMap<String, String> = HashMap::new();
    let claude_last_signatures: HashMap<String, String> = HashMap::new();
    let claude_last_stop_counts: HashMap<String, usize> = HashMap::new();
    let claude_transcripts: HashMap<String, PathBuf> = HashMap::new();
    let codex_pending_prompts: HashMap<String, String> = HashMap::new();
    let controls = RelayControlState::from_env();

    log_event(&log_path, "native_relay_start");
    let mut state = RelayState {
        bus,
        router,
        rx_a,
        rx_b,
        codex_transcripts,
        codex_last_signatures,
        claude_last_signatures,
        claude_last_stop_counts,
        claude_transcripts,
        codex_pending_prompts,
        controls,
        last_reported_status: None,
    };
    state.report_status(status_tx.as_ref()).await;

    loop {
        tokio::select! {
            Some((pane_id, prompt)) = input_rx.recv() => {
                handle_relay_input(&pane_id, &prompt, &pane_agents, &log_path, &mut state);
                state.flush_deliveries(&log_path, &write_tx, &pane_agents).await;
                state.report_status(status_tx.as_ref()).await;
            }
            Some(control) = control_rx.recv() => {
                handle_relay_control(control, &pane_agents, &write_tx, &log_path, &mut state).await;
                state.report_status(status_tx.as_ref()).await;
            }
            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                poll_codex_transcripts(
                    &cwd,
                    started_at,
                    &pane_agents,
                    &log_path,
                    &mut state,
                );
                state.flush_deliveries(&log_path, &write_tx, &pane_agents).await;
                state.report_status(status_tx.as_ref()).await;
            }
            Some(event) = hook_rx.recv() => {
                handle_claude_hook_event(
                    event,
                    &cwd,
                    started_at,
                    &pane_agents,
                    &log_path,
                    &mut state,
                ).await;
                state.flush_deliveries(&log_path, &write_tx, &pane_agents).await;
                state.report_status(status_tx.as_ref()).await;
            }
            _ = shutdown_rx.recv() => break,
        }
    }
    log_event(&log_path, "native_relay_stop");
}

struct RelayState {
    bus: MessageBus,
    router: PairRouter,
    rx_a: mpsc::Receiver<Message>,
    rx_b: mpsc::Receiver<Message>,
    codex_transcripts: HashMap<String, PathBuf>,
    codex_last_signatures: HashMap<String, String>,
    claude_last_signatures: HashMap<String, String>,
    claude_last_stop_counts: HashMap<String, usize>,
    claude_transcripts: HashMap<String, PathBuf>,
    codex_pending_prompts: HashMap<String, String>,
    controls: RelayControlState,
    last_reported_status: Option<RelayStatus>,
}

impl RelayState {
    async fn flush_deliveries(
        &mut self,
        log_path: &std::path::Path,
        write_tx: &mpsc::Sender<(String, Vec<u8>)>,
        pane_agents: &HashMap<String, String>,
    ) {
        deliver_via_channel(
            log_path,
            &mut self.rx_a,
            &mut self.rx_b,
            write_tx,
            pane_agents,
            &mut self.codex_pending_prompts,
        )
        .await;
    }

    async fn report_status(&mut self, status_tx: Option<&mpsc::Sender<RelayStatus>>) {
        let status = RelayStatus {
            auto_stopped: self.controls.stopped,
        };
        if self.last_reported_status == Some(status) {
            return;
        }
        self.last_reported_status = Some(status);
        if let Some(status_tx) = status_tx {
            let _ = status_tx.send(status).await;
        }
    }
}

fn handle_relay_input(
    pane_id: &str,
    prompt: &str,
    pane_agents: &HashMap<String, String>,
    log_path: &std::path::Path,
    state: &mut RelayState,
) {
    let prompt = normalize_prompt_text(prompt);
    if pane_uses_codex(pane_agents, pane_id) && !prompt.is_empty() {
        log_event(
            log_path,
            format!("codex_input source={pane_id} text=\"{}\"", preview(&prompt)),
        );
        state
            .codex_pending_prompts
            .insert(pane_id.to_string(), prompt);
    }
}

async fn handle_relay_control(
    control: RelayControl,
    pane_agents: &HashMap<String, String>,
    write_tx: &mpsc::Sender<(String, Vec<u8>)>,
    log_path: &std::path::Path,
    state: &mut RelayState,
) {
    match control {
        RelayControl::ManualRelay { pane_id } => {
            manual_relay(
                &pane_id,
                ManualRelayContext {
                    router: &state.router,
                    controls: &state.controls,
                    pane_agents,
                    codex_transcripts: &state.codex_transcripts,
                    claude_transcripts: &state.claude_transcripts,
                    pending_prompts: &mut state.codex_pending_prompts,
                    write_tx,
                    log_path,
                },
            )
            .await;
        }
        RelayControl::SetRoute {
            source,
            target,
            enabled,
        } => {
            if state.controls.set_route_enabled(&source, &target, enabled) {
                log_event(
                    log_path,
                    format!("route source={source} target={target} enabled={enabled}"),
                );
            }
        }
        RelayControl::SetPrefix(prefix) => {
            state
                .controls
                .set_delivery_prefix(prefix.unwrap_or_default());
            log_event(
                log_path,
                format!(
                    "prefix {}",
                    state
                        .controls
                        .delivery_prefix
                        .as_deref()
                        .map(preview)
                        .unwrap_or_else(|| "off".to_string())
                ),
            );
        }
        RelayControl::ResetStop => {
            state.controls.reset_stop();
            state.bus.clear_dedup();
            log_event(log_path, "relay_reset_stop");
        }
    }
}

fn poll_codex_transcripts(
    cwd: &std::path::Path,
    started_at: DateTime<Utc>,
    pane_agents: &HashMap<String, String>,
    log_path: &std::path::Path,
    state: &mut RelayState,
) {
    for pane in ["a", "b"] {
        if !pane_uses_codex(pane_agents, pane) {
            continue;
        }
        let pane_id = pane.to_string();
        ensure_codex_transcript_local(
            &pane_id,
            &mut state.codex_transcripts,
            &state.codex_pending_prompts,
            cwd,
            started_at,
            log_path,
        );
        let Some(path) = state.codex_transcripts.get(&pane_id) else {
            continue;
        };
        let output = drop_seen_signature(
            &pane_id,
            transcripts::codex::read_last_assistant(path),
            &mut state.codex_last_signatures,
        );
        if should_suppress_transcript_output(&output.output) {
            continue;
        }
        publish_transcript_output_with_controls(
            &mut state.bus,
            &state.router,
            log_path,
            &pane_id,
            &output,
            &mut state.controls,
        );
    }
}

async fn handle_claude_hook_event(
    event: HookEvent,
    cwd: &std::path::Path,
    started_at: DateTime<Utc>,
    pane_agents: &HashMap<String, String>,
    log_path: &std::path::Path,
    state: &mut RelayState,
) {
    let pane_id = event.terminal_id;
    if !pane_uses_claude(pane_agents, &pane_id) {
        return;
    }
    let transcript_path = event
        .transcript_path
        .as_deref()
        .map(PathBuf::from)
        .or_else(|| {
            let used_by_other_pane = state
                .claude_transcripts
                .iter()
                .filter(|(source, _)| source.as_str() != pane_id)
                .map(|(_, path)| path.clone())
                .collect::<std::collections::HashSet<_>>();
            let discovered =
                discover_recent_claude_transcript(cwd, started_at, &used_by_other_pane);
            if let Some(path) = &discovered {
                log_event(
                    log_path,
                    format!(
                        "claude_transcript_fallback source={pane_id} path={}",
                        path.display()
                    ),
                );
            }
            discovered
        });
    if let Some(path) = transcript_path.as_ref() {
        state
            .claude_transcripts
            .insert(pane_id.clone(), path.clone());
    }

    let output = if let Some(path) = transcript_path.as_deref() {
        let previous = state.claude_last_signatures.get(&pane_id).cloned();
        let previous_stop_count = state
            .claude_last_stop_counts
            .get(&pane_id)
            .copied()
            .unwrap_or(0);
        let output =
            read_claude_transcript_with_retry(path, previous.as_ref(), previous_stop_count).await;
        let new_stop_count = count_claude_stop_hook_summaries(path);
        if new_stop_count > previous_stop_count {
            state
                .claude_last_stop_counts
                .insert(pane_id.clone(), new_stop_count);
        }
        drop_seen_signature(&pane_id, output, &mut state.claude_last_signatures)
    } else {
        TranscriptOutput::empty()
    };

    log_event(
        log_path,
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
        &mut state.bus,
        &state.router,
        log_path,
        &pane_id,
        &output,
        &mut state.controls,
    );
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
        let content = prefixed_agent_content(&msg.source_node_id, &msg.content, pane_agents);
        log_deliver(log_path, "a", &content);
        pending_prompts.insert("a".to_string(), normalize_prompt_text(&content));
        let agent = pane_agents.get("a").map(String::as_str).unwrap_or("claude");
        send_relay_via_channel(write_tx, "a", &content, agent).await;
    }
    while let Ok(msg) = rx_b.try_recv() {
        let content = prefixed_agent_content(&msg.source_node_id, &msg.content, pane_agents);
        log_deliver(log_path, "b", &content);
        pending_prompts.insert("b".to_string(), normalize_prompt_text(&content));
        let agent = pane_agents.get("b").map(String::as_str).unwrap_or("claude");
        send_relay_via_channel(write_tx, "b", &content, agent).await;
    }
}

fn prefixed_agent_content(
    source: &str,
    content: &str,
    pane_agents: &HashMap<String, String>,
) -> String {
    format!(
        "Other {} says: {content}",
        agent_display_name(
            pane_agents
                .get(source)
                .map(String::as_str)
                .unwrap_or(source)
        )
    )
}

fn agent_display_name(agent: &str) -> String {
    match agent {
        "claude" => "Claude".to_string(),
        "codex" => "Codex".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => "Agent".to_string(),
            }
        }
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

    schedule_submit(write_tx, target, target_agent);
}

fn schedule_submit(write_tx: &mpsc::Sender<(String, Vec<u8>)>, target: &str, target_agent: &str) {
    let write_tx = write_tx.clone();
    let target = target.to_string();
    let delay = submit_delay_for_agent(target_agent);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(delay)).await;
        let _ = write_tx.send((target, b"\r".to_vec())).await;
    });
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

    if should_suppress_transcript_output(&output.output) {
        log_event(
            ctx.log_path,
            format!("manual source={pane_id} target={target} skipped=no_output"),
        );
        return;
    }

    let content = ctx.controls.delivered_content(&prefixed_agent_content(
        pane_id,
        &output.output,
        ctx.pane_agents,
    ));
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
    let Some(path) =
        discover_recent_codex_transcript(cwd, started_at, &used_by_other_pane, expected_prompt)
    else {
        log_event(
            log_path,
            format!(
                "codex_transcript_unmatched source={pane_id} prompt=\"{}\"",
                preview(expected_prompt)
            ),
        );
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
#[path = "relay_tests.rs"]
mod tests;
