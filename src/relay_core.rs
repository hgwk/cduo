//! Pure helpers shared by the native runtime's relay loop:
//! transcript parsing, codex rollout discovery, hook-summary counting,
//! deduplication, and structured logging to a per-session log file.

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::message::Message;
use crate::message_bus::MessageBus;
use crate::pair_router::PairRouter;
use crate::transcripts::{self, TranscriptOutput};

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
        // Claude's TUI can need a longer beat after bracketed paste before
        // Enter is accepted.
        "claude" => DEFAULT_CLAUDE_SUBMIT_DELAY_MS,
        _ => DEFAULT_SUBMIT_DELAY_MS,
    }
}

pub fn codex_sessions_root() -> PathBuf {
    std::env::var("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".codex")
        })
        .join("sessions")
}

pub fn collect_jsonl_files(dir: &Path, out: &mut Vec<PathBuf>) {
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

pub fn codex_session_meta(path: &Path) -> Option<(PathBuf, chrono::DateTime<chrono::Utc>)> {
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

pub fn normalize_prompt_text(value: &str) -> String {
    value.replace('\r', "").trim().to_string()
}

pub fn codex_transcript_contains_user_prompt(path: &Path, expected_prompt: &str) -> bool {
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

pub fn codex_user_text_from_entry(entry: &serde_json::Value) -> Option<String> {
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

pub fn discover_recent_codex_transcript(
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

pub fn count_claude_stop_hook_summaries(path: &Path) -> usize {
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

pub async fn read_claude_transcript_with_retry(
    path: &Path,
    previous_signature: Option<&String>,
    previous_stop_count: usize,
) -> TranscriptOutput {
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
    TranscriptOutput::empty()
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

pub fn publish_transcript_output(
    bus: &mut MessageBus,
    router: &PairRouter,
    log_path: &Path,
    pane_id: &str,
    output: &TranscriptOutput,
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

pub fn pane_uses_codex(pane_agents: &HashMap<String, String>, pane_id: &str) -> bool {
    pane_agents.get(pane_id).map(String::as_str) == Some("codex")
}

pub fn pane_uses_claude(pane_agents: &HashMap<String, String>, pane_id: &str) -> bool {
    pane_agents.get(pane_id).map(String::as_str) == Some("claude")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_stop_hook_summary_with_json_spacing() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            file.path(),
            "{\"type\": \"system\", \"subtype\": \"stop_hook_summary\"}\n\
             {\"type\":\"system\",\"subtype\":\"stop_hook_summary\"}\n",
        )
        .unwrap();

        assert_eq!(count_claude_stop_hook_summaries(file.path()), 2);
    }

    #[test]
    fn pane_agent_helpers_are_pane_specific() {
        let pane_agents = HashMap::from([
            ("a".to_string(), "claude".to_string()),
            ("b".to_string(), "codex".to_string()),
        ]);

        assert!(pane_uses_claude(&pane_agents, "a"));
        assert!(!pane_uses_codex(&pane_agents, "a"));
        assert!(pane_uses_codex(&pane_agents, "b"));
        assert!(!pane_uses_claude(&pane_agents, "b"));
    }

    #[test]
    fn submit_delay_is_longer_for_claude() {
        assert_eq!(
            submit_delay_for_agent("claude"),
            DEFAULT_CLAUDE_SUBMIT_DELAY_MS
        );
        assert_eq!(submit_delay_for_agent("codex"), DEFAULT_SUBMIT_DELAY_MS);
    }

    #[test]
    fn normalize_prompt_text_trims_and_strips_cr() {
        assert_eq!(normalize_prompt_text("  hi\r\n  "), "hi");
        assert_eq!(normalize_prompt_text("\r\rok"), "ok");
    }

    #[test]
    fn preview_caps_length_and_escapes_newlines() {
        let p = preview("a\nb\rc");
        assert_eq!(p, "a\\nb\\rc");
        let long: String = "x".repeat(200);
        assert_eq!(preview(&long).chars().count(), 160);
    }
}
