use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;

use crate::hook::HookEvent;
use crate::native::relay::{RelayControl, RelayState};
use crate::native::relay_control::*;
use crate::native::relay_delivery::{
    ensure_codex_transcript_local, manual_relay, ManualRelayContext, PendingPrompt,
};
use crate::relay_core::{
    count_claude_stop_hook_summaries, discover_recent_claude_transcript, drop_seen_signature,
    log_event, normalize_prompt_text, pane_uses_claude, pane_uses_codex, preview,
    read_claude_transcript_with_retry,
};
use crate::transcripts::{self, TranscriptOutput};

pub(super) fn handle_relay_input(
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
            .insert(pane_id.to_string(), PendingPrompt::new(prompt));
    }
}

pub(super) async fn handle_relay_control(
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

pub(super) fn poll_codex_transcripts(
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

pub(super) async fn handle_claude_hook_event(
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
    if let Some(pair_id) = event.pair_id.as_deref() {
        log_event(
            log_path,
            format!("hook_event_pair source={pane_id} pair={pair_id}"),
        );
    }
    let transcript_path = event
        .transcript_path
        .as_deref()
        .map(PathBuf::from)
        .or_else(|| discover_claude_fallback(cwd, started_at, log_path, state, &pane_id));
    if let Some(path) = transcript_path.as_ref() {
        state
            .claude_transcripts
            .insert(pane_id.clone(), path.clone());
    }

    let output = read_claude_output(&pane_id, transcript_path.as_deref(), state).await;
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

fn discover_claude_fallback(
    cwd: &std::path::Path,
    started_at: DateTime<Utc>,
    log_path: &std::path::Path,
    state: &RelayState,
    pane_id: &str,
) -> Option<PathBuf> {
    let used_by_other_pane = state
        .claude_transcripts
        .iter()
        .filter(|(source, _)| source.as_str() != pane_id)
        .map(|(_, path)| path.clone())
        .collect::<std::collections::HashSet<_>>();
    let discovered = discover_recent_claude_transcript(cwd, started_at, &used_by_other_pane);
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
}

async fn read_claude_output(
    pane_id: &str,
    transcript_path: Option<&std::path::Path>,
    state: &mut RelayState,
) -> TranscriptOutput {
    let Some(path) = transcript_path else {
        return TranscriptOutput::empty();
    };
    let previous = state.claude_last_signatures.get(pane_id).cloned();
    let previous_stop_count = state
        .claude_last_stop_counts
        .get(pane_id)
        .copied()
        .unwrap_or(0);
    let output =
        read_claude_transcript_with_retry(path, previous.as_ref(), previous_stop_count).await;
    let new_stop_count = count_claude_stop_hook_summaries(path);
    if new_stop_count > previous_stop_count {
        state
            .claude_last_stop_counts
            .insert(pane_id.to_string(), new_stop_count);
    }
    drop_seen_signature(pane_id, output, &mut state.claude_last_signatures)
}
