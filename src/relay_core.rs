//! Pure helpers shared by the native runtime's relay loop:
//! transcript parsing, codex rollout discovery, hook-summary counting,
//! deduplication, and structured logging to a per-session log file.

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

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

pub fn claude_projects_root() -> PathBuf {
    std::env::var("CLAUDE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".claude")
        })
        .join("projects")
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

pub fn claude_transcript_meta(path: &Path) -> Option<(PathBuf, chrono::DateTime<chrono::Utc>)> {
    let content = std::fs::read_to_string(path).ok()?;
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|value| {
            let cwd = value.get("cwd").and_then(serde_json::Value::as_str)?;
            let timestamp = value
                .get("timestamp")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())?
                .with_timezone(&chrono::Utc);
            Some((PathBuf::from(cwd), timestamp))
        })
        .max_by_key(|(_, timestamp)| *timestamp)
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
    let mut out = Vec::new();
    let mut chars = value.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '\u{7f}' | '\u{8}' => {
                out.pop();
            }
            '\x1b' => {
                skip_escape_sequence(&mut chars);
            }
            '\r' => {}
            '\n' => out.push(ch),
            ch if ch.is_control() => {}
            ch => out.push(ch),
        }
    }

    out.into_iter().collect::<String>().trim().to_string()
}

fn skip_escape_sequence<I>(chars: &mut std::iter::Peekable<I>)
where
    I: Iterator<Item = char>,
{
    match chars.peek().copied() {
        Some('[') => {
            chars.next();
            for ch in chars.by_ref() {
                if ('@'..='~').contains(&ch) {
                    break;
                }
            }
        }
        Some(']') => {
            chars.next();
            let mut prev = '\0';
            for ch in chars.by_ref() {
                if ch == '\u{7}' || (prev == '\x1b' && ch == '\\') {
                    break;
                }
                prev = ch;
            }
        }
        Some(_) => {
            chars.next();
        }
        None => {}
    }
}

pub fn codex_transcript_contains_user_prompt(path: &Path, expected_prompt: &str) -> bool {
    let expected_prompt = normalize_prompt_text(expected_prompt);
    let compact_expected = compact_prompt_text(&expected_prompt);
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
            codex_user_text_from_entry(&entry).is_some_and(|text| {
                let compact_text = compact_prompt_text(&text);
                text == expected_prompt
                    || text.contains(&expected_prompt)
                    || expected_prompt.contains(&text)
                    || (!compact_text.is_empty()
                        && !compact_expected.is_empty()
                        && (compact_text.contains(&compact_expected)
                            || compact_expected.contains(&compact_text)))
            })
        })
}

fn compact_prompt_text(value: &str) -> String {
    value.chars().filter(|c| !c.is_whitespace()).collect()
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
    discover_recent_codex_transcript_in_root(
        &codex_sessions_root(),
        cwd,
        started_at,
        excluded,
        expected_prompt,
    )
}

fn discover_recent_codex_transcript_in_root(
    root: &Path,
    cwd: &Path,
    started_at: chrono::DateTime<chrono::Utc>,
    excluded: &HashSet<PathBuf>,
    expected_prompt: &str,
) -> Option<PathBuf> {
    let mut files = Vec::new();
    collect_jsonl_files(root, &mut files);

    files
        .into_iter()
        .filter(|path| !excluded.contains(path))
        .filter(|path| codex_transcript_contains_user_prompt(path, expected_prompt))
        .filter_map(|path| {
            let (session_cwd, session_started_at) = codex_session_meta(&path)?;
            let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
            let modified_at = system_time_to_utc(modified);
            if session_cwd != cwd || (session_started_at < started_at && modified_at < started_at) {
                return None;
            }
            Some((modified, path))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

pub fn discover_recent_claude_transcript(
    cwd: &Path,
    started_at: chrono::DateTime<chrono::Utc>,
    excluded: &HashSet<PathBuf>,
) -> Option<PathBuf> {
    discover_recent_claude_transcript_in_root(&claude_projects_root(), cwd, started_at, excluded)
}

fn discover_recent_claude_transcript_in_root(
    root: &Path,
    cwd: &Path,
    started_at: chrono::DateTime<chrono::Utc>,
    excluded: &HashSet<PathBuf>,
) -> Option<PathBuf> {
    let mut files = Vec::new();
    collect_jsonl_files(root, &mut files);

    files
        .into_iter()
        .filter(|path| !excluded.contains(path))
        .filter_map(|path| {
            let (session_cwd, last_timestamp) = claude_transcript_meta(&path)?;
            let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
            let modified_at = system_time_to_utc(modified);
            if session_cwd != cwd || (last_timestamp < started_at && modified_at < started_at) {
                return None;
            }
            Some((last_timestamp, modified, path))
        })
        .max_by_key(|(last_timestamp, modified, _)| (*last_timestamp, *modified))
        .map(|(_, _, path)| path)
}

fn system_time_to_utc(value: SystemTime) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::<chrono::Utc>::from(value)
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

pub fn pane_uses_codex(pane_agents: &HashMap<String, String>, pane_id: &str) -> bool {
    pane_agents.get(pane_id).map(String::as_str) == Some("codex")
}

pub fn pane_uses_claude(pane_agents: &HashMap<String, String>, pane_id: &str) -> bool {
    pane_agents.get(pane_id).map(String::as_str) == Some("claude")
}

#[cfg(test)]
#[path = "relay_core_tests.rs"]
mod tests;
