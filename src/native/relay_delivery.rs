use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;

use crate::message::Message;
use crate::native::relay_control::*;
use crate::pair_router::PairRouter;
use crate::relay_core::{
    discover_recent_codex_transcript_after_prompt, log_event, normalize_prompt_text,
    pane_uses_claude, pane_uses_codex, preview, submit_delay_for_agent,
};
use crate::transcripts::{self, TranscriptOutput};

const PROMPT_MATCH_GRACE_MS: i64 = 2_000;

#[derive(Debug, Clone)]
pub(super) struct PendingPrompt {
    pub(super) text: String,
    pub(super) recorded_at: DateTime<Utc>,
}

impl PendingPrompt {
    pub(super) fn new(text: String) -> Self {
        Self {
            text,
            recorded_at: Utc::now() - chrono::Duration::milliseconds(PROMPT_MATCH_GRACE_MS),
        }
    }
}

pub(super) async fn deliver_via_channel(
    log_path: &std::path::Path,
    rx_a: &mut mpsc::Receiver<Message>,
    rx_b: &mut mpsc::Receiver<Message>,
    write_tx: &mpsc::Sender<(String, Vec<u8>)>,
    pane_agents: &HashMap<String, String>,
    pending_prompts: &mut HashMap<String, PendingPrompt>,
) {
    while let Ok(msg) = rx_a.try_recv() {
        let content = prefixed_agent_content(&msg.source_node_id, &msg.content, pane_agents);
        log_deliver(log_path, "a", &content);
        pending_prompts.insert(
            "a".to_string(),
            PendingPrompt::new(normalize_prompt_text(&content)),
        );
        let agent = pane_agents.get("a").map(String::as_str).unwrap_or("claude");
        send_relay_via_channel(write_tx, "a", &content, agent).await;
    }
    while let Ok(msg) = rx_b.try_recv() {
        let content = prefixed_agent_content(&msg.source_node_id, &msg.content, pane_agents);
        log_deliver(log_path, "b", &content);
        pending_prompts.insert(
            "b".to_string(),
            PendingPrompt::new(normalize_prompt_text(&content)),
        );
        let agent = pane_agents.get("b").map(String::as_str).unwrap_or("claude");
        send_relay_via_channel(write_tx, "b", &content, agent).await;
    }
}

pub(super) fn prefixed_agent_content(
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

pub(super) async fn send_relay_via_channel(
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

pub(super) struct ManualRelayContext<'a> {
    pub(super) router: &'a PairRouter,
    pub(super) controls: &'a RelayControlState,
    pub(super) pane_agents: &'a HashMap<String, String>,
    pub(super) codex_transcripts: &'a HashMap<String, PathBuf>,
    pub(super) claude_transcripts: &'a HashMap<String, PathBuf>,
    pub(super) pending_prompts: &'a mut HashMap<String, PendingPrompt>,
    pub(super) write_tx: &'a mpsc::Sender<(String, Vec<u8>)>,
    pub(super) log_path: &'a std::path::Path,
}

pub(super) async fn manual_relay(pane_id: &str, ctx: ManualRelayContext<'_>) {
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
    ctx.pending_prompts.insert(
        target.to_string(),
        PendingPrompt::new(normalize_prompt_text(&content)),
    );
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

pub(super) fn ensure_codex_transcript_local(
    pane_id: &str,
    transcripts: &mut HashMap<String, PathBuf>,
    pending_prompts: &HashMap<String, PendingPrompt>,
    cwd: &std::path::Path,
    started_at: DateTime<Utc>,
    log_path: &std::path::Path,
) {
    let Some(pending_prompt) = pending_prompts.get(pane_id) else {
        return;
    };
    let expected_prompt = &pending_prompt.text;

    if transcripts.get(pane_id).is_some_and(|path| {
        crate::relay_core::codex_transcript_contains_user_prompt_since(
            path,
            expected_prompt,
            Some(pending_prompt.recorded_at),
        )
    }) {
        return;
    }

    let used_by_other_pane = transcripts
        .iter()
        .filter(|(source, _)| source.as_str() != pane_id)
        .map(|(_, path)| path.clone())
        .collect::<std::collections::HashSet<_>>();
    let Some(path) = discover_recent_codex_transcript_after_prompt(
        cwd,
        started_at,
        &used_by_other_pane,
        expected_prompt,
        Some(pending_prompt.recorded_at),
    ) else {
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
