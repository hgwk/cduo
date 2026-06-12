use super::*;
use std::collections::{HashMap, HashSet};

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
fn normalize_prompt_text_applies_backspace_and_strips_control_noise() {
    assert_eq!(normalize_prompt_text("gk\u{7f}\u{7f}하이"), "하이");
    assert_eq!(
        normalize_prompt_text("\x1b[?1;2;4c\x1b]10;rgb:eded/ecec/eeee\x07하이"),
        "하이"
    );
}

#[test]
fn codex_prompt_match_tolerates_whitespace_changes() {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
            file.path(),
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello\nfrom codex"}]}}"#,
        )
        .unwrap();

    assert!(codex_transcript_contains_user_prompt(
        file.path(),
        "hello from codex"
    ));
}

#[test]
fn discovers_recent_claude_transcript_for_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let projects_root = temp.path().join("projects");
    let project_dir = projects_root.join("-tmp-project");
    std::fs::create_dir_all(&project_dir).unwrap();
    let cwd = temp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let path = project_dir.join("session.jsonl");
    let timestamp = chrono::Utc::now();
    std::fs::write(
            &path,
            format!(
                "{{\"type\":\"assistant\",\"message\":{{\"role\":\"assistant\",\"content\":\"hello\"}},\"cwd\":\"{}\",\"timestamp\":\"{}\"}}\n",
                cwd.display(),
                timestamp.to_rfc3339()
            ),
        )
        .unwrap();

    let discovered = discover_recent_claude_transcript_in_root(
        &projects_root,
        &cwd,
        timestamp - chrono::Duration::seconds(1),
        &HashSet::new(),
    );

    assert_eq!(discovered, Some(path));
}

#[test]
fn discovers_resumed_codex_transcript_when_file_was_modified_after_runtime_start() {
    let temp = tempfile::tempdir().unwrap();
    let sessions_root = temp.path().join("sessions");
    let cwd = temp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let path = sessions_root.join("rollout-resumed.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let old_session_ts = chrono::Utc::now() - chrono::Duration::days(1);
    let started_at = chrono::Utc::now() - chrono::Duration::seconds(1);
    let cwd_json = serde_json::to_string(&cwd.to_string_lossy()).unwrap();
    let prompt_json = serde_json::to_string("RESUMED_PROMPT").unwrap();
    std::fs::write(
            &path,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":{cwd_json},\"timestamp\":\"{}\"}}}}\n\
                 {{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":{prompt_json}}}]}}}}\n",
                old_session_ts.to_rfc3339()
            ),
        )
        .unwrap();

    let discovered = discover_recent_codex_transcript_in_root(
        &sessions_root,
        &cwd,
        started_at,
        &HashSet::new(),
        "RESUMED_PROMPT",
    );

    assert_eq!(discovered, Some(path));
}

#[test]
fn preview_caps_length_and_escapes_newlines() {
    let p = preview("a\nb\rc");
    assert_eq!(p, "a\\nb\\rc");
    let long: String = "x".repeat(200);
    assert_eq!(preview(&long).chars().count(), 160);
}
