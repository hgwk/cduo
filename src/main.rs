use clap::Parser;
use std::process;

mod cli;
mod daemon;
mod hook;
mod message;
mod message_bus;
mod pair_router;
mod project;
mod pty;
mod session;
mod tmux;
mod transcripts;

use cli::Commands;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().collect();

    if args.len() >= 2 && args[1] == "__daemon" {
        let session_id = args
            .iter()
            .position(|a| a == "--session")
            .and_then(|i| args.get(i + 1))
            .cloned()
            .unwrap_or_default();

        if session_id.is_empty() {
            eprintln!("Error: --session required for daemon mode");
            process::exit(1);
        }

        if let Err(e) = daemon::run_daemon(session_id).await {
            eprintln!("Daemon error: {e}");
            process::exit(1);
        }
        return;
    }

    let args = cli::Cli::parse();

    match args.command {
        Commands::Start {
            agent,
            yolo,
            full_access,
            new_session,
        } => {
            if let Err(e) = daemon::start(agent, yolo, full_access, new_session).await {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
        Commands::Claude {
            yolo,
            full_access,
            new_session,
        } => {
            if let Err(e) = daemon::start(cli::Agent::Claude, yolo, full_access, new_session).await
            {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
        Commands::Codex {
            yolo,
            full_access,
            new_session,
        } => {
            if let Err(e) = daemon::start(cli::Agent::Codex, yolo, full_access, new_session).await {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
        Commands::Stop { session } => {
            if let Err(e) = daemon::stop(session).await {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
        Commands::Resume { session } => {
            if let Err(e) = daemon::resume(session).await {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
        Commands::Status { verbose } => {
            if let Err(e) = daemon::status(verbose).await {
                eprintln!("Error: {e}");
                process::exit(1);
            }
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
            println!("cduo 2.0.0");
        }
        Commands::AttachPane {
            session_id,
            pane_id,
        } => {
            if let Err(e) = daemon::attach_pane(session_id, pane_id).await {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
    }
}
