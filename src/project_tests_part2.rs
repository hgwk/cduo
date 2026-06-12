    #[test]
    fn test_ensure_instruction_reference_preserves_front_matter() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        fs::write(&path, "---\ntitle: Keep\n---\n# Body\n").unwrap();

        let changed = ensure_instruction_reference(&path, false).unwrap();
        assert!(changed);

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(&orchestration_ref().unwrap()));
        assert!(content.contains("---\ntitle: Keep\n---\n# Body"));
    }

    #[test]
    fn test_ensure_instruction_reference_preserves_body_reference_as_content() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        let legacy_ref = LEGACY_ORCHESTRATION_REF;
        fs::write(
            &path,
            format!("# Body\n\nDocument mentions {legacy_ref} inline.\n"),
        )
        .unwrap();

        let changed = ensure_instruction_reference(&path, false).unwrap();
        assert!(changed);

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(&orchestration_ref().unwrap()));
        assert!(content.contains(&format!("Document mentions {legacy_ref} inline.")));
    }

    #[test]
    fn test_remove_instruction_reference_preserves_front_matter_without_cduo_ref() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        fs::write(&path, "---\ntitle: Keep\n---\n# Body\n").unwrap();

        assert!(!remove_instruction_reference(&path).unwrap());

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "---\ntitle: Keep\n---\n# Body\n");
    }

    #[test]
    fn test_remove_instruction_reference_preserves_body_reference_without_prelude() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        let original = format!("# Body\n\nDocument mentions {LEGACY_ORCHESTRATION_REF} inline.\n");
        fs::write(&path, &original).unwrap();

        assert!(!remove_instruction_reference(&path).unwrap());

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, original);
    }

    #[test]
    fn test_ensure_instruction_reference_prepends_existing_agents_policy() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        fs::write(&path, "# Existing Policy\n").unwrap();

        assert!(ensure_instruction_reference(&path, false).unwrap());

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(&orchestration_ref().unwrap()));
        assert!(content.contains("# Existing Policy"));
    }

    #[test]
    fn test_ensure_instruction_reference_preserves_body_reference_in_agents() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        let original = format!("# Existing Policy\n\nMention {LEGACY_ORCHESTRATION_REF} only.\n");
        fs::write(&path, &original).unwrap();

        assert!(ensure_instruction_reference(&path, false).unwrap());

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(&orchestration_ref().unwrap()));
        assert!(content.contains(&format!("Mention {LEGACY_ORCHESTRATION_REF} only.")));
    }

    #[test]
    fn test_uninstall_removes_orchestration() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        fs::write(
            &path,
            format!("{}\n\n---\n\n# Existing\n", orchestration_ref().unwrap()),
        )
        .unwrap();

        assert!(remove_instruction_reference(&path).unwrap());

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "# Existing\n");
    }

    #[test]
    fn test_uninstall_removes_agents_reference_but_keeps_body() {
        let _guard = env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        fs::write(
            &path,
            format!(
                "{}\n\n---\n\n# Existing Policy\n",
                orchestration_ref().unwrap()
            ),
        )
        .unwrap();

        let previous_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        let result = uninstall();
        std::env::set_current_dir(previous_dir).unwrap();

        assert!(result.is_ok());
        assert_eq!(fs::read_to_string(&path).unwrap(), "# Existing Policy\n");
        assert!(tmp.path().join(".cduo").join("backups").exists());
    }
