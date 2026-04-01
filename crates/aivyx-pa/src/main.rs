//! Aivyx Personal Assistant — main entry point.
//!
//! Usage:
//!   aivyx              Launch TUI (default)
//!   aivyx init         First-time setup
//!   aivyx chat "..."   One-shot chat from terminal
//!   aivyx status       Show what the assistant has been doing
//!   aivyx config       View/edit configuration

mod init;
mod tui;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "aivyx",
    about = "Your private AI personal assistant",
    version,
    after_help = "Run without arguments to launch the interactive TUI."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// First-time setup: passphrase, provider, model
    Init,
    /// One-shot chat (non-interactive)
    Chat {
        /// Your message
        message: String,
    },
    /// Show recent assistant activity
    Status,
    /// View or edit configuration
    Config,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();

    // Resolve home directory
    let home = dirs::home_dir()
        .expect("could not determine home directory")
        .join(".aivyx");

    match cli.command {
        // No subcommand → launch TUI
        None => {
            // Check if initialized
            if !home.join("keys").join("master.json").exists() {
                eprintln!("Aivyx is not set up yet. Run `aivyx init` first.");
                std::process::exit(1);
            }
            tui::run(&home).await?;
        }
        Some(Command::Init) => {
            init::run(&home).await?;
        }
        Some(Command::Chat { message }) => {
            if !home.join("keys").join("master.json").exists() {
                eprintln!("Aivyx is not set up yet. Run `aivyx init` first.");
                std::process::exit(1);
            }
            chat_oneshot(&home, &message).await?;
        }
        Some(Command::Status) => {
            println!("(status view not yet implemented)");
        }
        Some(Command::Config) => {
            println!("(config editor not yet implemented)");
        }
    }

    Ok(())
}

/// One-shot chat: send a message, print the response, exit.
async fn chat_oneshot(home: &std::path::Path, message: &str) -> anyhow::Result<()> {
    let _ = (home, message);
    // TODO: Initialize agent, send message, stream response to stdout
    println!("(one-shot chat not yet implemented)");
    Ok(())
}
