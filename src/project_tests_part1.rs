    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn test_ensure_stop_hook_creates_new() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");

        let changed = ensure_stop_hook(&path, false).unwrap();
        assert!(changed);
        assert!(path.exists());

        let content: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(content.get("hooks").and_then(|h| h.get("Stop")).is_some());
    }

    #[test]
    fn test_ensure_stop_hook_merges_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        fs::write(&path, r#"{"permissions":{"defaultMode":"accept"}}"#).unwrap();

        let changed = ensure_stop_hook(&path, false).unwrap();
        assert!(changed);

        let content: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["permissions"]["defaultMode"], "accept");
        assert!(content["hooks"]["Stop"].is_array());
    }

    #[test]
    fn test_ensure_stop_hook_force_overwrites_non_cduo_stop_hook() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        fs::write(
            &path,
            serde_json::json!({
                "hooks": {
                    "Stop": [{
                        "matcher": ".*",
                        "hooks": [{"type": "command", "command": "python3 custom.py"}]
                    }]
                }
            })
            .to_string(),
        )
        .unwrap();

        let changed = ensure_stop_hook(&path, true).unwrap();
        assert!(changed);
        let content: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(is_cduo_stop_hook(&content["hooks"]["Stop"]));
        assert!(!content.to_string().contains("custom.py"));
    }

    #[test]
    fn test_ensure_stop_hook_preserves_non_cduo_stop_hook_without_force() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        let custom = serde_json::json!({
            "hooks": {
                "Stop": [{
                    "matcher": ".*",
                    "hooks": [{"type": "command", "command": "python3 custom.py"}]
                }]
            }
        });
        fs::write(&path, custom.to_string()).unwrap();

        let changed = ensure_stop_hook(&path, false).unwrap();
        assert!(!changed);
        let content: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content, custom);
    }

    #[test]
    fn test_remove_cduo_stop_hooks_preserves_non_cduo_stop_hook() {
        let mut settings = serde_json::json!({
            "hooks": {
                "Stop": [
                    {
                        "matcher": ".*",
                        "hooks": [
                            {
                                "type": "command",
                                "command": "python3 custom_stop_hook.py"
                            }
                        ]
                    }
                ]
            }
        });

        assert!(!remove_cduo_stop_hooks_from_settings(&mut settings));
        assert_eq!(
            settings["hooks"]["Stop"][0]["hooks"][0]["command"],
            "python3 custom_stop_hook.py"
        );
    }

    #[test]
    fn test_remove_cduo_stop_hooks_preserves_mixed_non_cduo_entries() {
        let template = template_settings().unwrap();
        let cduo_entry = template["hooks"]["Stop"][0].clone();
        let custom_entry = serde_json::json!({
            "matcher": ".*",
            "hooks": [
                {
                    "type": "command",
                    "command": "python3 custom_stop_hook.py"
                }
            ]
        });
        let mut settings = serde_json::json!({
            "permissions": { "defaultMode": "accept" },
            "hooks": {
                "Stop": [cduo_entry, custom_entry.clone()],
                "PreToolUse": [{ "matcher": ".*", "hooks": [] }]
            }
        });

        assert!(remove_cduo_stop_hooks_from_settings(&mut settings));
        assert_eq!(settings["hooks"]["Stop"], serde_json::json!([custom_entry]));
        assert!(settings["hooks"].get("PreToolUse").is_some());
        assert_eq!(settings["permissions"]["defaultMode"], "accept");
    }

    #[test]
    fn test_remove_cduo_stop_hooks_removes_empty_hooks_object() {
        let template = template_settings().unwrap();
        let mut settings = serde_json::json!({
            "hooks": {
                "Stop": template["hooks"]["Stop"].clone()
            }
        });

        assert!(remove_cduo_stop_hooks_from_settings(&mut settings));
        assert!(settings.get("hooks").is_none());
    }

    #[test]
    fn test_ensure_orchestration_file_creates_new() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".cduo").join("orchestration.md");

        let changed = ensure_orchestration_file(&path, false).unwrap();
        assert!(changed);
        assert!(path.exists());
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("cduo Collaboration Mode"));
    }

    #[test]
    fn test_ensure_instruction_reference_creates_new() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        let reference = orchestration_ref().unwrap();

        let changed = ensure_instruction_reference(&path, false).unwrap();
        assert!(changed);

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, format!("{reference}\n"));
    }

    #[test]
    fn test_ensure_instruction_reference_prepends_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        fs::write(&path, "# My Project\n\nExisting content.").unwrap();

        let changed = ensure_instruction_reference(&path, false).unwrap();
        assert!(changed);

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(&orchestration_ref().unwrap()));
        assert!(content.contains("My Project"));
    }

    #[test]
    fn test_ensure_instruction_reference_replaces_legacy_block() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        fs::write(
            &path,
            format!(
                "{}\nlegacy\n{}\n\n---\n\n# My Project\n",
                ORCHESTRATION_START, ORCHESTRATION_END
            ),
        )
        .unwrap();

        let changed = ensure_instruction_reference(&path, false).unwrap();
        assert!(changed);

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(&orchestration_ref().unwrap()));
        assert!(!content.contains("legacy"));
        assert!(content.contains("My Project"));
    }

    #[test]
    fn test_ensure_instruction_reference_force_preserves_existing_body() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        fs::write(
            &path,
            format!("{}\n\n---\n\n# Keep Me\n", orchestration_ref().unwrap()),
        )
        .unwrap();

        let changed = ensure_instruction_reference(&path, true).unwrap();
        assert!(!changed);

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(&orchestration_ref().unwrap()));
        assert!(content.contains("# Keep Me"));
    }
