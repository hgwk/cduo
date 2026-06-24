use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::relay_core_prompt::{
    codex_transcript_contains_user_prompt, codex_transcript_contains_user_prompt_since,
};
use crate::transcripts::{self, TranscriptOutput};

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

fn collect_jsonl_files(dir: &Path, out: &mut Vec<PathBuf>) {
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

fn claude_transcript_meta(path: &Path) -> Option<(PathBuf, chrono::DateTime<chrono::Utc>)> {
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

fn codex_session_meta(path: &Path) -> Option<(PathBuf, chrono::DateTime<chrono::Utc>)> {
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

pub fn discover_recent_codex_transcript_after_prompt(
    cwd: &Path,
    started_at: chrono::DateTime<chrono::Utc>,
    excluded: &HashSet<PathBuf>,
    expected_prompt: &str,
    prompt_recorded_at: Option<chrono::DateTime<chrono::Utc>>,
) -> Option<PathBuf> {
    discover_recent_codex_transcript_in_root(
        &codex_sessions_root(),
        cwd,
        started_at,
        excluded,
        expected_prompt,
        prompt_recorded_at,
    )
}

pub(crate) fn discover_recent_codex_transcript_in_root(
    root: &Path,
    cwd: &Path,
    started_at: chrono::DateTime<chrono::Utc>,
    excluded: &HashSet<PathBuf>,
    expected_prompt: &str,
    prompt_recorded_at: Option<chrono::DateTime<chrono::Utc>>,
) -> Option<PathBuf> {
    let mut files = Vec::new();
    collect_jsonl_files(root, &mut files);

    files
        .into_iter()
        .filter(|path| !excluded.contains(path))
        .filter(|path| {
            prompt_recorded_at.map_or_else(
                || codex_transcript_contains_user_prompt(path, expected_prompt),
                |recorded_at| {
                    codex_transcript_contains_user_prompt_since(
                        path,
                        expected_prompt,
                        Some(recorded_at),
                    )
                },
            )
        })
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

pub(crate) fn discover_recent_claude_transcript_in_root(
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
