use super::*;
use clap::Parser;

#[test]
fn parses_start_with_mixed_agents() {
    let cli = Cli::parse_from(["cduo", "start", "claude", "codex"]);

    match cli.command.unwrap() {
        Commands::Start {
            agent, peer_agent, ..
        } => {
            assert_eq!(agent, Agent::Claude);
            assert_eq!(peer_agent, Some(Agent::Codex));
        }
        _ => panic!("expected start command"),
    }
}

#[test]
fn parses_split_layout() {
    let cli = Cli::parse_from(["cduo", "start", "codex", "claude", "--split", "rows"]);

    match cli.command.unwrap() {
        Commands::Start { split, .. } => {
            assert_eq!(split, SplitLayout::Rows);
        }
        _ => panic!("expected start command"),
    }
}

#[test]
fn parses_agent_shorthand_with_peer_agent() {
    let cli = Cli::parse_from(["cduo", "claude", "codex"]);

    match cli.command.unwrap() {
        Commands::Claude { peer_agent, .. } => {
            assert_eq!(peer_agent, Some(Agent::Codex));
        }
        _ => panic!("expected claude command"),
    }

    let cli = Cli::parse_from(["cduo", "codex", "claude"]);

    match cli.command.unwrap() {
        Commands::Codex { peer_agent, .. } => {
            assert_eq!(peer_agent, Some(Agent::Claude));
        }
        _ => panic!("expected codex command"),
    }
}

#[test]
fn parses_start_flags_before_agent() {
    let cli = Cli::parse_from(["cduo", "start", "--new", "codex"]);

    match cli.command.unwrap() {
        Commands::Start {
            agent,
            peer_agent,
            new_session,
            ..
        } => {
            assert_eq!(agent, Agent::Codex);
            assert_eq!(peer_agent, None);
            assert!(new_session);
        }
        _ => panic!("expected start command"),
    }
}

#[test]
fn parses_start_access_flags_between_agents() {
    let cli = Cli::parse_from(["cduo", "start", "claude", "--yolo", "codex"]);

    match cli.command.unwrap() {
        Commands::Start {
            agent,
            peer_agent,
            yolo,
            full_access,
            ..
        } => {
            assert_eq!(agent, Agent::Claude);
            assert_eq!(peer_agent, Some(Agent::Codex));
            assert!(yolo);
            assert!(!full_access);
        }
        _ => panic!("expected start command"),
    }

    let cli = Cli::parse_from(["cduo", "start", "codex", "--full-access"]);

    match cli.command.unwrap() {
        Commands::Start {
            agent,
            peer_agent,
            yolo,
            full_access,
            ..
        } => {
            assert_eq!(agent, Agent::Codex);
            assert_eq!(peer_agent, None);
            assert!(!yolo);
            assert!(full_access);
        }
        _ => panic!("expected start command"),
    }
}

#[test]
fn parses_session_name_and_roles() {
    let cli = Cli::parse_from([
        "cduo",
        "start",
        "--session-name",
        "api",
        "--role-a",
        "planner",
        "--role-b",
        "builder",
    ]);

    match cli.command.unwrap() {
        Commands::Start {
            session_name,
            role_a,
            role_b,
            ..
        } => {
            assert_eq!(session_name.as_deref(), Some("api"));
            assert_eq!(role_a.as_deref(), Some("planner"));
            assert_eq!(role_b.as_deref(), Some("builder"));
        }
        _ => panic!("expected start command"),
    }
}

#[test]
fn parses_claude_session_name_and_roles() {
    let cli = Cli::parse_from([
        "cduo",
        "claude",
        "codex",
        "--session-name",
        "api",
        "--role-a",
        "planner",
        "--role-b",
        "builder",
    ]);

    match cli.command.unwrap() {
        Commands::Claude {
            peer_agent,
            session_name,
            role_a,
            role_b,
            ..
        } => {
            assert_eq!(peer_agent, Some(Agent::Codex));
            assert_eq!(session_name.as_deref(), Some("api"));
            assert_eq!(role_a.as_deref(), Some("planner"));
            assert_eq!(role_b.as_deref(), Some("builder"));
        }
        _ => panic!("expected claude command"),
    }
}

#[test]
fn parses_codex_session_name_and_roles() {
    let cli = Cli::parse_from([
        "cduo",
        "codex",
        "claude",
        "--session-name",
        "api",
        "--role-a",
        "planner",
        "--role-b",
        "builder",
    ]);

    match cli.command.unwrap() {
        Commands::Codex {
            peer_agent,
            session_name,
            role_a,
            role_b,
            ..
        } => {
            assert_eq!(peer_agent, Some(Agent::Claude));
            assert_eq!(session_name.as_deref(), Some("api"));
            assert_eq!(role_a.as_deref(), Some("planner"));
            assert_eq!(role_b.as_deref(), Some("builder"));
        }
        _ => panic!("expected codex command"),
    }
}

#[test]
fn parses_doctor_subcommands() {
    let cli = Cli::parse_from(["cduo", "doctor", "paths"]);
    match cli.command.unwrap() {
        Commands::Doctor { command } => {
            assert_eq!(command, Some(DoctorCommand::Paths));
        }
        _ => panic!("expected doctor command"),
    }

    let cli = Cli::parse_from(["cduo", "doctor"]);
    match cli.command.unwrap() {
        Commands::Doctor { command } => {
            assert_eq!(command, None);
        }
        _ => panic!("expected doctor command"),
    }

    let cli = Cli::parse_from(["cduo", "doctor", "runtime"]);
    match cli.command.unwrap() {
        Commands::Doctor { command } => {
            assert_eq!(command, Some(DoctorCommand::Runtime));
        }
        _ => panic!("expected doctor command"),
    }
}

#[test]
fn parses_init_target() {
    let cli = Cli::parse_from(["cduo", "init", "--target", "/tmp/cduo-target"]);
    match cli.command.unwrap() {
        Commands::Init { force, target } => {
            assert!(!force);
            assert_eq!(
                target.as_deref(),
                Some(std::path::Path::new("/tmp/cduo-target"))
            );
        }
        _ => panic!("expected init command"),
    }
}

#[test]
fn bare_cduo_defaults_to_start() {
    let cli = Cli::parse_from(["cduo"]);
    assert!(cli.command.is_none());
}
