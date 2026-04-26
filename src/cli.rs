use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "cduo")]
#[command(about = "Run Claude Code and the OpenAI Codex CLI side by side")]
#[command(version = "2.0.0")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    #[command(about = "Start a cduo workspace")]
    Start {
        #[arg(value_enum, default_value = "claude")]
        agent: Agent,

        #[arg(long, default_value_t = false)]
        yolo: bool,

        #[arg(long, default_value_t = false)]
        full_access: bool,

        #[arg(long = "new", alias = "new-session", default_value_t = false)]
        new_session: bool,
    },

    #[command(about = "Start or reconnect to a Claude workspace")]
    Claude {
        #[arg(long, default_value_t = false)]
        yolo: bool,

        #[arg(long, default_value_t = false)]
        full_access: bool,

        #[arg(long = "new", alias = "new-session", default_value_t = false)]
        new_session: bool,
    },

    #[command(about = "Start or reconnect to a Codex workspace")]
    Codex {
        #[arg(long, default_value_t = false)]
        yolo: bool,

        #[arg(long, default_value_t = false)]
        full_access: bool,

        #[arg(long = "new", alias = "new-session", default_value_t = false)]
        new_session: bool,
    },

    #[command(about = "Stop the current or named workspace")]
    Stop { session: Option<String> },

    #[command(about = "Resume a workspace")]
    Resume { session: Option<String> },

    #[command(about = "Show active cduo workspaces")]
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

    #[command(name = "__attach-pane", hide = true)]
    AttachPane { session_id: String, pane_id: String },
}

#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
pub enum Agent {
    #[default]
    Claude,
    Codex,
}
