use super::*;
use tempfile::tempdir;

#[tokio::test]
async fn codex_transcript_binding_allows_small_timestamp_skew_for_manual_input() {
    let _guard = env_lock().lock().await;

    let temp = tempdir().unwrap();
    let codex_home = temp.path().join("codex");
    let cwd = temp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let prev_codex_home = std::env::var_os("CODEX_HOME");
    std::env::set_var("CODEX_HOME", &codex_home);

    let started_at = chrono::Utc::now() - chrono::Duration::seconds(10);
    let prompt = "MANUAL_INPUT_PROMPT_WITH_CLOCK_SKEW";
    let pending_prompt = PendingPrompt::new(prompt.to_string());
    let message_at = pending_prompt.recorded_at + chrono::Duration::milliseconds(500);
    let rollout = codex_home
        .join("sessions")
        .join("2026")
        .join("06")
        .join("19")
        .join("rollout-manual-skew.jsonl");
    write_codex_rollout_with_message_timestamp(
        &rollout,
        &cwd,
        message_at,
        message_at,
        prompt,
        "MANUAL_SKEW_ANSWER",
    );

    let mut transcripts = HashMap::new();
    let pending_prompts = HashMap::from([(
        "a".to_string(),
        PendingPrompt {
            text: pending_prompt.text,
            recorded_at: pending_prompt.recorded_at,
        },
    )]);

    ensure_codex_transcript_local(
        "a",
        &mut transcripts,
        &pending_prompts,
        &cwd,
        started_at,
        &temp.path().join("relay.log"),
    );

    restore_codex_home(prev_codex_home);

    assert_eq!(
        transcripts.get("a"),
        Some(&rollout),
        "manual Codex input should tolerate small timestamp skew"
    );
}
