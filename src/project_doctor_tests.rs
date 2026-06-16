use super::*;

#[test]
fn counts_only_session_start_command_hooks() {
    let settings = serde_json::json!({
        "hooks": {
            "SessionStart": [
                {"hooks": [
                    {"type": "command", "command": "echo ok"},
                    {"type": "command", "command": "  "},
                    {"type": "prompt", "command": "ignored"}
                ]}
            ],
            "Stop": [
                {"hooks": [{"type": "command", "command": "echo stop"}]}
            ]
        }
    });
    assert_eq!(count_hook_commands(&settings, "SessionStart"), 1);
    assert_eq!(count_hook_commands(&settings, "Stop"), 1);
}

#[test]
fn startup_hook_report_identifies_project_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let settings_path = tmp.path().join(".claude/settings.local.json");
    std::fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
    std::fs::write(
        &settings_path,
        serde_json::json!({
            "hooks": {
                "SessionStart": [{"hooks": [{"type": "command", "command": "echo ok"}]}]
            }
        })
        .to_string(),
    )
    .unwrap();

    let report = claude_startup_hooks_report(&[settings_path]);
    assert!(report.contains("found 1 command(s)"));
}

#[test]
fn startup_hook_report_warns_about_multiple_hooks() {
    let tmp = tempfile::tempdir().unwrap();
    let settings_path = tmp.path().join("settings.json");
    std::fs::write(
        &settings_path,
        serde_json::json!({
            "hooks": {
                "SessionStart": [
                    {"hooks": [{"type": "command", "command": "one"}]},
                    {"hooks": [{"type": "command", "command": "two"}]}
                ]
            }
        })
        .to_string(),
    )
    .unwrap();

    let report = claude_startup_hooks_report(&[settings_path]);
    assert!(report.contains("found 2 command(s)"));
    assert!(report.contains("multiple Claude SessionStart hooks"));
    assert!(report.contains("cduo relay uses the project Stop hook"));
}

#[test]
fn startup_hook_report_separates_invalid_json_from_hook_count() {
    let tmp = tempfile::tempdir().unwrap();
    let settings_path = tmp.path().join("settings.json");
    std::fs::write(&settings_path, "{not json").unwrap();

    let report = claude_startup_hooks_report(&[settings_path]);
    assert!(report.contains("invalid JSON"));
}

#[test]
fn stop_hook_pair_id_report_detects_pair_aware_hook() {
    let tmp = tempfile::tempdir().unwrap();
    let settings_path = tmp.path().join("settings.json");
    std::fs::write(
        &settings_path,
        serde_json::json!({
            "hooks": {
                "Stop": [{"hooks": [{
                    "type": "command",
                    "command": "CDUO_PAIR_ID=${CDUO_PAIR_ID:-} python3 -c 'payload={\"pair_id\":\"x\",\"terminal_id\":\"a\"}'"
                }]}]
            }
        })
        .to_string(),
    )
    .unwrap();

    let report = claude_stop_pair_id_report(&[settings_path]);
    assert!(report.contains("found pair-aware hook"));
}

#[test]
fn stop_hook_pair_id_report_warns_when_stop_hook_is_not_pair_aware() {
    let tmp = tempfile::tempdir().unwrap();
    let settings_path = tmp.path().join("settings.json");
    std::fs::write(
        &settings_path,
        serde_json::json!({
            "hooks": {
                "Stop": [{"hooks": [{"type": "command", "command": "cduo relay"}]}]
            }
        })
        .to_string(),
    )
    .unwrap();

    let report = claude_stop_pair_id_report(&[settings_path]);
    assert!(report.contains("missing CDUO_PAIR_ID"));
    assert!(report.contains("cduo init"));
}

#[test]
fn stop_hook_pair_id_report_warns_about_mixed_legacy_cduo_hooks() {
    let tmp = tempfile::tempdir().unwrap();
    let settings_path = tmp.path().join("settings.json");
    std::fs::write(
        &settings_path,
        serde_json::json!({
            "hooks": {
                "Stop": [{"hooks": [
                    {
                        "type": "command",
                        "command": "CDUO_PAIR_ID=${CDUO_PAIR_ID:-} python3 -c 'payload={\"pair_id\":\"x\",\"terminal_id\":\"a\"}; url=\"/hook\"'"
                    },
                    {
                        "type": "command",
                        "command": "python3 -c 'payload={\"terminal_id\":\"a\"}; url=\"/hook\"'"
                    }
                ]}]
            }
        })
        .to_string(),
    )
    .unwrap();

    let report = claude_stop_pair_id_report(&[settings_path]);
    assert!(report.contains("found pair-aware hook"));
    assert!(report.contains("legacy cduo Stop hook without pair id"));
}
