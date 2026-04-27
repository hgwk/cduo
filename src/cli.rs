use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "cduo")]
#[command(about = "Run Claude Code and the OpenAI Codex CLI side by side")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    #[command(about = "Start a cduo workspace")]
    Start {
        #[arg(value_enum, default_value = "claude")]
        agent: Agent,

        #[arg(value_enum)]
        peer_agent: Option<Agent>,

        #[arg(long, default_value_t = false)]
        yolo: bool,

        #[arg(long, default_value_t = false)]
        full_access: bool,

        #[arg(long = "new", alias = "new-session", default_value_t = false)]
        new_session: bool,
    },

    #[command(about = "Start a Claude/Claude native pair")]
    Claude {
        #[arg(long, default_value_t = false)]
        yolo: bool,

        #[arg(long, default_value_t = false)]
        full_access: bool,

        #[arg(long = "new", alias = "new-session", default_value_t = false)]
        new_session: bool,
    },

    #[command(about = "Start a Codex/Codex native pair")]
    Codex {
        #[arg(long, default_value_t = false)]
        yolo: bool,

        #[arg(long, default_value_t = false)]
        full_access: bool,

        #[arg(long = "new", alias = "new-session", default_value_t = false)]
        new_session: bool,
    },

    #[command(about = "Explain native foreground session behavior")]
    Status {
        #[arg(long, short)]
        verbose: bool,
    },

    #[command(about = "Initialize Claude orchestration files")]
    Init {
        #[arg(long, short)]
        force: bool,
    },

    #[command(about = "Check setup")]
    Doctor,

    #[command(about = "Backup orchestration files")]
    Backup,

    #[command(about = "Remove orchestration settings")]
    Uninstall,

    #[command(about = "Update cduo")]
    Update,

    #[command(about = "Show version")]
    Version,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Agent {
    #[default]
    Claude,
    Codex,
}

#[cfg(test)]
mod tests {
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
    fn bare_cduo_defaults_to_start() {
        let cli = Cli::parse_from(["cduo"]);
        assert!(cli.command.is_none());
    }
}
