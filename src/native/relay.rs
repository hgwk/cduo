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
use crate::native::relay_delivery::*;
use crate::native::relay_handlers::*;
use crate::pair_router::PairRouter;
use crate::relay_core::log_event;

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
    pub pair_id: String,
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
        pair_id,
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
    let codex_pending_prompts: HashMap<String, PendingPrompt> = HashMap::new();
    let controls = RelayControlState::from_env();

    log_event(&log_path, format!("native_relay_start pair={pair_id}"));
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
    log_event(&log_path, format!("native_relay_stop pair={pair_id}"));
}

pub(super) struct RelayState {
    pub(super) bus: MessageBus,
    pub(super) router: PairRouter,
    pub(super) rx_a: mpsc::Receiver<Message>,
    pub(super) rx_b: mpsc::Receiver<Message>,
    pub(super) codex_transcripts: HashMap<String, PathBuf>,
    pub(super) codex_last_signatures: HashMap<String, String>,
    pub(super) claude_last_signatures: HashMap<String, String>,
    pub(super) claude_last_stop_counts: HashMap<String, usize>,
    pub(super) claude_transcripts: HashMap<String, PathBuf>,
    pub(super) codex_pending_prompts: HashMap<String, PendingPrompt>,
    pub(super) controls: RelayControlState,
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

#[cfg(test)]
#[path = "relay_tests.rs"]
mod tests;
