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
        session_name: None,
        role_a: None,
        role_b: None,
    }) {
        Commands::Start {
            agent,
            peer_agent,
            split,
            yolo,
            full_access,
            new_session,
            session_name,
            role_a,
            role_b,
        } => {
            run_native_or_exit(native::runtime::RuntimeOptions {
                agent_a: agent,
                agent_b: peer_agent.unwrap_or(agent),
                split,
                yolo,
                full_access,
                new_session,
                session_name,
                role_a,
                role_b,
            })
            .await;
        }
        Commands::Claude {
            peer_agent,
            split,
            yolo,
            full_access,
            new_session,
            session_name,
            role_a,
            role_b,
        } => {
            run_native_or_exit(native::runtime::RuntimeOptions {
                agent_a: cli::Agent::Claude,
                agent_b: peer_agent.unwrap_or(cli::Agent::Claude),
                split,
                yolo,
                full_access,
                new_session,
                session_name,
                role_a,
                role_b,
            })
            .await;
        }
        Commands::Codex {
            peer_agent,
            split,
            yolo,
            full_access,
            new_session,
            session_name,
            role_a,
            role_b,
        } => {
            run_native_or_exit(native::runtime::RuntimeOptions {
                agent_a: cli::Agent::Codex,
                agent_b: peer_agent.unwrap_or(cli::Agent::Codex),
                split,
                yolo,
                full_access,
                new_session,
                session_name,
                role_a,
                role_b,
            })
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
        Commands::Doctor { command } => {
            let result = match command {
                None | Some(cli::DoctorCommand::Check) => project::doctor(),
                Some(cli::DoctorCommand::Paths) => project::doctor_paths(),
                Some(cli::DoctorCommand::Hooks) => project::doctor_hooks(),
            };
            if let Err(e) = result {
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

async fn run_native_or_exit(opts: native::runtime::RuntimeOptions) {
    if let Err(e) = native::runtime::run(opts).await {
        eprintln!("Error: {e}");
        process::exit(1);
    }
}
