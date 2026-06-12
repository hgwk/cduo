use crate::message::Message;
use crate::message_bus::{MessageBus, PublishResult};
use crate::pair_router::PairRouter;
use crate::relay_core::{log_event, normalize_prompt_text, preview};
use crate::transcripts::TranscriptOutput;

#[cfg(test)]
use crate::relay_core::drop_seen_signature;
#[cfg(test)]
use crate::transcripts;
#[cfg(test)]
use std::{collections::HashMap, path::PathBuf};

const SHORT_OUTPUT_SUPPRESSION_LIMIT: usize = 6;
const DEFAULT_STOP_TOKEN: &str = "~~~";

#[derive(Debug, Clone)]
pub(super) struct RelayControlState {
    pub(super) a_to_b_enabled: bool,
    pub(super) b_to_a_enabled: bool,
    pub(super) delivery_prefix: Option<String>,
    pub(super) max_auto_relays: Option<usize>,
    pub(super) stop_token: String,
    pub(super) auto_relay_count: usize,
    pub(super) last_auto_content: Option<(String, String)>,
    pub(super) stopped: bool,
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
            last_auto_content: None,
            stopped: false,
        }
    }
}

impl RelayControlState {
    pub(super) fn from_env() -> Self {
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

    pub(super) fn set_route_enabled(&mut self, source: &str, target: &str, enabled: bool) -> bool {
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

    pub(super) fn route_enabled(&self, source: &str, target: &str) -> bool {
        match (source, target) {
            ("a", "b") => self.a_to_b_enabled,
            ("b", "a") => self.b_to_a_enabled,
            _ => false,
        }
    }

    pub(super) fn set_delivery_prefix(&mut self, prefix: impl Into<String>) {
        let prefix = prefix.into();
        self.delivery_prefix = (!prefix.is_empty()).then_some(prefix);
    }

    pub(super) fn delivered_content(&self, content: &str) -> String {
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

    pub(super) fn reset_stop(&mut self) {
        self.stopped = false;
        self.auto_relay_count = 0;
        self.last_auto_content = None;
    }

    fn should_stop_for_duplicate(&self, source: &str, content: &str) -> bool {
        let key = duplicate_content_key(content);
        !key.is_empty()
            && self
                .last_auto_content
                .as_ref()
                .is_some_and(|(last_source, last_key)| last_source != source && last_key == &key)
    }

    fn record_auto_content(&mut self, source: &str, content: &str) {
        let key = duplicate_content_key(content);
        if !key.is_empty() {
            self.last_auto_content = Some((source.to_string(), key));
        }
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

fn duplicate_content_key(content: &str) -> String {
    normalize_prompt_text(content).trim().to_string()
}

pub(super) fn publish_transcript_output_with_controls(
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
    if controls.should_stop_for_duplicate(pane_id, &output.output) {
        controls.stop();
        log_event(
            log_path,
            format!(
                "relay_duplicate_loop source={pane_id} len={} text=\"{}\"",
                output.output.len(),
                preview(&output.output)
            ),
        );
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
        controls.record_auto_content(&source, &output.output);
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

pub(super) fn should_suppress_transcript_output(output: &str) -> bool {
    output.trim().is_empty() || output.len() <= SHORT_OUTPUT_SUPPRESSION_LIMIT
}

#[cfg(test)]
pub(super) fn publish_bound_codex_transcript_with_controls(
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
