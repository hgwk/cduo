use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use crate::transcripts::TranscriptOutput;

pub const DEFAULT_SUBMIT_DELAY_MS: u64 = 300;
pub const DEFAULT_CLAUDE_SUBMIT_DELAY_MS: u64 = 900;

pub fn log_event(path: &Path, message: impl AsRef<str>) {
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

pub fn preview(value: &str) -> String {
    value
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .chars()
        .take(160)
        .collect()
}

pub fn submit_delay_for_agent(target_agent: &str) -> u64 {
    let env_key = if target_agent == "claude" {
        "CDUO_CLAUDE_SUBMIT_DELAY_MS"
    } else {
        "CDUO_SUBMIT_DELAY_MS"
    };
    if let Ok(value) = std::env::var(env_key) {
        if let Ok(ms) = value.parse::<u64>() {
            return ms;
        }
    }

    match target_agent {
        "claude" => DEFAULT_CLAUDE_SUBMIT_DELAY_MS,
        _ => DEFAULT_SUBMIT_DELAY_MS,
    }
}

pub fn drop_seen_signature(
    pane_id: &str,
    output: TranscriptOutput,
    last_signatures: &mut HashMap<String, String>,
) -> TranscriptOutput {
    let Some(signature) = &output.signature else {
        return output;
    };

    if last_signatures.get(pane_id) == Some(signature) {
        TranscriptOutput::empty()
    } else {
        last_signatures.insert(pane_id.to_string(), signature.clone());
        output
    }
}

pub fn pane_uses_codex(pane_agents: &HashMap<String, String>, pane_id: &str) -> bool {
    pane_agents.get(pane_id).map(String::as_str) == Some("codex")
}

pub fn pane_uses_claude(pane_agents: &HashMap<String, String>, pane_id: &str) -> bool {
    pane_agents.get(pane_id).map(String::as_str) == Some("claude")
}
