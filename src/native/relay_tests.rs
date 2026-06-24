use super::*;
use crate::message_bus::PublishResult;
use crate::transcripts::TranscriptOutput;
use std::sync::OnceLock;
use std::time::Duration;
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
            Ok(None) | Err(_) => break,
        }
    }
    out.extend(drain_writes(rx));
    if out
        .iter()
        .any(|(_, bytes)| String::from_utf8_lossy(bytes).contains("\x1b[200~"))
        && !out.iter().any(|(_, bytes)| bytes == b"\r")
    {
        let submit_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < submit_deadline {
            let remaining = submit_deadline.saturating_duration_since(tokio::time::Instant::now());
            match timeout(remaining, rx.recv()).await {
                Ok(Some(item)) => {
                    let is_enter = item.1 == b"\r";
                    out.push(item);
                    if is_enter {
                        break;
                    }
                }
                Ok(None) | Err(_) => break,
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

fn assert_paste_write_contains(
    writes: &[(String, Vec<u8>)],
    expected_target: &str,
    expected: &str,
) {
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
            s.contains("\x1b[200~").then_some(s.to_string())
        })
        .expect("expected at least one bracketed-paste bundle");
    assert!(
        body.contains(expected),
        "paste body missing expected content: {body:?}"
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
    write_codex_rollout_with_message_timestamp(
        path,
        cwd,
        timestamp,
        timestamp,
        user_prompt,
        assistant_text,
    );
}

fn write_codex_rollout_with_message_timestamp(
    path: &std::path::Path,
    cwd: &std::path::Path,
    session_timestamp: chrono::DateTime<chrono::Utc>,
    message_timestamp: chrono::DateTime<chrono::Utc>,
    user_prompt: &str,
    assistant_text: &str,
) {
    let cwd_json = serde_json::to_string(&cwd.to_string_lossy()).unwrap();
    let session_ts = session_timestamp.to_rfc3339();
    let message_ts = message_timestamp.to_rfc3339();
    let user_json = serde_json::to_string(user_prompt).unwrap();
    let assistant_json = serde_json::to_string(assistant_text).unwrap();
    let body = format!(
        "{{\"timestamp\":\"{session_ts}\",\"type\":\"session_meta\",\"payload\":{{\"cwd\":{cwd_json},\"timestamp\":\"{session_ts}\"}}}}\n\
         {{\"timestamp\":\"{message_ts}\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":{user_json}}}]}}}}\n\
         {{\"timestamp\":\"{message_ts}\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"assistant\",\"phase\":\"final_answer\",\"content\":[{{\"type\":\"output_text\",\"text\":{assistant_json}}}]}}}}\n",
    );
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn transcript_output(text: &str) -> TranscriptOutput {
    TranscriptOutput::new(text.to_string(), format!("test-signature-{text}"))
}

#[path = "relay_tests_part1.rs"]
mod relay_tests_part1;
#[path = "relay_tests_part10.rs"]
mod relay_tests_part10;
#[path = "relay_tests_part11.rs"]
mod relay_tests_part11;
#[path = "relay_tests_part2.rs"]
mod relay_tests_part2;
#[path = "relay_tests_part3.rs"]
mod relay_tests_part3;
#[path = "relay_tests_part4.rs"]
mod relay_tests_part4;
#[path = "relay_tests_part5.rs"]
mod relay_tests_part5;
#[path = "relay_tests_part6.rs"]
mod relay_tests_part6;
#[path = "relay_tests_part7.rs"]
mod relay_tests_part7;
#[path = "relay_tests_part8.rs"]
mod relay_tests_part8;
#[path = "relay_tests_part9.rs"]
mod relay_tests_part9;
