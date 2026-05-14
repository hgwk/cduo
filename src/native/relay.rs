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
use crate::message_bus::{MessageBus, PublishResult};
use crate::pair_router::PairRouter;
use crate::relay_core::{
    count_claude_stop_hook_summaries, discover_recent_claude_transcript,
    discover_recent_codex_transcript, drop_seen_signature, log_event, normalize_prompt_text,
    pane_uses_claude, pane_uses_codex, preview, read_claude_transcript_with_retry,
    submit_delay_for_agent,
};
use crate::transcripts::{self, TranscriptOutput};

const SHORT_OUTPUT_SUPPRESSION_LIMIT: usize = 6;
const DEFAULT_STOP_TOKEN: &str = "~~~";

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

#[derive(Debug, Clone)]
struct RelayControlState {
    a_to_b_enabled: bool,
    b_to_a_enabled: bool,
    delivery_prefix: Option<String>,
    max_auto_relays: Option<usize>,
    stop_token: String,
    auto_relay_count: usize,
    stopped: bool,
}

impl Default for RelayControlState {
    fn default() -> Self {
        Self {
            a_to_b_enabled: true,
            b_to_a_enabled: true,
            delivery_prefix: None,
            max_auto_relays: None,
            stop_token: DEFAULT_STOP_TOKEN.to_string(),
            auto_relay_count: 0,
            stopped: false,
        }
    }
}

impl RelayControlState {
    fn from_env() -> Self {
        Self {
            max_auto_relays: std::env::var("CDUO_MAX_RELAY_TURNS")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| *value > 0),
            stop_token: std::env::var("CDUO_STOP_TOKEN")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_STOP_TOKEN.to_string()),
            ..Self::default()
        }
    }

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

    fn should_stop_for_marker(&self, content: &str) -> bool {
        content.contains("CDUO_STOP_RELAY")
            || content.contains("[CDUO_STOP]")
            || content.trim() == self.stop_token
    }

    fn stop(&mut self) {
        self.stopped = true;
    }

    fn can_publish_auto(&mut self) -> bool {
        if self.stopped {
            return false;
        }
        if self
            .max_auto_relays
            .is_some_and(|max| self.auto_relay_count >= max)
        {
            self.stopped = true;
            return false;
        }
        true
    }

    fn record_auto_publish(&mut self) {
        self.auto_relay_count += 1;
    }
}

