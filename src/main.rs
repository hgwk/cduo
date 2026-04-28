use clap::Parser;
use std::process;

mod cli;
mod hook;
mod message;
mod message_bus;
mod native;
mod pair_router;
mod project;
mod relay_core;
mod session;
mod transcripts;

use cli::Commands;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let args = cli::Cli::parse();

    match args.command.unwrap_or(Commands::Start {
        agent: cli::Agent::Claude,
        peer_agent: None,
        split: cli::SplitLayout::Columns,
        yolo: false,
        full_access: false,
        new_session: false,
    }) {
        Commands::Start {
            agent,
            peer_agent,
            split,
            yolo,
            full_access,
            new_session,
        } => {
            run_native_or_exit(
                agent,
                peer_agent.unwrap_or(agent),
                split,
                yolo,
                full_access,
                new_session,
            )
            .await;
        }
        Commands::Claude {
            peer_agent,
            split,
            yolo,
            full_access,
            new_session,
        } => {
            run_native_or_exit(
                cli::Agent::Claude,
                peer_agent.unwrap_or(cli::Agent::Claude),
                split,
                yolo,
                full_access,
                new_session,
            )
            .await;
        }
        Commands::Codex {
            peer_agent,
            split,
            yolo,
            full_access,
            new_session,
        } => {
            run_native_or_exit(
                cli::Agent::Codex,
                peer_agent.unwrap_or(cli::Agent::Codex),
                split,
                yolo,
                full_access,
                new_session,
            )
            .await;
        }
        Commands::Status { verbose } => {
            let _ = verbose;
            println!("Native cduo sessions run in the foreground. No background tmux sessions are managed.");
        }
        Commands::Init { force } => {
            if let Err(e) = project::init(force) {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
        Commands::Doctor => {
            if let Err(e) = project::doctor() {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
        Commands::Backup => {
            if let Err(e) = project::backup() {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
        Commands::Uninstall => {
            if let Err(e) = project::uninstall() {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
        Commands::Update => {
            if let Err(e) = project::update() {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
        Commands::Version => {
            println!("cduo {}", env!("CARGO_PKG_VERSION"));
        }
    }
}

async fn run_native_or_exit(
    agent_a: cli::Agent,
    agent_b: cli::Agent,
    split: cli::SplitLayout,
    yolo: bool,
    full_access: bool,
    new_session: bool,
) {
    let opts = native::runtime::RuntimeOptions {
        agent_a,
        agent_b,
        split,
        yolo,
        full_access,
        new_session,
    };
    if let Err(e) = native::runtime::run(opts).await {
        eprintln!("Error: {e}");
        process::exit(1);
    }
}
