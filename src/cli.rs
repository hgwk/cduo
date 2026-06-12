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

        #[arg(long, value_enum, default_value = "columns")]
        split: SplitLayout,

        #[arg(long, default_value_t = false)]
        yolo: bool,

        #[arg(long, default_value_t = false)]
        full_access: bool,

        #[arg(long = "new", alias = "new-session", default_value_t = false)]
        new_session: bool,

        #[arg(long, alias = "session")]
        session_name: Option<String>,

        #[arg(long)]
        role_a: Option<String>,

        #[arg(long)]
        role_b: Option<String>,
    },

    #[command(about = "Start a native pair with Claude in pane A")]
    Claude {
        #[arg(value_enum)]
        peer_agent: Option<Agent>,

        #[arg(long, value_enum, default_value = "columns")]
        split: SplitLayout,

        #[arg(long, default_value_t = false)]
        yolo: bool,

        #[arg(long, default_value_t = false)]
        full_access: bool,

        #[arg(long = "new", alias = "new-session", default_value_t = false)]
        new_session: bool,

        #[arg(long, alias = "session")]
        session_name: Option<String>,

        #[arg(long)]
        role_a: Option<String>,

        #[arg(long)]
        role_b: Option<String>,
    },

    #[command(about = "Start a native pair with Codex in pane A")]
    Codex {
        #[arg(value_enum)]
        peer_agent: Option<Agent>,

        #[arg(long, value_enum, default_value = "columns")]
        split: SplitLayout,

        #[arg(long, default_value_t = false)]
        yolo: bool,

        #[arg(long, default_value_t = false)]
        full_access: bool,

        #[arg(long = "new", alias = "new-session", default_value_t = false)]
        new_session: bool,

        #[arg(long, alias = "session")]
        session_name: Option<String>,

        #[arg(long)]
        role_a: Option<String>,

        #[arg(long)]
        role_b: Option<String>,
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
    Doctor {
        #[command(subcommand)]
        command: Option<DoctorCommand>,
    },

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum SplitLayout {
    #[default]
    Columns,
    Rows,
}

#[derive(Clone, Debug, PartialEq, Eq, Subcommand)]
pub enum DoctorCommand {
    #[command(about = "Run the standard readiness check")]
    Check,
    #[command(about = "Print cduo, Claude, Codex, and project guide paths")]
    Paths,
    #[command(about = "Print Claude hook locations and command counts")]
    Hooks,
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