fn publish_transcript_output_with_controls(
    bus: &mut MessageBus,
    router: &PairRouter,
    log_path: &std::path::Path,
    pane_id: &str,
    output: &TranscriptOutput,
    controls: &mut RelayControlState,
) -> bool {
    if controls.should_stop_for_marker(&output.output) {
        controls.stop();
        log_event(
            log_path,
            format!(
                "relay_stop_marker source={pane_id} len={} text=\"{}\"",
                output.output.len(),
                preview(&output.output)
            ),
        );
        return false;
    }
    if should_suppress_transcript_output(&output.output) {
        return false;
    }
    if !controls.can_publish_auto() {
        log_event(log_path, format!("relay_stopped source={pane_id}"));
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
    let publish_result = bus.publish(relay_msg);
    if publish_result == PublishResult::Delivered {
        controls.record_auto_publish();
    }
    log_event(
        log_path,
        format!(
            "{} source={source} target={target} len={delivered_len} text=\"{delivered_preview}\"",
            publish_result.log_label()
        ),
    );
    publish_result.is_delivered()
}

fn should_suppress_transcript_output(output: &str) -> bool {
    output.trim().is_empty() || output.len() <= SHORT_OUTPUT_SUPPRESSION_LIMIT
}

#[cfg(test)]
fn publish_bound_codex_transcript_with_controls(
    bus: &mut MessageBus,
    router: &PairRouter,
    log_path: &std::path::Path,
    pane_id: &str,
    transcripts: &HashMap<String, PathBuf>,
    last_signatures: &mut HashMap<String, String>,
    controls: &mut RelayControlState,
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

    /// Transcript discovery reads process env. Serialize tests that mutate
    /// `CODEX_HOME` or `CLAUDE_HOME` so parallel test threads do not interleave.
    fn env_lock() -> &'static Mutex<()> {
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

    /// Wait until `rx` has produced at least one bracketed-paste body and its
    /// delayed Enter, or the deadline expires. When the paste arrives near the
    /// deadline, allow a small submit-delay grace period so tests still observe
    /// the scheduled Enter.
    async fn collect_writes(
        rx: &mut mpsc::Receiver<(String, Vec<u8>)>,
        within: Duration,
    ) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        let mut deadline = tokio::time::Instant::now() + within;
        while tokio::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match timeout(remaining, rx.recv()).await {
                Ok(Some(item)) => {
                    let is_paste = String::from_utf8_lossy(&item.1).contains("\x1b[200~");
                    out.push(item);
                    if is_paste {
                        let submit_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
                        if submit_deadline > deadline {
                            deadline = submit_deadline;
                        }
                    }
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
        if out
            .iter()
            .any(|(_, bytes)| String::from_utf8_lossy(bytes).contains("\x1b[200~"))
            && !out.iter().any(|(_, bytes)| bytes == b"\r")
        {
            let submit_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            while tokio::time::Instant::now() < submit_deadline {
                let remaining =
                    submit_deadline.saturating_duration_since(tokio::time::Instant::now());
                match timeout(remaining, rx.recv()).await {
                    Ok(Some(item)) => {
                        let is_enter = item.1 == b"\r";
                        out.push(item);
                        if is_enter {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            out.extend(drain_writes(rx));
        }
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
    fn short_transcript_output_policy_has_explicit_boundaries() {
        assert!(should_suppress_transcript_output(""));
        assert!(should_suppress_transcript_output("OK"));
        assert!(should_suppress_transcript_output("Done."));
        assert!(should_suppress_transcript_output("123456"));
        assert!(!should_suppress_transcript_output("1234567"));
        assert!(!should_suppress_transcript_output("valid longer response"));
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
            &mut controls,
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
            &mut controls,
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
            &mut controls,
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
            &mut controls,
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
        let mut controls = RelayControlState::default();

        assert!(publish_bound_codex_transcript_with_controls(
            &mut bus,
            &router,
            &temp.path().join("relay.log"),
            "b",
            &transcripts,
            &mut last_signatures,
            &mut controls,
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
            &mut controls,
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

    #[tokio::test]
    async fn relay_delivery_schedules_enter_without_blocking_next_paste() {
        let temp = tempdir().unwrap();
        let log_path = temp.path().join("relay.log");
        let pane_agents = HashMap::from([
            ("a".to_string(), "claude".to_string()),
            ("b".to_string(), "claude".to_string()),
        ]);
        let mut bus = MessageBus::new();
        let mut rx_a = bus.subscribe("a");
        let mut rx_b = bus.subscribe("b");
        let (write_tx, mut write_rx) = mpsc::channel::<(String, Vec<u8>)>(8);
        let mut pending_prompts = HashMap::new();

        assert_eq!(
            bus.publish(Message::new_relay("test", "a", "FIRST_DELAYED_PASTE")),
            PublishResult::Delivered
        );
        assert_eq!(
            bus.publish(Message::new_relay("test", "b", "SECOND_DELAYED_PASTE")),
            PublishResult::Delivered
        );

        deliver_via_channel(
            &log_path,
            &mut rx_a,
            &mut rx_b,
            &write_tx,
            &pane_agents,
            &mut pending_prompts,
        )
        .await;

        let mut writes_before_enter = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(100);
        while tokio::time::Instant::now() < deadline {
            match timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
                write_rx.recv(),
            )
            .await
            {
                Ok(Some(item)) => writes_before_enter.push(item),
                Ok(None) => break,
                Err(_) => break,
            }
        }
        writes_before_enter.extend(drain_writes(&mut write_rx));
        let paste_count = writes_before_enter
            .iter()
            .filter(|(_, bytes)| String::from_utf8_lossy(bytes).contains("\x1b[200~"))
            .count();
        assert_eq!(
            paste_count, 2,
            "both paste bundles should be queued before Claude's submit delay elapses"
        );
        assert!(
            writes_before_enter.iter().all(|(_, bytes)| bytes != b"\r"),
            "Enter should remain delayed instead of blocking the relay drain"
        );

        let writes_after_delay = collect_writes(&mut write_rx, Duration::from_secs(2)).await;
        let enter_count = writes_after_delay
            .iter()
            .filter(|(_, bytes)| bytes == b"\r")
            .count();
        assert_eq!(enter_count, 2, "each delayed paste should still submit");
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
            &mut controls,
        ));
        let msg = rx_b.try_recv().expect("prefixed delivery should publish");
        assert_eq!(msg.source_node_id, "a");
        assert_eq!(msg.target_node_id, "b");
        assert_eq!(msg.content, "[relay from peer] PREFIXED_DELIVERY_BODY");
        assert_eq!(output.output, "PREFIXED_DELIVERY_BODY");
    }

    #[test]
    fn relay_stop_marker_blocks_publish_and_stops_future_auto_relay() {
        let temp = tempdir().unwrap();
        let router = PairRouter::new("a", "b");
        let mut bus = MessageBus::new();
        let mut rx_b = bus.subscribe("b");
        let mut controls = RelayControlState::default();

        assert!(!publish_transcript_output_with_controls(
            &mut bus,
            &router,
            &temp.path().join("relay.log"),
            "a",
            &transcript_output("done CDUO_STOP_RELAY"),
            &mut controls,
        ));
        assert!(rx_b.try_recv().is_err());

        assert!(!publish_transcript_output_with_controls(
            &mut bus,
            &router,
            &temp.path().join("relay.log"),
            "a",
            &transcript_output("NEXT_OUTPUT_SHOULD_NOT_RELAY"),
            &mut controls,
        ));
        assert!(rx_b.try_recv().is_err());
    }

    #[test]
    fn explicit_stop_token_only_stops_when_returned_exactly() {
        let temp = tempdir().unwrap();
        let router = PairRouter::new("a", "b");
        let mut bus = MessageBus::new();
        let mut rx_b = bus.subscribe("b");
        let mut controls = RelayControlState::default();

        assert!(publish_transcript_output_with_controls(
            &mut bus,
            &router,
            &temp.path().join("relay.log"),
            "a",
            &transcript_output("Here is a fenced block:\n~~~\nbody\n~~~"),
            &mut controls,
        ));
        assert!(rx_b.try_recv().is_ok());

        assert!(!publish_transcript_output_with_controls(
            &mut bus,
            &router,
            &temp.path().join("relay.log"),
            "a",
            &transcript_output("~~~"),
            &mut controls,
        ));
        assert!(rx_b.try_recv().is_err());

        assert!(!publish_transcript_output_with_controls(
            &mut bus,
            &router,
            &temp.path().join("relay.log"),
            "a",
            &transcript_output("OUTPUT_AFTER_EXPLICIT_STOP"),
            &mut controls,
        ));
        assert!(rx_b.try_recv().is_err());
    }

    #[tokio::test]
    async fn relay_reports_auto_stopped_status_after_explicit_stop_token() {
        let temp = tempdir().unwrap();
        let log_path = temp.path().join("relay.log");
        let transcript_path = temp.path().join("claude-stop.jsonl");
        write_claude_transcript(&transcript_path, "~~~");

        let pane_agents = HashMap::from([
            ("a".to_string(), "claude".to_string()),
            ("b".to_string(), "claude".to_string()),
        ]);

        let (hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
        let (_control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
        let (_input_tx, input_rx) = mpsc::channel::<(String, String)>(8);
        let (write_tx, _write_rx) = mpsc::channel::<(String, Vec<u8>)>(16);
        let (status_tx, mut status_rx) = mpsc::channel::<RelayStatus>(8);
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
            status_tx: Some(status_tx),
            shutdown_rx: shutdown_tx.subscribe(),
        }));

        let initial = timeout(Duration::from_secs(1), status_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(!initial.auto_stopped);

        hook_tx
            .send(HookEvent {
                terminal_id: "a".to_string(),
                transcript_path: Some(transcript_path.to_string_lossy().into_owned()),
            })
            .await
            .unwrap();

        let stopped = timeout(Duration::from_secs(5), async {
            loop {
                if let Some(status) = status_rx.recv().await {
                    if status.auto_stopped {
                        break status;
                    }
                }
            }
        })
        .await
        .unwrap();

        let _ = shutdown_tx.send(());
        let _ = timeout(Duration::from_secs(2), handle).await;

        assert!(stopped.auto_stopped);
    }

    #[test]
    fn max_relay_turns_blocks_auto_ping_pong_after_limit() {
        let temp = tempdir().unwrap();
        let router = PairRouter::new("a", "b");
        let mut bus = MessageBus::new();
        let mut rx_b = bus.subscribe("b");
        let mut controls = RelayControlState {
            max_auto_relays: Some(1),
            ..RelayControlState::default()
        };

        assert!(publish_transcript_output_with_controls(
            &mut bus,
            &router,
            &temp.path().join("relay.log"),
            "a",
            &transcript_output("FIRST_ALLOWED_RELAY"),
            &mut controls,
        ));
        assert!(rx_b.try_recv().is_ok());

        assert!(!publish_transcript_output_with_controls(
            &mut bus,
            &router,
            &temp.path().join("relay.log"),
            "a",
            &transcript_output("SECOND_BLOCKED_RELAY"),
            &mut controls,
        ));
        assert!(rx_b.try_recv().is_err());
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
            status_tx: None,
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
            status_tx: None,
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
        let _guard = env_lock().lock().await;

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
            status_tx: None,
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
    async fn communication_gate_claude_b_to_codex_a_without_hook_transcript_path() {
        let _guard = env_lock().lock().await;

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
            .join("claude-b-fallback.jsonl");
        let answer = "COMM_GATE_CLAUDE_B_TO_CODEX_A_FALLBACK";
        write_claude_project_transcript(&transcript_path, &cwd, transcript_ts, answer);

        let pane_agents = HashMap::from([
            ("a".to_string(), "codex".to_string()),
            ("b".to_string(), "claude".to_string()),
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
            status_tx: None,
            shutdown_rx: shutdown_tx.subscribe(),
        }));

        hook_tx
            .send(HookEvent {
                terminal_id: "b".to_string(),
                transcript_path: None,
            })
            .await
            .unwrap();

        let writes = collect_writes(&mut write_rx, Duration::from_secs(5)).await;
        let _ = shutdown_tx.send(());
        let _ = timeout(Duration::from_secs(2), handle).await;

        restore_claude_home(prev_claude_home);

        assert_relay_writes(&writes, "a", answer);
    }

    #[tokio::test]
    async fn communication_gate_claude_b_to_claude_a_without_hook_transcript_path() {
        let _guard = env_lock().lock().await;

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
            .join("claude-b-to-claude-a-fallback.jsonl");
        let answer = "COMM_GATE_CLAUDE_B_TO_CLAUDE_A_FALLBACK";
        write_claude_project_transcript(&transcript_path, &cwd, transcript_ts, answer);

        let pane_agents = HashMap::from([
            ("a".to_string(), "claude".to_string()),
            ("b".to_string(), "claude".to_string()),
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
            status_tx: None,
            shutdown_rx: shutdown_tx.subscribe(),
        }));

        hook_tx
            .send(HookEvent {
                terminal_id: "b".to_string(),
                transcript_path: None,
            })
            .await
            .unwrap();

        let writes = collect_writes(&mut write_rx, Duration::from_secs(5)).await;
        let _ = shutdown_tx.send(());
        let _ = timeout(Duration::from_secs(2), handle).await;

        restore_claude_home(prev_claude_home);

        assert_relay_writes(&writes, "a", answer);
    }

    #[tokio::test]
    async fn communication_gate_route_off_blocks_a_to_b_in_run_loop() {
        let temp = tempdir().unwrap();
        let log_path = temp.path().join("relay.log");
        let transcript_path = temp.path().join("claude-route-off.jsonl");
        write_claude_transcript(&transcript_path, "ROUTE_OFF_A_TO_B_SHOULD_NOT_SEND");

        let pane_agents = HashMap::from([
            ("a".to_string(), "claude".to_string()),
            ("b".to_string(), "codex".to_string()),
        ]);

        let (hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
        let (control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
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
            status_tx: None,
            shutdown_rx: shutdown_tx.subscribe(),
        }));

        control_tx
            .send(RelayControl::SetRoute {
                source: "a".to_string(),
                target: "b".to_string(),
                enabled: false,
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        hook_tx
            .send(HookEvent {
                terminal_id: "a".to_string(),
                transcript_path: Some(transcript_path.to_string_lossy().into_owned()),
            })
            .await
            .unwrap();

        let writes = collect_writes(&mut write_rx, Duration::from_millis(900)).await;
        let _ = shutdown_tx.send(());
        let _ = timeout(Duration::from_secs(2), handle).await;

        assert!(writes.is_empty(), "disabled A->B route should not write");
    }

    #[tokio::test]
    async fn communication_gate_route_off_blocks_b_to_a_in_run_loop() {
        let temp = tempdir().unwrap();
        let log_path = temp.path().join("relay.log");
        let transcript_path = temp.path().join("claude-route-off-b.jsonl");
        write_claude_transcript(&transcript_path, "ROUTE_OFF_B_TO_A_SHOULD_NOT_SEND");

        let pane_agents = HashMap::from([
            ("a".to_string(), "codex".to_string()),
            ("b".to_string(), "claude".to_string()),
        ]);

        let (hook_tx, hook_rx) = mpsc::channel::<HookEvent>(8);
        let (control_tx, control_rx) = mpsc::channel::<RelayControl>(8);
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
            status_tx: None,
            shutdown_rx: shutdown_tx.subscribe(),
        }));

        control_tx
            .send(RelayControl::SetRoute {
                source: "b".to_string(),
                target: "a".to_string(),
                enabled: false,
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        hook_tx
            .send(HookEvent {
                terminal_id: "b".to_string(),
                transcript_path: Some(transcript_path.to_string_lossy().into_owned()),
            })
            .await
            .unwrap();

        let writes = collect_writes(&mut write_rx, Duration::from_millis(900)).await;
        let _ = shutdown_tx.send(());
        let _ = timeout(Duration::from_secs(2), handle).await;

        assert!(writes.is_empty(), "disabled B->A route should not write");
    }

    #[tokio::test]
    async fn communication_gate_codex_to_codex() {
        let _guard = env_lock().lock().await;

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
            status_tx: None,
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
    async fn communication_gate_codex_resume_session_with_old_session_timestamp() {
        let _guard = env_lock().lock().await;

        let temp = tempdir().unwrap();
        let codex_home = temp.path().join("codex");
        let cwd = temp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        let prev_codex_home = std::env::var_os("CODEX_HOME");
        std::env::set_var("CODEX_HOME", &codex_home);

        let started_at = chrono::Utc::now() - chrono::Duration::seconds(1);
        let old_session_ts = chrono::Utc::now() - chrono::Duration::days(1);
        let rollout = codex_home
            .join("sessions")
            .join("2026")
            .join("04")
            .join("27")
            .join("rollout-resumed-codex.jsonl");
        let prompt = "RESUMED_CODEX_PROMPT";
        let answer = "RESUMED_CODEX_ANSWER";
        write_codex_rollout(&rollout, &cwd, old_session_ts, prompt, answer);

        let pane_agents = HashMap::from([
            ("a".to_string(), "codex".to_string()),
            ("b".to_string(), "claude".to_string()),
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
            status_tx: None,
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
        let _guard = env_lock().lock().await;

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
            status_tx: None,
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
        let _guard = env_lock().lock().await;

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
            status_tx: None,
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
    async fn codex_transcript_binding_excludes_rollout_bound_to_other_pane() {
        let _guard = env_lock().lock().await;

        let temp = tempdir().unwrap();
        let codex_home = temp.path().join("codex");
        let cwd = temp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        let prev_codex_home = std::env::var_os("CODEX_HOME");
        std::env::set_var("CODEX_HOME", &codex_home);

        let started_at = chrono::Utc::now() - chrono::Duration::seconds(10);
        let session_ts = chrono::Utc::now() + chrono::Duration::seconds(1);
        let prompt = "SAME_PROMPT_FOR_TWO_CODEX_PANES";
        let first_rollout = codex_home
            .join("sessions")
            .join("2026")
            .join("04")
            .join("28")
            .join("rollout-first-pane.jsonl");
        let second_rollout = codex_home
            .join("sessions")
            .join("2026")
            .join("04")
            .join("28")
            .join("rollout-second-pane.jsonl");
        write_codex_rollout(
            &first_rollout,
            &cwd,
            session_ts,
            prompt,
            "FIRST_PANE_ANSWER",
        );
        write_codex_rollout(
            &second_rollout,
            &cwd,
            session_ts,
            prompt,
            "SECOND_PANE_ANSWER",
        );

        let mut transcripts = HashMap::from([("b".to_string(), second_rollout.clone())]);
        let pending_prompts = HashMap::from([("a".to_string(), prompt.to_string())]);

        ensure_codex_transcript_local(
            "a",
            &mut transcripts,
            &pending_prompts,
            &cwd,
            started_at,
            &temp.path().join("relay.log"),
        );

        restore_codex_home(prev_codex_home);

        assert_eq!(
            transcripts.get("a"),
            Some(&first_rollout),
            "Codex pane A should not bind to the rollout already owned by pane B"
        );
        assert_eq!(
            transcripts.get("b"),
            Some(&second_rollout),
            "Codex pane B should keep its existing rollout binding"
        );
    }

    #[tokio::test]
    async fn codex_transcript_binding_does_not_fallback_to_unmatched_rollout() {
        let _guard = env_lock().lock().await;

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
            .join("28")
            .join("rollout-other-pane.jsonl");
        write_codex_rollout(
            &rollout,
            &cwd,
            session_ts,
            "PROMPT_FROM_OTHER_PANE",
            "OTHER_PANE_ANSWER",
        );

        let mut transcripts = HashMap::new();
        let pending_prompts = HashMap::from([("a".to_string(), "PROMPT_FROM_A".to_string())]);

        ensure_codex_transcript_local(
            "a",
            &mut transcripts,
            &pending_prompts,
            &cwd,
            started_at,
            &temp.path().join("relay.log"),
        );

        restore_codex_home(prev_codex_home);

        assert!(
            !transcripts.contains_key("a"),
            "Codex pane A should not bind an unrelated recent rollout"
        );
    }

    #[tokio::test]
    async fn codex_manual_input_keeps_existing_transcript_binding() {
        let _guard = env_lock().lock().await;
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
            status_tx: None,
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
        let _guard = env_lock().lock().await;

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
            status_tx: None,
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
